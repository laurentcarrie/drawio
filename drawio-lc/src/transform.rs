use std::{collections::{HashMap, HashSet}, fs, io::Write, path::Path, process::Command};

use pulldown_cmark::{html, Options, Parser};
use quick_xml::{
    events::{BytesStart, Event},
    Reader, Writer,
};

use crate::model::{Derived, Transform};

/// Apply all transforms defined in `derived` to `input_xml` and write the
/// result to `derived.output`.
pub fn transform(
    input_xml: &str,
    derived: &Derived,
    ref_size: Option<(u32, u32)>,
) -> Result<(), Box<dyn std::error::Error>> {
    let xml = apply_transforms(input_xml, &derived.transforms, ref_size)?;

    let output_path = Path::new(&derived.output);
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let mut file = fs::File::create(output_path)?;
    file.write_all(xml.as_bytes())?;

    export_png(output_path, ref_size)?;

    Ok(())
}

/// Export a drawio file to a specific PNG output path, with no size constraint.
/// Used to produce the reference PNG from the original file.
pub fn export_reference_png(
    drawio_path: &Path,
    png_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("drawio")
        .args([
            "--export",
            "--format",
            "png",
            "--output",
            png_path.to_str().ok_or("invalid output path")?,
            drawio_path.to_str().ok_or("invalid input path")?,
        ])
        .stderr(std::process::Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!(
            "drawio exited with status {} while exporting reference PNG",
            s
        )
        .into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err("drawio CLI not found — install the draw.io desktop app and ensure `drawio` is on PATH".into())
        }
        Err(e) => Err(e.into()),
    }
}

/// Shell out to the draw.io CLI to export the given `.drawio` file as PNG.
/// The PNG is written next to the drawio file with a `.png` extension.
/// If `size` is provided, `--width` and `--height` are passed to fix the canvas size.
fn export_png(drawio_path: &Path, size: Option<(u32, u32)>) -> Result<(), Box<dyn std::error::Error>> {
    let png_path = drawio_path.with_extension("png");
    let width_str;
    let height_str;

    let mut args = vec![
        "--export",
        "--format",
        "png",
        "--output",
        png_path.to_str().ok_or("invalid output path")?,
    ];

    if let Some((w, h)) = size {
        width_str = w.to_string();
        height_str = h.to_string();
        args.push("--width");
        args.push(&width_str);
        args.push("--height");
        args.push(&height_str);
    }

    args.push(drawio_path.to_str().ok_or("invalid input path")?);

    let status = Command::new("drawio").args(&args).stderr(std::process::Stdio::null()).status();

    match status {
        Ok(s) if s.success() => {
            println!("Exported {}", png_path.display());
            Ok(())
        }
        Ok(s) => Err(format!(
            "drawio exited with status {} while exporting {}",
            s,
            drawio_path.display()
        )
        .into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err("drawio CLI not found — install the draw.io desktop app and ensure `drawio` is on PATH".into())
        }
        Err(e) => Err(e.into()),
    }
}

/// Walk the XML stream and apply every transform in order, returning the
/// modified XML string.
fn apply_transforms(
    input_xml: &str,
    transforms: &[Transform],
    ref_size: Option<(u32, u32)>,
) -> Result<String, Box<dyn std::error::Error>> {
    // TitleSlide must be the only transform — short-circuit here.
    if transforms.len() == 1 {
        if let Transform::TitleSlide { text } = &transforms[0] {
            let (w, h) = ref_size.ok_or(
                "TitleSlide requires a reference size — ensure the original file is exported first",
            )?;
            return Ok(build_title_slide_xml(text, w, h));
        }
    }
    if transforms.iter().any(|t| matches!(t, Transform::TitleSlide { .. })) {
        return Err("TitleSlide must be the only transform in the list".into());
    }

    // Validate LayerVisibility transforms before touching the XML.
    validate_layer_transforms(input_xml, transforms)?;

    // Pre-compute the set of edge ids to recolor for each ColorEdges transform.
    let edge_ids_to_recolor = collect_edge_ids_to_recolor(input_xml, transforms)?;

    let mut reader = Reader::from_str(input_xml);
    reader.config_mut().trim_text(false);

    let mut writer = Writer::new(Vec::new());

    loop {
        match reader.read_event()? {
            Event::Eof => break,
            Event::Start(elem) if elem.name().as_ref() == b"mxCell" => {
                let patched = patch_cell(elem, transforms, &edge_ids_to_recolor)?;
                writer.write_event(Event::Start(patched))?;
            }
            Event::Empty(elem) if elem.name().as_ref() == b"mxCell" => {
                let patched = patch_cell(elem, transforms, &edge_ids_to_recolor)?;
                writer.write_event(Event::Empty(patched))?;
            }
            other => {
                writer.write_event(other)?;
            }
        }
    }

    Ok(String::from_utf8(writer.into_inner())?)
}

