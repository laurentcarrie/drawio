use std::{collections::{HashMap, HashSet}, fs, io::Write, path::Path, process::Command};

use image::GenericImageView;

use pulldown_cmark::{html, Options, Parser};
use quick_xml::{
    events::{BytesStart, Event},
    Reader, Writer,
};

use crate::model::{Derived, Transform};

/// Apply transforms to `input_xml`, write the resulting drawio XML to
/// `drawio_path` (a temp file), export it as a PNG to `png_path`, then
/// delete the temporary drawio file.
/// Returns the transformed XML so the caller can keep it in memory for
/// chained steps.
pub fn transform(
    input_xml: &str,
    derived: &Derived,
    drawio_path: &Path,
    png_path: &Path,
    ref_size: Option<(u32, u32)>,
    heading_margin_bottom: u32,
    list_item_spacing: u32,
    list_item_indent: u32,
    config_dir: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let xml = apply_transforms(input_xml, &derived.transforms, ref_size, heading_margin_bottom, list_item_spacing, list_item_indent, config_dir)?;

    if let Some(parent) = drawio_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let mut file = fs::File::create(drawio_path)?;
    file.write_all(xml.as_bytes())?;

    export_png(drawio_path, png_path, ref_size)?;

    fs::remove_file(drawio_path).unwrap_or_else(|e| {
        eprintln!(
            "Warning: could not remove temp file {}: {}",
            drawio_path.display(),
            e
        );
    });

    Ok(xml)
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
/// The PNG is written to `png_path`.
/// If `size` is provided, `--width` and `--height` are passed to fix the canvas size.
fn export_png(drawio_path: &Path, png_path: &Path, size: Option<(u32, u32)>) -> Result<(), Box<dyn std::error::Error>> {
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
    heading_margin_bottom: u32,
    list_item_spacing: u32,
    list_item_indent: u32,
    config_dir: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    // TitleSlide must be the only *effective* transform (Animation markers are ignored).
    let effective: Vec<&Transform> = transforms
        .iter()
        .filter(|t| !matches!(t, Transform::Animation))
        .collect();
    if effective.len() == 1 {
        if let Transform::TitleSlide { text } = effective[0] {
            let (w, h) = ref_size.ok_or(
                "TitleSlide requires a reference size — ensure the original file is exported first",
            )?;
            return Ok(build_title_slide_xml(text, w, h, heading_margin_bottom, list_item_spacing, list_item_indent));
        }
    }
    if effective.iter().any(|t| matches!(t, Transform::TitleSlide { .. })) {
        return Err("TitleSlide must be the only transform in the list".into());
    }

    // Validate ReplaceText/ElementVisibility/Color transforms: all referenced tags must exist.
    validate_replace_text_transforms(input_xml, transforms)?;

    // Validate ElementVisibility: no cell may be targeted by both show and hide.
    validate_element_visibility_conflicts(input_xml, transforms)?;

    // Build tag → ids map from UserObject / object elements.
    let tag_to_id = collect_tag_to_ids(input_xml)?;

    // Pre-compute the set of edge ids to recolor for each ColorEdges transform.
    let edge_ids_to_recolor = collect_edge_ids_to_recolor(input_xml, transforms)?;

    // Resolve all id-or-tag references in transforms to concrete cell ids.
    let resolved_transforms = resolve_transforms(transforms, &tag_to_id);

    let mut reader = Reader::from_str(input_xml);
    reader.config_mut().trim_text(false);

    let mut writer = Writer::new(Vec::new());

    // When inside a UserObject, this holds the UserObject's resolved cell id
    // so the inner mxCell (which has no id of its own) can be patched.
    let mut user_object_id: Option<String> = None;

    // When the last mxCell processed was an EmbedImage target, holds the
    // (width, height) to apply to the next mxGeometry child element.
    let mut pending_geometry: Option<(Option<f64>, Option<f64>)> = None;

    loop {
        match reader.read_event()? {
            Event::Eof => break,

            // UserObject / object start — patch label if ReplaceText targets it,
            // then record its id for the inner mxCell.
            Event::Start(ref elem) if is_cell_wrapper(elem.name().as_ref()) => {
                let id = elem.attributes().filter_map(|a| a.ok())
                    .find(|a| a.key.as_ref() == b"id")
                    .and_then(|a| String::from_utf8(a.value.to_vec()).ok());
                user_object_id = id.clone();
                let patched = patch_wrapper(elem.to_owned(), &resolved_transforms, heading_margin_bottom, list_item_spacing, list_item_indent, config_dir)?;
                writer.write_event(Event::Start(patched))?;
            }
            Event::End(ref elem) if is_cell_wrapper(elem.name().as_ref()) => {
                user_object_id = None;
                writer.write_event(Event::End(elem.to_owned()))?;
            }

            // mxCell — patch using its own id, or its parent UserObject's id.
            Event::Start(elem) if elem.name().as_ref() == b"mxCell" => {
                let (patched, geom) = patch_cell_with_geometry(elem, &resolved_transforms, &edge_ids_to_recolor, heading_margin_bottom, list_item_spacing, list_item_indent, user_object_id.as_deref(), config_dir)?;
                pending_geometry = geom;
                writer.write_event(Event::Start(patched))?;
            }
            Event::Empty(elem) if elem.name().as_ref() == b"mxCell" => {
                let (patched, geom) = patch_cell_with_geometry(elem, &resolved_transforms, &edge_ids_to_recolor, heading_margin_bottom, list_item_spacing, list_item_indent, user_object_id.as_deref(), config_dir)?;
                pending_geometry = geom;
                writer.write_event(Event::Empty(patched))?;
            }

            // mxGeometry — if inside an EmbedImage cell, update width/height.
            Event::Empty(elem) if elem.name().as_ref() == b"mxGeometry" => {
                if let Some((w, h)) = pending_geometry.take() {
                    let patched = patch_geometry(elem, w, h)?;
                    writer.write_event(Event::Empty(patched))?;
                } else {
                    writer.write_event(Event::Empty(elem))?;
                }
            }

            other => {
                writer.write_event(other)?;
            }
        }
    }

    Ok(String::from_utf8(writer.into_inner())?)
}

/// Convert Markdown text to an HTML string escaped for use inside an XML
/// attribute value (as drawio stores cell labels).
/// `heading_margin_bottom` sets the bottom margin (px) on h1–h6 tags, overriding
/// the browser default which creates a large gap below headings.
/// `list_item_spacing` sets the bottom margin (px) on li tags.
/// `list_item_indent` sets the number of non-breaking spaces prepended before
/// each bullet/number so items are indented from the left edge of the box.
fn markdown_to_xml_attr(text: &str, heading_margin_bottom: u32, list_item_spacing: u32, list_item_indent: u32) -> String {
    let mut html_output = String::new();
    let parser = Parser::new_ext(text, Options::all());
    html::push_html(&mut html_output, parser);

    let html_output = inject_html_spacing(&html_output, heading_margin_bottom, list_item_spacing, list_item_indent);

    html_output
        .trim()
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', "&#xa;")
        .replace('\r', "")
}

/// Inject explicit margin styles on headings, paragraphs and list items so
/// spacing is controlled rather than driven by browser defaults.
fn inject_html_spacing(html: &str, heading_margin_bottom: u32, list_item_spacing: u32, list_item_indent: u32) -> String {
    let mut out = html.to_string();
    for level in 1..=6 {
        let open = format!("<h{}>", level);
        let replacement = format!(
            r#"<h{} style="margin-top:0;margin-bottom:{}px">"#,
            level, heading_margin_bottom
        );
        out = out.replace(&open, &replacement);
    }
    // Zero out paragraph top-margins — browsers default <p> to ~1em top
    // margin which is the main cause of the visible gap after a heading.
    out = out.replace("<p>", r#"<p style="margin-top:0">"#);

    // draw.io's HTML renderer ignores all CSS margin/padding on <ul>/<li>.
    // The only reliable way to control inter-item spacing is to replace the
    // entire <ul>/<li> structure with plain bullet lines separated by
    // <font style="font-size:1px">.</font><br> spacer units. Each unit adds
    // ~10-12px of gap; list_item_spacing controls how many units are inserted.
    //
    // The dot '.' is rendered at 1px — essentially invisible in practice.
    let spacer_unit = r#"<font style="font-size:1px">.</font><br>"#;
    let between = spacer_unit.repeat(list_item_spacing as usize);

    // Extract all <li>…</li> content blocks, replace the whole <ul>…</ul>
    // with plain bullet lines joined by the spacer.
    let mut result = String::new();
    let mut remaining = out.as_str();
    while let Some(ul_start) = remaining.find("<ul>") {
        result.push_str(&remaining[..ul_start]);
        remaining = &remaining[ul_start + 4..]; // skip "<ul>"
        let ul_end = remaining.find("</ul>").unwrap_or(remaining.len());
        let ul_content = &remaining[..ul_end];
        remaining = if ul_end + 5 <= remaining.len() { &remaining[ul_end + 5..] } else { "" };

        // collect <li>…</li> items
        let mut items: Vec<&str> = Vec::new();
        let mut li_remaining = ul_content;
        while let Some(li_start) = li_remaining.find("<li>") {
            li_remaining = &li_remaining[li_start + 4..];
            let li_end = li_remaining.find("</li>").unwrap_or(li_remaining.len());
            items.push(&li_remaining[..li_end]);
            li_remaining = if li_end + 5 <= li_remaining.len() { &li_remaining[li_end + 5..] } else { "" };
        }

        let sep = format!("<br>{}", between);
        let indent = "&nbsp;".repeat(list_item_indent as usize);
        let bullets: Vec<String> = items.iter().map(|item| format!("{}• {}", indent, item)).collect();
        result.push_str(&bullets.join(&sep));
    }
    result.push_str(remaining);

    // Same for <ol> — use numbered items
    let mut out2 = String::new();
    let mut remaining2 = result.as_str();
    let mut ol_counter;
    while let Some(ol_start) = remaining2.find("<ol>") {
        out2.push_str(&remaining2[..ol_start]);
        remaining2 = &remaining2[ol_start + 4..];
        let ol_end = remaining2.find("</ol>").unwrap_or(remaining2.len());
        let ol_content = &remaining2[..ol_end];
        remaining2 = if ol_end + 5 <= remaining2.len() { &remaining2[ol_end + 5..] } else { "" };

        let mut items: Vec<&str> = Vec::new();
        let mut li_remaining = ol_content;
        while let Some(li_start) = li_remaining.find("<li>") {
            li_remaining = &li_remaining[li_start + 4..];
            let li_end = li_remaining.find("</li>").unwrap_or(li_remaining.len());
            items.push(&li_remaining[..li_end]);
            li_remaining = if li_end + 5 <= li_remaining.len() { &li_remaining[li_end + 5..] } else { "" };
        }

        let sep = format!("<br>{}", between);
        ol_counter = 1usize;
        let indent = "&nbsp;".repeat(list_item_indent as usize);
        let bullets: Vec<String> = items.iter().map(|item| {
            let s = format!("{}{}. {}", indent, ol_counter, item);
            ol_counter += 1;
            s
        }).collect();
        out2.push_str(&bullets.join(&sep));
    }
    out2.push_str(remaining2);

    out2
}

/// Return a copy of `transforms` with every id-or-tag reference resolved to
/// concrete cell ids using `tag_to_ids`. A tag may map to multiple ids; a
/// plain cell id (not found in the map) is kept as-is.
fn resolve_transforms(
    transforms: &[Transform],
    tag_to_ids: &HashMap<String, Vec<String>>,
) -> Vec<Transform> {
    // Expand one reference: tag → all its ids, or plain id → itself.
    let expand = |s: &String| -> Vec<String> {
        tag_to_ids.get(s).cloned().unwrap_or_else(|| vec![s.clone()])
    };
    // Expand a list of references, flattening tag → many ids.
    let expand_list = |refs: &Vec<String>| -> Vec<String> {
        refs.iter().flat_map(expand).collect()
    };
    transforms.iter().map(|t| match t {
        Transform::Color { tags, color } => Transform::Color {
            tags: expand_list(tags),
            color: color.clone(),
        },
        Transform::ColorEdges { exclude, color } => Transform::ColorEdges {
            exclude: expand_list(exclude),
            color: color.clone(),
        },
        Transform::ReplaceText { tag, text } => Transform::ReplaceText {
            // ReplaceText targets a single cell; use the first resolved id.
            tag: expand(tag).into_iter().next().unwrap_or_else(|| tag.clone()),
            text: text.clone(),
        },
        Transform::ImportMarkdown { tag, file } => Transform::ImportMarkdown {
            tag: expand(tag).into_iter().next().unwrap_or_else(|| tag.clone()),
            file: file.clone(),
        },
        Transform::ElementVisibility { show, hide } => Transform::ElementVisibility {
            show: expand_list(show),
            hide: expand_list(hide),
        },
        Transform::ArrowVisibility { tags, begin, end } => Transform::ArrowVisibility {
            tags: expand_list(tags),
            begin: *begin,
            end: *end,
        },
        Transform::ShapeAttributes { tags, shape, fill_color, stroke_color, text, font_size } => Transform::ShapeAttributes {
            tags: expand_list(tags),
            shape: shape.clone(),
            fill_color: fill_color.clone(),
            stroke_color: stroke_color.clone(),
            text: text.clone(),
            font_size: *font_size,
        },
        Transform::EdgeAttributes { tags, text, color, line_style, thickness, font_color, font_size, text_background, text_border_color, start_arrow, end_arrow } => Transform::EdgeAttributes {
            tags: expand_list(tags),
            text: text.clone(),
            color: color.clone(),
            line_style: line_style.clone(),
            thickness: *thickness,
            font_color: font_color.clone(),
            font_size: *font_size,
            text_background: text_background.clone(),
            text_border_color: text_border_color.clone(),
            start_arrow: start_arrow.clone(),
            end_arrow: end_arrow.clone(),
        },
        Transform::EmbedImage { tag, file, width, height } => Transform::EmbedImage {
            tag: expand(tag).into_iter().next().unwrap_or_else(|| tag.clone()),
            file: file.clone(),
            width: *width,
            height: *height,
        },
        // TitleSlide and Animation don't reference cell ids.
        other => other.clone(),
    }).collect()
}

/// Generate a minimal drawio XML containing only a centered text label,
/// sized to match the reference PNG dimensions.
/// `text` is interpreted as Markdown and converted to HTML.
/// If the text contains a list, the cell is left-aligned so bullet items
/// are not centered.
fn build_title_slide_xml(text: &str, width: u32, height: u32, heading_margin_bottom: u32, list_item_spacing: u32, list_item_indent: u32) -> String {
    let escaped = markdown_to_xml_attr(text, heading_margin_bottom, list_item_spacing, list_item_indent);

    // Use left-align when the content has list items so bullets are not centered.
    let has_list = text.lines().any(|l| {
        let t = l.trim();
        t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ")
            || t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && t.contains(". ")
    });
    let align = if has_list { "left" } else { "center" };

    // A white background rectangle gives drawio concrete bounds to export,
    // and a text cell on top renders the Markdown HTML centred.
    format!(
        r#"<mxfile host="drawio-lc"><diagram name="Title"><mxGraphModel><root><mxCell id="0"/><mxCell id="1" parent="0"/><mxCell id="bg" parent="1" vertex="1" style="rounded=0;whiteSpace=wrap;html=1;fillColor=#ffffff;strokeColor=none;"><mxGeometry x="0" y="0" width="{w}" height="{h}" as="geometry"/></mxCell><mxCell id="2" parent="1" value="{text}" vertex="1" style="text;html=1;align={align};verticalAlign=middle;fontSize=24;fontStyle=0;whiteSpace=wrap;"><mxGeometry x="0" y="0" width="{w}" height="{h}" as="geometry"/></mxCell></root></mxGraphModel></diagram></mxfile>"#,
        w = width,
        h = height,
        align = align,
        text = escaped,
    )
}

/// Returns true for XML element names that draw.io uses as cell wrappers
/// carrying `id` and `tags` attributes (`UserObject` and `object`).
#[inline]
fn is_cell_wrapper(name: &[u8]) -> bool {
    name == b"UserObject" || name == b"object"
}

/// Build a map from every draw.io tag → list of cell ids by scanning wrapper
/// elements (`UserObject` / `object`). The `tags` attribute is comma-separated;
/// each individual tag maps to all wrapper `id`s that carry it.
fn collect_tag_to_ids(
    input_xml: &str,
) -> Result<HashMap<String, Vec<String>>, Box<dyn std::error::Error>> {
    let mut reader = Reader::from_str(input_xml);
    reader.config_mut().trim_text(false);
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    loop {
        match reader.read_event()? {
            Event::Eof => break,
            Event::Start(ref elem) | Event::Empty(ref elem)
                if is_cell_wrapper(elem.name().as_ref()) =>
            {
                let mut id = None;
                let mut tags = None;
                for attr in elem.attributes().filter_map(|a| a.ok()) {
                    match attr.key.as_ref() {
                        b"id" => id = String::from_utf8(attr.value.to_vec()).ok(),
                        b"tags" => tags = String::from_utf8(attr.value.to_vec()).ok(),
                        _ => {}
                    }
                }
                if let (Some(id), Some(tags)) = (id, tags) {
                    for tag in tags.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        map.entry(tag.to_string()).or_default().push(id.clone());
                    }
                }
            }
            _ => {}
        }
    }
    Ok(map)
}

/// Validate all `ReplaceText` and `ElementVisibility` transforms: every referenced
/// tag must exist in the document.
fn validate_replace_text_transforms(
    input_xml: &str,
    transforms: &[Transform],
) -> Result<(), Box<dyn std::error::Error>> {
    let tags_to_check: Vec<(&str, &str)> = transforms
        .iter()
        .flat_map(|t| match t {
            Transform::ReplaceText { tag, .. } => vec![("ReplaceText", tag.as_str())],
            Transform::ImportMarkdown { tag, .. } => vec![("ImportMarkdown", tag.as_str())],
            Transform::ArrowVisibility { tags, .. } => tags
                .iter()
                .map(|tag| ("ArrowVisibility", tag.as_str()))
                .collect(),
            Transform::ShapeAttributes { tags, .. } => tags
                .iter()
                .map(|tag| ("ShapeAttributes", tag.as_str()))
                .collect(),
            Transform::EdgeAttributes { tags, .. } => tags
                .iter()
                .map(|tag| ("EdgeAttributes", tag.as_str()))
                .collect(),
            Transform::EmbedImage { tag, .. } => vec![("EmbedImage", tag.as_str())],
            Transform::ElementVisibility { show, hide } => show
                .iter()
                .map(|tag| ("ElementVisibility", tag.as_str()))
                .chain(hide.iter().map(|tag| ("ElementVisibility", tag.as_str())))
                .collect(),
            _ => vec![],
        })
        .collect();

    if tags_to_check.is_empty() {
        return Ok(());
    }

    // Collect all cell ids AND tags present in the document.
    let mut reader = Reader::from_str(input_xml);
    reader.config_mut().trim_text(false);
    let mut existing: HashSet<String> = HashSet::new();
    loop {
        match reader.read_event()? {
            Event::Eof => break,
            Event::Start(ref elem) | Event::Empty(ref elem)
                if elem.name().as_ref() == b"mxCell" =>
            {
                for attr in elem.attributes().filter_map(|a| a.ok()) {
                    if attr.key.as_ref() == b"id" {
                        if let Ok(val) = String::from_utf8(attr.value.to_vec()) {
                            existing.insert(val);
                        }
                    }
                }
            }
            Event::Start(ref elem) | Event::Empty(ref elem)
                if is_cell_wrapper(elem.name().as_ref()) =>
            {
                for attr in elem.attributes().filter_map(|a| a.ok()) {
                    match attr.key.as_ref() {
                        b"id" => {
                            if let Ok(val) = String::from_utf8(attr.value.to_vec()) {
                                existing.insert(val);
                            }
                        }
                        b"tags" => {
                            if let Ok(val) = String::from_utf8(attr.value.to_vec()) {
                                for tag in val.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                                    existing.insert(tag.to_string());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    for (transform_name, tag) in tags_to_check {
        if !existing.contains(tag) {
            return Err(format!(
                "{}: tag {:?} does not exist in the document. Available tags: [{}]",
                transform_name,
                tag,
                {
                    let mut sorted: Vec<&str> =
                        existing.iter().map(|s| s.as_str()).collect();
                    sorted.sort();
                    sorted.join(", ")
                }
            )
            .into());
        }
    }

    Ok(())
}

/// Validate `ElementVisibility` transforms: no cell may be targeted by both
/// a show tag and a hide tag. A cell with multiple draw.io tags (e.g. "A" and
/// "B") would receive contradictory instructions if "A" is in `show` and "B"
/// is in `hide` within the same transform.
fn validate_element_visibility_conflicts(
    input_xml: &str,
    transforms: &[Transform],
) -> Result<(), Box<dyn std::error::Error>> {
    let ev_transforms: Vec<(&[String], &[String])> = transforms
        .iter()
        .filter_map(|t| match t {
            Transform::ElementVisibility { show, hide } => {
                Some((show.as_slice(), hide.as_slice()))
            }
            _ => None,
        })
        .collect();

    if ev_transforms.is_empty() {
        return Ok(());
    }

    // Build a map from cell id → set of tags by scanning wrapper elements.
    let mut reader = Reader::from_str(input_xml);
    reader.config_mut().trim_text(false);
    // cell_id → tags
    let mut cell_tags: Vec<(String, Vec<String>)> = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Eof => break,
            Event::Start(ref elem) | Event::Empty(ref elem)
                if is_cell_wrapper(elem.name().as_ref()) =>
            {
                let mut id = None;
                let mut tags: Vec<String> = Vec::new();
                for attr in elem.attributes().filter_map(|a| a.ok()) {
                    match attr.key.as_ref() {
                        b"id" => id = String::from_utf8(attr.value.to_vec()).ok(),
                        b"tags" => {
                            if let Ok(val) = String::from_utf8(attr.value.to_vec()) {
                                tags = val
                                    .split(',')
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_string)
                                    .collect();
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(id) = id {
                    if tags.len() > 1 {
                        cell_tags.push((id, tags));
                    }
                }
            }
            _ => {}
        }
    }

    for (show, hide) in &ev_transforms {
        for (cell_id, tags) in &cell_tags {
            let shown = tags.iter().any(|t| show.contains(t));
            let hidden = tags.iter().any(|t| hide.contains(t));
            if shown && hidden {
                let show_tags: Vec<&str> = tags.iter().filter(|t| show.contains(t)).map(String::as_str).collect();
                let hide_tags: Vec<&str> = tags.iter().filter(|t| hide.contains(t)).map(String::as_str).collect();
                return Err(format!(
                    "ElementVisibility conflict: cell '{}' is targeted by both show (via tag(s): [{}]) and hide (via tag(s): [{}])",
                    cell_id,
                    show_tags.join(", "),
                    hide_tags.join(", "),
                )
                .into());
            }
        }
    }

    Ok(())
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

/// For one `<object …>` / `<UserObject …>` wrapper element, apply any
/// `ReplaceText` or `ImportMarkdown` transform that targets its id (replacing the `label` attr).
fn patch_wrapper(
    elem: BytesStart,
    transforms: &[Transform],
    heading_margin_bottom: u32,
    list_item_spacing: u32,
    list_item_indent: u32,
    config_dir: &Path,
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

    for t in transforms {
        let (matched_id, text) = match t {
            Transform::ReplaceText { tag, text } if tag == &id => {
                (true, text.clone())
            }
            Transform::ImportMarkdown { tag, file } if tag == &id => {
                let path = config_dir.join(file);
                let content = fs::read_to_string(&path).map_err(|e| {
                    format!("ImportMarkdown: could not read {:?}: {}", path, e)
                })?;
                (true, content)
            }
            _ => (false, String::new()),
        };
        if matched_id {
            let new_label = markdown_to_xml_attr(&text, heading_margin_bottom, list_item_spacing, list_item_indent);
            if let Some(entry) = attrs.iter_mut().find(|(k, _)| k == b"label") {
                entry.1 = new_label.into_bytes();
            } else {
                attrs.push((b"label".to_vec(), new_label.into_bytes()));
            }
        }
    }

    let mut out = BytesStart::new(String::from_utf8(elem.name().as_ref().to_vec())?);
    for (k, v) in attrs {
        out.push_attribute((k.as_slice(), v.as_slice()));
    }
    Ok(out)
}

/// For one `<mxCell …>` element, apply Color, ColorEdges, ElementVisibility,
/// ReplaceText, ImportMarkdown, ShapeAttributes, EdgeAttributes, EmbedImage,
/// and ArrowVisibility transforms.
/// Returns the patched element and, for EmbedImage targets, the (width, height)
/// to apply to the following mxGeometry child.
fn patch_cell_with_geometry(
    elem: BytesStart,
    transforms: &[Transform],
    edge_ids_to_recolor: &HashMap<String, HashSet<String>>,
    heading_margin_bottom: u32,
    list_item_spacing: u32,
    list_item_indent: u32,
    user_object_id: Option<&str>,
    config_dir: &Path,
) -> Result<(BytesStart<'static>, Option<(Option<f64>, Option<f64>)>), Box<dyn std::error::Error>> {
    let mut attrs: Vec<(Vec<u8>, Vec<u8>)> = elem
        .attributes()
        .filter_map(|a| a.ok())
        .map(|a| (a.key.as_ref().to_vec(), a.value.to_vec()))
        .collect();

    // Use the mxCell's own id if present; fall back to the parent UserObject's id.
    let id = attrs
        .iter()
        .find(|(k, _)| k == b"id")
        .and_then(|(_, v)| String::from_utf8(v.clone()).ok())
        .or_else(|| user_object_id.map(str::to_string))
        .unwrap_or_default();

    for t in transforms {
        match t {
            // --- Color ---
            Transform::Color { tags, color } if tags.contains(&id) => {
                patch_style_color(&mut attrs, color);
            }

            // --- ColorEdges (pre-computed set) ---
            Transform::ColorEdges { color, .. } => {
                if let Some(ids) = edge_ids_to_recolor.get(color.as_str()) {
                    if ids.contains(&id) {
                        patch_style_color(&mut attrs, color);
                    }
                }
            }

            // --- ElementVisibility ---
            Transform::ElementVisibility { show, hide } => {
                if show.contains(&id) {
                    patch_visibility_visible(&mut attrs);
                } else if hide.contains(&id) {
                    patch_visibility_hidden(&mut attrs);
                }
            }

            // --- ReplaceText ---
            Transform::ReplaceText { tag, text } if tag == &id => {
                let formatted = markdown_to_xml_attr(text, heading_margin_bottom, list_item_spacing, list_item_indent);
                for (key, val) in attrs.iter_mut() {
                    if key == b"value" {
                        *val = formatted.as_bytes().to_vec();
                    }
                    // Remove overflow=hidden so the replaced text is not clipped
                    // when it is taller than the original cell bounds.
                    if key == b"style" {
                        if let Ok(s) = std::str::from_utf8(val) {
                            let patched = s
                                .split(';')
                                .filter(|part| {
                                    part.trim() != "overflow=hidden"
                                })
                                .collect::<Vec<_>>()
                                .join(";");
                            *val = patched.as_bytes().to_vec();
                        }
                    }
                }
            }

            // --- ImportMarkdown ---
            Transform::ImportMarkdown { tag, file } if tag == &id => {
                let path = config_dir.join(file);
                let text = fs::read_to_string(&path).map_err(|e| {
                    format!("ImportMarkdown: could not read {:?}: {}", path, e)
                })?;
                let formatted = markdown_to_xml_attr(&text, heading_margin_bottom, list_item_spacing, list_item_indent);
                for (key, val) in attrs.iter_mut() {
                    if key == b"value" {
                        *val = formatted.as_bytes().to_vec();
                    }
                    if key == b"style" {
                        if let Ok(s) = std::str::from_utf8(val) {
                            let patched = s
                                .split(';')
                                .filter(|part| part.trim() != "overflow=hidden")
                                .collect::<Vec<_>>()
                                .join(";");
                            *val = patched.as_bytes().to_vec();
                        }
                    }
                }
            }

            // --- ArrowVisibility ---
            Transform::ArrowVisibility { tags, begin, end } if tags.contains(&id) => {
                patch_arrow_visibility(&mut attrs, *begin, *end);
            }

            // --- ShapeAttributes ---
            Transform::ShapeAttributes { tags, shape, fill_color, stroke_color, text, font_size }
                if tags.contains(&id) =>
            {
                if let Some(s) = shape {
                    patch_style_token_mut(&mut attrs, "shape", &format!("mxgraph.basic.{}", s));
                }
                if let Some(c) = fill_color {
                    patch_style_token_in_attrs(&mut attrs, "fillColor", c);
                }
                if let Some(c) = stroke_color {
                    patch_style_token_in_attrs(&mut attrs, "strokeColor", c);
                }
                if let Some(fs) = font_size {
                    patch_style_token_in_attrs(&mut attrs, "fontSize", &fs.to_string());
                }
                if let Some(t) = text {
                    let formatted = markdown_to_xml_attr(t, heading_margin_bottom, list_item_spacing, list_item_indent);
                    // mxCell stores label in `value`; wrapper elements use `label`.
                    for (key, val) in attrs.iter_mut() {
                        if key == b"value" || key == b"label" {
                            *val = formatted.clone().into_bytes();
                        }
                    }
                }
            }

            // --- EdgeAttributes ---
            Transform::EdgeAttributes {
                tags, text, color, line_style, thickness,
                font_color, font_size, text_background, text_border_color,
                start_arrow, end_arrow,
            } if tags.contains(&id) => {
                if let Some(c) = color {
                    patch_style_token_in_attrs(&mut attrs, "strokeColor", c);
                }
                if let Some(ls) = line_style {
                    match ls.as_str() {
                        "dashed" => {
                            patch_style_token_in_attrs(&mut attrs, "dashed", "1");
                        }
                        "dotted" => {
                            patch_style_token_in_attrs(&mut attrs, "dashed", "1");
                            patch_style_token_in_attrs(&mut attrs, "dashPattern", "1 4");
                        }
                        "solid" => {
                            patch_style_token_in_attrs(&mut attrs, "dashed", "0");
                        }
                        _ => {}
                    }
                }
                if let Some(t) = thickness {
                    patch_style_token_in_attrs(&mut attrs, "strokeWidth", &t.to_string());
                }
                if let Some(c) = font_color {
                    patch_style_token_in_attrs(&mut attrs, "fontColor", c);
                    // Also update the XML-level `fontColor` attribute if present.
                    for (key, val) in attrs.iter_mut() {
                        if key == b"fontColor" {
                            *val = c.as_bytes().to_vec();
                        }
                    }
                }
                if let Some(fs) = font_size {
                    patch_style_token_in_attrs(&mut attrs, "fontSize", &fs.to_string());
                }
                if let Some(bg) = text_background {
                    patch_style_token_in_attrs(&mut attrs, "labelBackgroundColor", bg);
                }
                if let Some(bc) = text_border_color {
                    patch_style_token_in_attrs(&mut attrs, "labelBorderColor", bc);
                }
                if let Some(sa) = start_arrow {
                    patch_style_token_in_attrs(&mut attrs, "startArrow", sa);
                }
                if let Some(ea) = end_arrow {
                    patch_style_token_in_attrs(&mut attrs, "endArrow", ea);
                }
                if let Some(t) = text {
                    let formatted = markdown_to_xml_attr(t, heading_margin_bottom, list_item_spacing, list_item_indent);
                    for (key, val) in attrs.iter_mut() {
                        if key == b"value" || key == b"label" {
                            *val = formatted.clone().into_bytes();
                        }
                    }
                }
            }

            _ => {}
        }
    }

    // Handle EmbedImage: update style to a data URI and collect geometry changes.
    let mut pending_geom: Option<(Option<f64>, Option<f64>)> = None;
    for t in transforms {
        if let Transform::EmbedImage { tag, file, width, height } = t {
            if tag == &id {
                let path = config_dir.join(file);
                let img_bytes = fs::read(&path).map_err(|e| {
                    format!("EmbedImage: could not read {:?}: {}", path, e)
                })?;
                use base64::Engine;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&img_bytes);
                let mime = if file.ends_with(".png") { "image/png" }
                    else if file.ends_with(".jpg") || file.ends_with(".jpeg") { "image/jpeg" }
                    else { "image/png" };
                let data_uri = format!("data:{};base64,{}", mime, b64);
                // Replace the style with an image style using the data URI.
                let new_style = format!("shape=image;verticalLabelPosition=bottom;labelBackgroundColor=#ffffff;verticalAlign=top;align=center;strokeColor=none;fillColor=none;image;image={};", data_uri);
                if let Some((_, val)) = attrs.iter_mut().find(|(k, _)| k == b"style") {
                    *val = new_style.into_bytes();
                } else {
                    attrs.push((b"style".to_vec(), new_style.into_bytes()));
                }
                // Determine final width/height by loading actual image dimensions.
                let actual = image::load_from_memory(&img_bytes).ok().map(|i| i.dimensions());
                let (w_out, h_out) = match (*width, *height) {
                    (Some(w), Some(h)) => (Some(w), Some(h)),
                    (Some(w), None) => {
                        let h = actual.map(|(aw, ah)| w * ah as f64 / aw as f64);
                        (Some(w), h)
                    }
                    (None, Some(h)) => {
                        let w = actual.map(|(aw, ah)| h * aw as f64 / ah as f64);
                        (w, Some(h))
                    }
                    (None, None) => (
                        actual.map(|(w, _)| w as f64),
                        actual.map(|(_, h)| h as f64),
                    ),
                };
                pending_geom = Some((w_out, h_out));
                break;
            }
        }
    }

    let name = String::from_utf8_lossy(elem.name().as_ref()).into_owned();
    let mut new_elem = BytesStart::new(name);
    for (k, v) in &attrs {
        new_elem.push_attribute((k.as_slice(), v.as_slice()));
    }
    Ok((new_elem, pending_geom))
}

/// Patch the `width` and/or `height` attributes of an `mxGeometry` element.
fn patch_geometry(
    elem: BytesStart,
    width: Option<f64>,
    height: Option<f64>,
) -> Result<BytesStart<'static>, Box<dyn std::error::Error>> {
    let mut attrs: Vec<(Vec<u8>, Vec<u8>)> = elem
        .attributes()
        .filter_map(|a| a.ok())
        .map(|a| (a.key.as_ref().to_vec(), a.value.to_vec()))
        .collect();
    if let Some(w) = width {
        if let Some(entry) = attrs.iter_mut().find(|(k, _)| k == b"width") {
            entry.1 = format!("{}", w).into_bytes();
        }
    }
    if let Some(h) = height {
        if let Some(entry) = attrs.iter_mut().find(|(k, _)| k == b"height") {
            entry.1 = format!("{}", h).into_bytes();
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

/// Set `visible="0"` on the cell — the draw.io-native way to hide an element.
fn patch_visibility_hidden(attrs: &mut Vec<(Vec<u8>, Vec<u8>)>) {
    let existing = attrs.iter_mut().find(|(k, _)| k == b"visible");
    if let Some((_, val)) = existing {
        *val = b"0".to_vec();
    } else {
        attrs.push((b"visible".to_vec(), b"0".to_vec()));
    }
}

/// Set `visible="1"` on the cell — the draw.io-native way to show an element.
fn patch_visibility_visible(attrs: &mut Vec<(Vec<u8>, Vec<u8>)>) {
    let existing = attrs.iter_mut().find(|(k, _)| k == b"visible");
    if let Some((_, val)) = existing {
        *val = b"1".to_vec();
    } else {
        attrs.push((b"visible".to_vec(), b"1".to_vec()));
    }
}

/// Patch `startArrow` / `endArrow` in the style attribute of an edge cell.
/// `begin = Some(false)` sets `startArrow=none`; `begin = Some(true)` restores
/// `startArrow=classic`.  `None` leaves the token unchanged.
/// Same logic applies to `end` / `endArrow`.
fn patch_arrow_visibility(attrs: &mut Vec<(Vec<u8>, Vec<u8>)>, begin: Option<bool>, end: Option<bool>) {
    for (key, val) in attrs.iter_mut() {
        if key == b"style" {
            let style = String::from_utf8_lossy(val).into_owned();
            let style = match begin {
                Some(true)  => patch_style_token(&style, "startArrow", "classic"),
                Some(false) => patch_style_token(&style, "startArrow", "none"),
                None        => style,
            };
            let style = match end {
                Some(true)  => patch_style_token(&style, "endArrow", "classic"),
                Some(false) => patch_style_token(&style, "endArrow", "none"),
                None        => style,
            };
            *val = style.into_bytes();
        }
    }
}

/// Replace (or append) a `key=value` token inside a draw.io style string.
/// If the token already exists its value is replaced; otherwise it is appended.
fn patch_style_token(style: &str, key: &str, value: &str) -> String {
    let token = format!("{}=", key);
    if let Some(start) = style.find(&token) {
        let after = start + token.len();
        let end = style[after..].find(';').map(|i| after + i).unwrap_or(style.len());
        format!("{}{}{}", &style[..after], value, &style[end..])
    } else if style.ends_with(';') {
        format!("{}{}={};", style, key, value)
    } else {
        format!("{};{}={};", style, key, value)
    }
}

/// Replace (or append) a style token, working directly on the attrs list.
fn patch_style_token_in_attrs(attrs: &mut Vec<(Vec<u8>, Vec<u8>)>, key: &str, value: &str) {
    for (k, v) in attrs.iter_mut() {
        if k == b"style" {
            let style = String::from_utf8_lossy(v).into_owned();
            *v = patch_style_token(&style, key, value).into_bytes();
            return;
        }
    }
    // No `style` attribute yet — create one.
    attrs.push((b"style".to_vec(), format!("{}={};", key, value).into_bytes()));
}

/// Patch a style token using the mutable attrs form (alias kept for clarity).
#[inline]
fn patch_style_token_mut(attrs: &mut Vec<(Vec<u8>, Vec<u8>)>, key: &str, value: &str) {
    patch_style_token_in_attrs(attrs, key, value);
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
    use std::path::Path;

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

    /// XML that uses `object` wrappers with draw.io tags (as real draw.io files do).
    const TAGGED_XML: &str = r#"<mxfile><diagram><mxGraphModel><root>
        <mxCell id="0" />
        <mxCell id="1" parent="0" value="Layer 1" />
        <object id="cell-a" label="Box A" tags="highlight,important">
            <mxCell parent="1" vertex="1" style="fillColor=#ffffff;" />
        </object>
        <object id="cell-b" label="Box B" tags="secondary">
            <mxCell parent="1" vertex="1" style="fillColor=#ffffff;" />
        </object>
    </root></mxGraphModel></diagram></mxfile>"#;

    #[test]
    fn heading_margin_is_injected() {
        let html = markdown_to_xml_attr("# Title\n\nsome text", 6, 0, 0);
        assert!(html.contains("margin-bottom:6px"), "got: {}", html);
    }

    #[test]
    fn color_transform_by_tag_patches_fill() {
        let transforms = vec![Transform::Color {
            tags: vec!["highlight".to_string()],
            color: "#FF0000".to_string(),
        }];
        let result = apply_transforms(TAGGED_XML, &transforms, None, 4, 0, 0, Path::new(".")).unwrap();
        // cell-a has tag "highlight" — its mxCell style should be patched
        assert!(
            result.contains("fillColor=#FF0000"),
            "expected fillColor=#FF0000 in: {result}"
        );
        // cell-b does not have "highlight" — should remain white
        assert!(
            result.contains("fillColor=#ffffff"),
            "expected cell-b fillColor unchanged in: {result}"
        );
    }

    #[test]
    fn hide_elements_by_tag() {
        let transforms = vec![Transform::ElementVisibility {
            show: vec![],
            hide: vec!["secondary".to_string()],
        }];
        let result = apply_transforms(TAGGED_XML, &transforms, None, 4, 0, 0, Path::new(".")).unwrap();
        // cell-b has tag "secondary" — should be hidden
        assert!(
            result.contains(r#"visible="0""#),
            "expected visible=0 in: {result}"
        );
    }

    #[test]
    fn hide_elements_unknown_tag_errors() {
        let transforms = vec![Transform::ElementVisibility {
            show: vec![],
            hide: vec!["nonexistent-tag".to_string()],
        }];
        let err = apply_transforms(TAGGED_XML, &transforms, None, 4, 0, 0, Path::new(".")).unwrap_err();
        assert!(
            err.to_string().contains("nonexistent-tag"),
            "expected error mentioning the unknown tag, got: {err}"
        );
    }

    /// XML where one cell carries two tags so we can test show/hide conflicts.
    const MULTI_TAG_XML: &str = r#"<mxfile><diagram><mxGraphModel><root>
        <mxCell id="0" />
        <mxCell id="1" parent="0" value="Layer 1" />
        <object id="cell-ab" label="Box AB" tags="A,B">
            <mxCell parent="1" vertex="1" style="fillColor=#ffffff;" />
        </object>
    </root></mxGraphModel></diagram></mxfile>"#;

    #[test]
    fn element_visibility_conflict_errors() {
        let transforms = vec![Transform::ElementVisibility {
            show: vec!["A".to_string()],
            hide: vec!["B".to_string()],
        }];
        let err = apply_transforms(MULTI_TAG_XML, &transforms, None, 4, 0, 0, Path::new(".")).unwrap_err();
        assert!(
            err.to_string().contains("conflict"),
            "expected conflict error, got: {err}"
        );
        assert!(err.to_string().contains("cell-ab"), "expected cell id in error, got: {err}");
    }

    #[test]
    fn animation_is_noop() {
        // Animation between two Color transforms should produce the same result
        // as the two Color transforms without Animation.
        let transforms_with = vec![
            Transform::Color { tags: vec!["highlight".to_string()], color: "#FF0000".to_string() },
            Transform::Animation,
            Transform::Color { tags: vec!["secondary".to_string()], color: "#00FF00".to_string() },
        ];
        let transforms_without = vec![
            Transform::Color { tags: vec!["highlight".to_string()], color: "#FF0000".to_string() },
            Transform::Color { tags: vec!["secondary".to_string()], color: "#00FF00".to_string() },
        ];
        let with_anim = apply_transforms(TAGGED_XML, &transforms_with, None, 4, 0, 0, Path::new(".")).unwrap();
        let without_anim = apply_transforms(TAGGED_XML, &transforms_without, None, 4, 0, 0, Path::new(".")).unwrap();
        assert_eq!(with_anim, without_anim, "Animation should not affect the XML output");
    }

    #[test]
    fn title_slide_with_animation_accepted() {
        // TitleSlide + Animation markers should be accepted (Animation is ignored
        // for the "must be the only transform" validation).
        let transforms = vec![
            Transform::Animation,
            Transform::TitleSlide { text: "# Hello".to_string() },
            Transform::Animation,
        ];
        let result = apply_transforms(TAGGED_XML, &transforms, Some((800, 600)), 4, 0, 0, Path::new("."));
        assert!(result.is_ok(), "TitleSlide + Animation should be accepted, got: {:?}", result);
    }

    /// XML with an edge cell that has startArrow and endArrow in its style.
    const EDGE_XML: &str = r#"<mxfile><diagram><mxGraphModel><root>
        <mxCell id="0" />
        <mxCell id="1" parent="0" />
        <object id="edge-a" label="" tags="myedge">
            <mxCell parent="1" edge="1" style="endArrow=classic;startArrow=classic;html=1;" />
        </object>
    </root></mxGraphModel></diagram></mxfile>"#;

    #[test]
    fn arrow_visibility_hide_end() {
        let transforms = vec![Transform::ArrowVisibility {
            tags: vec!["myedge".to_string()],
            begin: None,
            end: Some(false),
        }];
        let result = apply_transforms(EDGE_XML, &transforms, None, 4, 0, 0, Path::new(".")).unwrap();
        assert!(result.contains("endArrow=none"), "endArrow should be none, got: {result}");
        assert!(result.contains("startArrow=classic"), "startArrow should be unchanged, got: {result}");
    }

    #[test]
    fn arrow_visibility_hide_begin() {
        let transforms = vec![Transform::ArrowVisibility {
            tags: vec!["myedge".to_string()],
            begin: Some(false),
            end: None,
        }];
        let result = apply_transforms(EDGE_XML, &transforms, None, 4, 0, 0, Path::new(".")).unwrap();
        assert!(result.contains("startArrow=none"), "startArrow should be none, got: {result}");
        assert!(result.contains("endArrow=classic"), "endArrow should be unchanged, got: {result}");
    }

    #[test]
    fn arrow_visibility_show_both() {
        // Start from a state where both arrows are hidden, then show both.
        let hide = vec![Transform::ArrowVisibility {
            tags: vec!["myedge".to_string()],
            begin: Some(false),
            end: Some(false),
        }];
        let hidden = apply_transforms(EDGE_XML, &hide, None, 4, 0, 0, Path::new(".")).unwrap();
        let show = vec![Transform::ArrowVisibility {
            tags: vec!["myedge".to_string()],
            begin: Some(true),
            end: Some(true),
        }];
        let shown = apply_transforms(&hidden, &show, None, 4, 0, 0, Path::new(".")).unwrap();
        assert!(shown.contains("startArrow=classic"), "startArrow should be classic, got: {shown}");
        assert!(shown.contains("endArrow=classic"), "endArrow should be classic, got: {shown}");
    }

    #[test]
    fn patch_style_token_replaces_existing() {
        let style = "endArrow=classic;startArrow=classic;html=1;";
        let result = patch_style_token(style, "endArrow", "none");
        assert_eq!(result, "endArrow=none;startArrow=classic;html=1;");
    }

    #[test]
    fn patch_style_token_appends_when_absent() {
        let style = "html=1;rounded=0;";
        let result = patch_style_token(style, "startArrow", "none");
        assert_eq!(result, "html=1;rounded=0;startArrow=none;");
    }
}
