use std::{fs, path::Path};

/// Build a self-contained HTML slideshow from a list of PNG paths.
/// The PNGs are embedded as base64 data URIs so the file has no external
/// dependencies and can be attached to Confluence or sent by email.
pub fn build_html_slideshow(
    png_paths: &[&Path],
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if png_paths.is_empty() {
        return Err("No PNG frames provided for HTML slideshow".into());
    }

    // Encode every PNG as a base64 data URI.
    let slides: Vec<String> = png_paths
        .iter()
        .map(|p| {
            let bytes = fs::read(p)
                .map_err(|e| format!("Failed to read {}: {}", p.display(), e))?;
            Ok(format!("data:image/png;base64,{}", base64_encode(&bytes)))
        })
        .collect::<Result<_, Box<dyn std::error::Error>>>()?;

    let slides_js = slides
        .iter()
        .map(|s| format!("\"{}\"", s))
        .collect::<Vec<_>>()
        .join(",\n    ");

    let total = slides.len();

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Slideshow</title>
<style>
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  body {{
    background: #1a1a1a;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: 100vh;
    font-family: sans-serif;
    color: #eee;
  }}
  #slide-container {{
    position: relative;
    max-width: 95vw;
    max-height: 80vh;
  }}
  #slide {{
    display: block;
    max-width: 95vw;
    max-height: 80vh;
    box-shadow: 0 4px 24px rgba(0,0,0,0.6);
  }}
  #controls {{
    display: flex;
    align-items: center;
    gap: 16px;
    margin-top: 16px;
  }}
  button {{
    background: #444;
    color: #eee;
    border: none;
    border-radius: 6px;
    padding: 10px 24px;
    font-size: 16px;
    cursor: pointer;
    transition: background 0.15s;
  }}
  button:hover:not(:disabled) {{ background: #666; }}
  button:disabled {{ opacity: 0.3; cursor: default; }}
  #counter {{ font-size: 15px; min-width: 60px; text-align: center; }}
</style>
</head>
<body>
<div id="slide-container">
  <img id="slide" src="" alt="Slide">
</div>
<div id="controls">
  <button id="btn-prev" onclick="go(-1)">&#8592; Prev</button>
  <span id="counter">1 / {total}</span>
  <button id="btn-next" onclick="go(1)">Next &#8594;</button>
</div>
<script>
  var slides = [
    {slides_js}
  ];
  var current = 0;

  function show(n) {{
    current = Math.max(0, Math.min(slides.length - 1, n));
    document.getElementById("slide").src = slides[current];
    document.getElementById("counter").textContent = (current + 1) + " / " + slides.length;
    document.getElementById("btn-prev").disabled = current === 0;
    document.getElementById("btn-next").disabled = current === slides.length - 1;
  }}

  function go(delta) {{ show(current + delta); }}

  document.addEventListener("keydown", function(e) {{
    if (e.key === "ArrowRight" || e.key === "ArrowDown") go(1);
    if (e.key === "ArrowLeft"  || e.key === "ArrowUp")   go(-1);
  }});

  show(0);
</script>
</body>
</html>
"#,
        total = total,
        slides_js = slides_js,
    );

    fs::write(output_path, html)?;
    Ok(())
}

/// Minimal base64 encoder (no external dependency needed).
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((n >> 18) & 0x3f) as usize] as char);
        out.push(CHARS[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 { CHARS[((n >> 6) & 0x3f) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { CHARS[(n & 0x3f) as usize] as char } else { '=' });
    }
    out
}