/// Generate a minimal drawio XML containing only a centered text label,
/// sized to match the reference PNG dimensions.
/// `text` is interpreted as Markdown and converted to HTML.
fn build_title_slide_xml(text: &str, width: u32, height: u32) -> String {
    // Convert Markdown to HTML.
    let mut html_output = String::new();
    let parser = Parser::new_ext(text, Options::all());
    html::push_html(&mut html_output, parser);

    // Escape for use inside an XML attribute value.
    let escaped = html_output
        .trim()
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', "&#xa;")
        .replace('\r', "");

    // A white background rectangle gives drawio concrete bounds to export,
    // and a text cell on top renders the Markdown HTML centred.
    format!(
        r#"<mxfile host="drawio-lc"><diagram name="Title"><mxGraphModel><root><mxCell id="0"/><mxCell id="1" parent="0"/><mxCell id="bg" parent="1" vertex="1" style="rounded=0;whiteSpace=wrap;html=1;fillColor=#ffffff;strokeColor=none;"><mxGeometry x="0" y="0" width="{w}" height="{h}" as="geometry"/></mxCell><mxCell id="2" parent="1" value="{text}" vertex="1" style="text;html=1;align=center;verticalAlign=middle;fontSize=24;fontStyle=0;whiteSpace=wrap;"><mxGeometry x="0" y="0" width="{w}" height="{h}" as="geometry"/></mxCell></root></mxGraphModel></diagram></mxfile>"#,
        w = width,
        h = height,
        text = escaped,
    )
}

/// Validate all `LayerVisibility` transforms against the actual XML:
/// - a layer must not appear in both show and hide
/// - every named layer must exist in the document
fn validate_layer_transforms(
    input_xml: &str,
    transforms: &[Transform],
) -> Result<(), Box<dyn std::error::Error>> {
    let layer_transforms: Vec<(&[String], &[String])> = transforms
        .iter()
        .filter_map(|t| match t {
            Transform::LayerVisibility { show, hide } => Some((show.as_slice(), hide.as_slice())),
            _ => None,
        })
        .collect();

    if layer_transforms.is_empty() {
        return Ok(());
    }

    // Check for names that appear in both show and hide within the same transform.
    for (show, hide) in &layer_transforms {
        for name in *show {
            if hide.contains(name) {
                return Err(format!(
                    "Layer '{}' appears in both show and hide lists",
                    name
                )
                .into());
            }
        }
    }

    // Collect all layer names referenced across all LayerVisibility transforms.
    let referenced: Vec<&str> = layer_transforms
        .iter()
        .flat_map(|(show, hide)| show.iter().chain(hide.iter()).map(|s| s.as_str()))
        .collect();

    if referenced.is_empty() {
        return Ok(());
    }

    // Scan the XML to collect actual layer names (mxCell with parent="0" and a value).
    let existing = collect_layer_names(input_xml)?;

    for name in referenced {
        if !existing.contains_key(name) {
            return Err(format!(
                "Layer '{}' does not exist in the document. Available layers: [{}]",
                name,
                existing.keys().cloned().collect::<Vec<_>>().join(", ")
            )
            .into());
        }
    }

    Ok(())
}

