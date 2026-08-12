use std::{env, fs, path::Path};

use crate::model::ConfluenceConfig;

/// Push slides to Confluence Cloud:
/// 1. Resolve or create the target page.
/// 2. Upload every PNG as an attachment (update if already present).
/// 3. Upload the global MP4 as an attachment.
/// 4. Upload each section MP4 as an attachment.
/// 5. Replace the page body with a native Confluence gallery macro so the
///    slides are browsable without any HTML macro being enabled.
///
/// Auth: reads CONFLUENCE_USER (email) and CONFLUENCE_TOKEN (API token) from
/// the environment.
pub fn push_to_confluence(
    png_paths: &[&Path],
    mp4_path: &Path,
    section_mp4_paths: &[&Path],
    cfg: &ConfluenceConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let user = env::var("CONFLUENCE_USER").map_err(|_| {
        "CONFLUENCE_USER environment variable not set (your Atlassian account email)"
    })?;
    let token = env::var("CONFLUENCE_TOKEN")
        .map_err(|_| "CONFLUENCE_TOKEN environment variable not set (your API token)")?;

    let base = cfg.url.trim_end_matches('/');
    let agent = ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(
            native_tls::TlsConnector::builder()
                .build()?,
        ))
        .build();

    // ── 1. Resolve or create the page ────────────────────────────────────────
    check_space(&agent, base, &user, &token, &cfg.space_key)?;
    let page_id = resolve_or_create_page(&agent, base, &user, &token, cfg)?;
    println!("Confluence page id: {}", page_id);

    // ── 2. Upload attachments ─────────────────────────────────────────────────
    for path in png_paths {
        upload_attachment(&agent, base, &user, &token, &page_id, path, "image/png")?;
        println!(
            "Uploaded attachment: {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    upload_attachment(&agent, base, &user, &token, &page_id, mp4_path, "video/mp4")?;
    println!(
        "Uploaded attachment: {}",
        mp4_path.file_name().unwrap_or_default().to_string_lossy()
    );
    for path in section_mp4_paths {
        upload_attachment(&agent, base, &user, &token, &page_id, path, "video/mp4")?;
        println!(
            "Uploaded attachment: {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
    }

    // ── 3. Build page body with Confluence gallery macro ─────────────────────
    let filenames: Vec<String> = png_paths
        .iter()
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    let section_mp4_filenames: Vec<String> = section_mp4_paths
        .iter()
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    let body = build_page_body(&filenames, mp4_path, &section_mp4_filenames);

    update_page_body(&agent, base, &user, &token, &page_id, &cfg.page_title, &body)?;
    println!(
        "Confluence page updated: {}/wiki/spaces/{}/pages/{}",
        base, cfg.space_key, page_id
    );

    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn auth_header(user: &str, token: &str) -> String {
    let creds = format!("{}:{}", user, token);
    format!("Basic {}", base64_encode(creds.as_bytes()))
}

/// Turn a ureq error into a message that names the operation that failed and
/// carries Confluence's own explanation, which lives in the response body —
/// `ureq`'s Display impl shows only "<url>: status code NNN".
fn api_error(op: &str, err: ureq::Error) -> Box<dyn std::error::Error> {
    match err {
        ureq::Error::Status(code, resp) => {
            let url = resp.get_url().to_string();
            let body = resp.into_string().unwrap_or_default();
            let detail = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| {
                    v["message"]
                        .as_str()
                        .or_else(|| v["errors"][0]["title"].as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| body.chars().take(400).collect());
            let hint = match code {
                401 => "\n  hint: check CONFLUENCE_USER (your Atlassian account email) and \
                        CONFLUENCE_TOKEN (an API token from \
                        https://id.atlassian.com/manage-profile/security/api-tokens)",
                403 => "\n  hint: authenticated, but this account lacks permission on the space",
                404 => "\n  hint: a 404 here usually means the space or page does not exist, \
                        not that the URL is wrong",
                _ => "",
            };
            format!("{} failed (HTTP {} on {})\n  {}{}", op, code, url, detail, hint).into()
        }
        ureq::Error::Transport(t) => format!("{} failed: {}", op, t).into(),
    }
}

/// Confirm the configured space exists before touching any page. Confluence
/// answers a create-in-unknown-space with a bare 404 that names only the
/// generic `/wiki/rest/api/content` endpoint, so without this check a typo in
/// `space_key` is nearly undiagnosable.
fn check_space(
    agent: &ureq::Agent,
    base: &str,
    user: &str,
    token: &str,
    space_key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let auth = auth_header(user, token);
    let url = format!("{}/wiki/rest/api/space/{}", base, percent_encode(space_key));
    match agent
        .get(&url)
        .set("Authorization", &auth)
        .set("Accept", "application/json")
        .call()
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(404, _)) => {
            let mut msg = format!(
                "Confluence space key {:?} does not exist, or is not visible to {}.\n  \
                 Note that a personal space is keyed \"~<accountId>\" (e.g. \"~712020abc…\"), \
                 not by your username.",
                space_key, user
            );
            if let Some(mut spaces) = list_visible_spaces(agent, base, &auth) {
                // Surface likely matches first: with ~100 spaces, an alphabetical
                // list buries the one the user meant below the cut-off.
                let needle = space_key.trim_start_matches('~').to_lowercase();
                spaces.sort_by_key(|(key, name)| {
                    let hit = key.to_lowercase().contains(&needle)
                        || name.to_lowercase().contains(&needle);
                    !hit // false (0) sorts first, so hits lead
                });
                msg.push_str("\n  Spaces you can see:");
                for (key, name) in spaces.iter().take(25) {
                    msg.push_str(&format!("\n    {:<44} {}", key, name));
                }
                if spaces.len() > 25 {
                    msg.push_str(&format!("\n    … and {} more", spaces.len() - 25));
                }
            }
            Err(msg.into())
        }
        Err(e) => Err(api_error(&format!("Looking up space {:?}", space_key), e)),
    }
}

/// Best-effort list of (key, name) for every space the account can read; used
/// only to enrich the "unknown space" error, so failures here are swallowed.
fn list_visible_spaces(
    agent: &ureq::Agent,
    base: &str,
    auth: &str,
) -> Option<Vec<(String, String)>> {
    let url = format!("{}/wiki/rest/api/space?limit=200", base);
    let resp: serde_json::Value = agent
        .get(&url)
        .set("Authorization", auth)
        .set("Accept", "application/json")
        .call()
        .ok()?
        .into_json()
        .ok()?;
    let spaces: Vec<(String, String)> = resp["results"]
        .as_array()?
        .iter()
        .filter_map(|s| {
            Some((
                s["key"].as_str()?.to_string(),
                s["name"].as_str().unwrap_or("").to_string(),
            ))
        })
        .collect();
    (!spaces.is_empty()).then_some(spaces)
}

/// Find an existing page by title in the given space, or create a blank one.
/// Returns the page id as a String.
fn resolve_or_create_page(
    agent: &ureq::Agent,
    base: &str,
    user: &str,
    token: &str,
    cfg: &ConfluenceConfig,
) -> Result<String, Box<dyn std::error::Error>> {
    let auth = auth_header(user, token);

    // Search for an existing page.
    let search_url = format!(
        "{}/wiki/rest/api/content?title={}&spaceKey={}&expand=version",
        base,
        percent_encode(&cfg.page_title),
        percent_encode(&cfg.space_key)
    );
    let search_resp = agent
        .get(&search_url)
        .set("Authorization", &auth)
        .set("Accept", "application/json")
        .call();

    match search_resp {
        Ok(r) => {
            let resp: serde_json::Value = r.into_json()?;
            if let Some(id) = resp["results"][0]["id"].as_str() {
                return Ok(id.to_string());
            }
        }
        Err(ureq::Error::Status(404, _)) => {
            // Page not found — fall through to create it. The space itself was
            // already validated by check_space, so this really is about the page.
        }
        Err(e) => {
            return Err(api_error(
                &format!("Searching for page {:?}", cfg.page_title),
                e,
            ))
        }
    }

    // Page not found — create it.
    let create_url = format!("{}/wiki/rest/api/content", base);
    let body = serde_json::json!({
        "type": "page",
        "title": cfg.page_title,
        "space": { "key": cfg.space_key },
        "body": {
            "storage": {
                "value": "<p>Generating slides…</p>",
                "representation": "storage"
            }
        }
    });
    let resp: serde_json::Value = agent
        .post(&create_url)
        .set("Authorization", &auth)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .send_json(body)
        .map_err(|e| {
            api_error(
                &format!(
                    "Creating page {:?} in space {:?}",
                    cfg.page_title, cfg.space_key
                ),
                e,
            )
        })?
        .into_json()?;

    resp["id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Unexpected response creating page: {}", resp).into())
}

/// Upload `path` as an attachment on `page_id`, replacing any existing
/// attachment with the same filename.
fn upload_attachment(
    agent: &ureq::Agent,
    base: &str,
    user: &str,
    token: &str,
    page_id: &str,
    path: &Path,
    mime_type: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let auth = auth_header(user, token);
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let data = fs::read(path)?;

    // Check if attachment already exists.
    let list_url = format!(
        "{}/wiki/rest/api/content/{}/child/attachment?filename={}",
        base,
        page_id,
        percent_encode(&filename)
    );
    let existing: serde_json::Value = agent
        .get(&list_url)
        .set("Authorization", &auth)
        .set("Accept", "application/json")
        .call()
        .map_err(|e| api_error(&format!("Listing attachments of page {}", page_id), e))?
        .into_json()?;

    let url = if let Some(att_id) = existing["results"][0]["id"].as_str() {
        // Update existing attachment.
        format!(
            "{}/wiki/rest/api/content/{}/child/attachment/{}/data",
            base, page_id, att_id
        )
    } else {
        // Create new attachment.
        format!(
            "{}/wiki/rest/api/content/{}/child/attachment",
            base, page_id
        )
    };

    // Confluence attachment upload requires multipart/form-data.
    // Build a minimal multipart body manually to avoid pulling in a heavy dep.
    let boundary = "----ConfluenceUploadBoundary";
    let mut multipart: Vec<u8> = Vec::new();
    // file part
    multipart.extend_from_slice(
        format!(
            "--{}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
            boundary, filename, mime_type
        )
        .as_bytes(),
    );
    multipart.extend_from_slice(&data);
    multipart.extend_from_slice(format!("\r\n--{}--\r\n", boundary).as_bytes());

    agent
        .post(&url)
        .set("Authorization", &auth)
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={}", boundary),
        )
        .set("X-Atlassian-Token", "no-check")
        .send_bytes(&multipart)
        .map_err(|e| api_error(&format!("Uploading attachment {:?}", filename), e))?;

    Ok(())
}