/// Return a map of layer_name → cell_id for every layer cell in the XML.
/// A layer cell is an `<mxCell parent="0" value="…">` element.
fn collect_layer_names(
    input_xml: &str,
) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    let mut reader = Reader::from_str(input_xml);
    reader.config_mut().trim_text(false);
    let mut layers = HashMap::new();

    loop {
        match reader.read_event()? {
            Event::Eof => break,
            Event::Start(ref elem) | Event::Empty(ref elem)
                if elem.name().as_ref() == b"mxCell" =>
            {
                let mut parent = None;
                let mut value = None;
                let mut id = None;
                for a in elem.attributes().filter_map(|a| a.ok()) {
                    match a.key.as_ref() {
                        b"parent" => parent = Some(String::from_utf8_lossy(&a.value).into_owned()),
                        b"value" => value = Some(String::from_utf8_lossy(&a.value).into_owned()),
                        b"id" => id = Some(String::from_utf8_lossy(&a.value).into_owned()),
                        _ => {}
                    }
                }
                if parent.as_deref() == Some("0") {
                    if let (Some(v), Some(i)) = (value, id) {
                        if !v.is_empty() {
                            layers.insert(v, i);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(layers)
}

/// For each `ColorEdges` transform, scan the XML and return the set of edge cell ids
/// whose source and target are both NOT in the exclude list.
/// Returns a map from color string to set of edge ids to recolor.
fn collect_edge_ids_to_recolor(
    input_xml: &str,
    transforms: &[Transform],
) -> Result<HashMap<String, HashSet<String>>, Box<dyn std::error::Error>> {
    let color_edge_transforms: Vec<(&Vec<String>, &str)> = transforms
        .iter()
        .filter_map(|t| match t {
            Transform::ColorEdges { exclude, color } => Some((exclude, color.as_str())),
            _ => None,
        })
        .collect();

    if color_edge_transforms.is_empty() {
        return Ok(HashMap::new());
    }

    let mut reader = Reader::from_str(input_xml);
    reader.config_mut().trim_text(false);
    // edge id -> (source, target)  — either may be absent
    let mut edges: Vec<(String, Option<String>, Option<String>)> = Vec::new();

    loop {
        match reader.read_event()? {
            Event::Eof => break,
            Event::Start(ref elem) | Event::Empty(ref elem)
                if elem.name().as_ref() == b"mxCell" =>
            {
                let mut is_edge = false;
                let mut id = None;
                let mut source = None;
                let mut target = None;
                for a in elem.attributes().filter_map(|a| a.ok()) {
                    match a.key.as_ref() {
                        b"edge" => is_edge = a.value.as_ref() == b"1",
                        b"id" => id = Some(String::from_utf8_lossy(&a.value).into_owned()),
                        b"source" => source = Some(String::from_utf8_lossy(&a.value).into_owned()),
                        b"target" => target = Some(String::from_utf8_lossy(&a.value).into_owned()),
                        _ => {}
                    }
                }
                if is_edge {
                    if let Some(i) = id {
                        edges.push((i, source, target));
                    }
                }
            }
            _ => {}
        }
    }

    let mut result: HashMap<String, HashSet<String>> = HashMap::new();

    for (exclude, color) in &color_edge_transforms {
        let ids: HashSet<String> = edges
            .iter()
            .filter(|(_, source, target)| {
                let src_excluded = source.as_deref().map(|s| exclude.iter().any(|e| e == s)).unwrap_or(false);
                let tgt_excluded = target.as_deref().map(|t| exclude.iter().any(|e| e == t)).unwrap_or(false);
                !src_excluded && !tgt_excluded
            })
            .map(|(id, _, _)| id.clone())
            .collect();
        result
            .entry(color.to_string())
            .or_default()
            .extend(ids);
    }

    Ok(result)
}

/// For one `<mxCell …>` element, apply Color and LayerVisibility transforms.
fn patch_cell(
    elem: BytesStart,
    transforms: &[Transform],
    edge_ids_to_recolor: &HashMap<String, HashSet<String>>,
) -> Result<BytesStart<'static>, Box<dyn std::error::Error>> {
    let mut attrs: Vec<(Vec<u8>, Vec<u8>)> = elem
        .attributes()
        .filter_map(|a| a.ok())
        .map(|a| (a.key.as_ref().to_vec(), a.value.to_vec()))
        .collect();

    let id = attrs
        .iter()
        .find(|(k, _)| k == b"id")
        .and_then(|(_, v)| String::from_utf8(v.clone()).ok())
        .unwrap_or_default();

    let parent = attrs
        .iter()
        .find(|(k, _)| k == b"parent")
        .and_then(|(_, v)| String::from_utf8(v.clone()).ok())
        .unwrap_or_default();

    let value = attrs
        .iter()
        .find(|(k, _)| k == b"value")
        .and_then(|(_, v)| String::from_utf8(v.clone()).ok())
        .unwrap_or_default();

    for t in transforms {
        match t {
            // --- Color ---
            Transform::Color { ids, color } if ids.contains(&id) => {
                patch_style_color(&mut attrs, color);
            }

            // --- LayerVisibility ---
            Transform::LayerVisibility { show, hide } if parent == "0" && !value.is_empty() => {
                let visible = if show.contains(&value) {
                    Some(b"1".as_slice())
                } else if hide.contains(&value) {
                    Some(b"0".as_slice())
                } else {
                    None
                };

                if let Some(vis_val) = visible {
                    // Update existing `visible` attr or append it.
                    let existing = attrs.iter_mut().find(|(k, _)| k == b"visible");
                    if let Some((_, v)) = existing {
                        *v = vis_val.to_vec();
                    } else {
                        attrs.push((b"visible".to_vec(), vis_val.to_vec()));
                    }
                }
            }

            // --- ColorEdges (pre-computed set) ---
            Transform::ColorEdges { color, .. } => {
                if let Some(ids) = edge_ids_to_recolor.get(color.as_str()) {
                    if ids.contains(&id) {
                        patch_style_color(&mut attrs, color);
                    }
                }
            }

            // --- ReplaceText ---
            Transform::ReplaceText { id: tid, text } if tid == &id => {
                for (key, val) in attrs.iter_mut() {
                    if key == b"value" {
                        *val = text.as_bytes().to_vec();
                    }
                }
            }

            _ => {}
        }
    }

    let name = String::from_utf8_lossy(elem.name().as_ref()).into_owned();
    let mut new_elem = BytesStart::new(name);
    for (k, v) in &attrs {
        new_elem.push_attribute((k.as_slice(), v.as_slice()));
    }
    Ok(new_elem)
}

/// Patch the `style` attribute in an attr list to set `fillColor` and `strokeColor`.
fn patch_style_color(attrs: &mut Vec<(Vec<u8>, Vec<u8>)>, color: &str) {
    for (key, val) in attrs.iter_mut() {
        if key == b"style" {
            let style = String::from_utf8_lossy(val).into_owned();
            let style = patch_fill_color(&style, color);
            let style = patch_stroke_color(&style, color);
            *val = style.into_bytes();
        }
    }
}

/// Replace (or append) the `fillColor` token inside a draw.io style string.
fn patch_fill_color(style: &str, color: &str) -> String {
    let token = "fillColor=";
    if let Some(start) = style.find(token) {
        let after = start + token.len();
        if let Some(semi) = style[after..].find(';') {
            let end = after + semi;
            format!("{}{}{}", &style[..after], color, &style[end..])
        } else {
            format!("{}{}", &style[..after], color)
        }
    } else {
        if style.ends_with(';') {
            format!("{}fillColor={};", style, color)
        } else {
            format!("{};fillColor={};", style, color)
        }
    }
}

/// Replace (or append) the `strokeColor` token inside a draw.io style string.
fn patch_stroke_color(style: &str, color: &str) -> String {
    let token = "strokeColor=";
    if let Some(start) = style.find(token) {
        let after = start + token.len();
        if let Some(semi) = style[after..].find(';') {
            let end = after + semi;
            format!("{}{}{}", &style[..after], color, &style[end..])
        } else {
            format!("{}{}", &style[..after], color)
        }
    } else {
        if style.ends_with(';') {
            format!("{}strokeColor={};", style, color)
        } else {
            format!("{};strokeColor={};", style, color)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Transform;

    #[test]
    fn replaces_existing_fill_color() {
        let style = "ellipse;fillColor=#e1d5e7;strokeColor=#9673a6;";
        let result = patch_fill_color(style, "#FF0000");
        assert_eq!(result, "ellipse;fillColor=#FF0000;strokeColor=#9673a6;");
    }

    #[test]
    fn appends_fill_color_when_absent() {
        let style = "ellipse;whiteSpace=wrap;";
        let result = patch_fill_color(style, "#00FF00");
        assert_eq!(result, "ellipse;whiteSpace=wrap;fillColor=#00FF00;");
    }

    #[test]
    fn fill_color_at_end_without_semicolon() {
        let style = "ellipse;fillColor=#e1d5e7";
        let result = patch_fill_color(style, "#0000FF");
        assert_eq!(result, "ellipse;fillColor=#0000FF");
    }

    const SAMPLE_XML: &str = r#"<mxfile><diagram><mxGraphModel><root>
        <mxCell id="0" />
        <mxCell id="1" parent="0" value="background" />
        <mxCell id="2" parent="0" value="foreground" />
        <mxCell id="3" parent="1" value="shape" vertex="1" />
    </root></mxGraphModel></diagram></mxfile>"#;

    #[test]
    fn layer_visibility_hides_layer() {
        let transforms = vec![Transform::LayerVisibility {
            show: vec![],
            hide: vec!["background".to_string()],
        }];
        let result = apply_transforms(SAMPLE_XML, &transforms).unwrap();
        assert!(result.contains(r#"value="background" visible="0""#) || result.contains(r#"visible="0""#));
        // foreground untouched — no visible attribute injected for it
        assert!(!result.contains(r#"value="foreground" visible="#));
    }

    #[test]
    fn layer_visibility_shows_layer() {
        let transforms = vec![Transform::LayerVisibility {
            show: vec!["foreground".to_string()],
            hide: vec![],
        }];
        let result = apply_transforms(SAMPLE_XML, &transforms).unwrap();
        assert!(result.contains(r#"visible="1""#));
    }

    #[test]
    fn layer_visibility_conflict_errors() {
        let transforms = vec![Transform::LayerVisibility {
            show: vec!["background".to_string()],
            hide: vec!["background".to_string()],
        }];
        let err = apply_transforms(SAMPLE_XML, &transforms).unwrap_err();
        assert!(err.to_string().contains("both show and hide"));
    }

    #[test]
    fn layer_visibility_unknown_layer_errors() {
        let transforms = vec![Transform::LayerVisibility {
            show: vec!["nonexistent".to_string()],
            hide: vec![],
        }];
        let err = apply_transforms(SAMPLE_XML, &transforms).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }
}