/// Replace the page body with a Confluence storage-format document that shows
/// only the first slide, so the page does not read as a long vertical stack.
///
/// The remaining slides go inside a collapsed `expand` macro. That is not
/// decoration: Confluence's media viewer builds its Prev / Next collection from
/// the images in the page *content*, not from the page's attachments. A page
/// rendering a single image opens a viewer with no navigation at all (verified
/// against Confluence Cloud). Keeping the other slides in the content — merely
/// collapsed — gives the viewer its collection back while keeping them out of
/// sight.
///
/// The MP4 is embedded below using the `widget` macro so Confluence renders it
/// with its native video player (play/pause/seek controls).
fn build_page_body(filenames: &[String], mp4_path: &Path, section_mp4_filenames: &[String]) -> String {
    let mp4_filename = mp4_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    if filenames.is_empty() {
        return "<p>No slides were generated.</p>".to_string();
    };

    let mut body = format!(
        r#"<ac:structured-macro ac:name="expand">
  <ac:parameter ac:name="title">▶ Play animation ({} slides)</ac:parameter>
  <ac:rich-text-body>
<ac:structured-macro ac:name="multimedia">
  <ac:parameter ac:name="name"><ri:attachment ri:filename="{}" /></ac:parameter>
  <ac:parameter ac:name="autostart">false</ac:parameter>
</ac:structured-macro>
  </ac:rich-text-body>
</ac:structured-macro>"#,
        filenames.len(),
        mp4_filename,
    );

    // Section animations — one expand block per section MP4.
    for (i, section_mp4) in section_mp4_filenames.iter().enumerate() {
        body.push_str(&format!(
            r#"
<ac:structured-macro ac:name="expand">
  <ac:parameter ac:name="title">▶ Animation section {} </ac:parameter>
  <ac:rich-text-body>
<ac:structured-macro ac:name="multimedia">
  <ac:parameter ac:name="name"><ri:attachment ri:filename="{}" /></ac:parameter>
  <ac:parameter ac:name="autostart">false</ac:parameter>
</ac:structured-macro>
  </ac:rich-text-body>
</ac:structured-macro>"#,
            i + 1,
            section_mp4,
        ));
    }

    let all_images: String = filenames
        .iter()
        .map(|name| {
            format!(
                r#"<ac:image><ri:attachment ri:filename="{}" /></ac:image>"#,
                name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    body.push_str(&format!(
        r#"
<ac:structured-macro ac:name="expand">
  <ac:parameter ac:name="title">Browse slides ({} slides)</ac:parameter>
  <ac:rich-text-body>
{}
  </ac:rich-text-body>
</ac:structured-macro>"#,
        filenames.len(),
        all_images
    ));

    body
}

/// Fetch the current page version, then PUT an updated body.
fn update_page_body(
    agent: &ureq::Agent,
    base: &str,
    user: &str,
    token: &str,
    page_id: &str,
    title: &str,
    body: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let auth = auth_header(user, token);

    // Get current version number.
    let get_url = format!("{}/wiki/rest/api/content/{}?expand=version", base, page_id);
    let current: serde_json::Value = agent
        .get(&get_url)
        .set("Authorization", &auth)
        .set("Accept", "application/json")
        .call()
        .map_err(|e| api_error(&format!("Reading version of page {}", page_id), e))?
        .into_json()?;
    let version = current["version"]["number"].as_u64().ok_or_else(|| {
        format!(
            "Could not read version number of page {} from response: {}",
            page_id, current
        )
    })?;

    let update_url = format!("{}/wiki/rest/api/content/{}", base, page_id);
    let payload = serde_json::json!({
        "version": { "number": version + 1 },
        "title": title,
        "type": "page",
        "body": {
            "storage": {
                "value": body,
                "representation": "storage"
            }
        }
    });

    agent
        .request("PUT", &update_url)
        .set("Authorization", &auth)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .send_json(payload)
        .map_err(|e| api_error(&format!("Updating body of page {}", page_id), e))?;

    Ok(())
}

/// Minimal percent-encoder for URL query parameter values.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Minimal base64 encoder (mirrors the one in html_export to avoid a dep).
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
