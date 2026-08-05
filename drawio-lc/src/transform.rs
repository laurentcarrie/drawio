use std::{fs, io::Write, path::Path};

use quick_xml::{
    events::{BytesStart, Event},
    Reader, Writer,
};

use crate::model::{Derived, Transform};

/// Apply all transforms defined in `derived` to `input_xml` and write the
/// result to `derived.output`.
pub fn transform(input_xml: &str, derived: &Derived) -> Result<(), Box<dyn std::error::Error>> {
    let xml = apply_transforms(input_xml, &derived.transforms)?;

    let output_path = Path::new(&derived.output);
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let mut file = fs::File::create(output_path)?;
    file.write_all(xml.as_bytes())?;

    Ok(())
}

/// Walk the XML stream and apply every transform in order, returning the
/// modified XML string.
fn apply_transforms(
    input_xml: &str,
    transforms: &[Transform],
) -> Result<String, Box<dyn std::error::Error>> {
    let mut reader = Reader::from_str(input_xml);
    reader.config_mut().trim_text(false);

    let mut writer = Writer::new(Vec::new());

    loop {
        match reader.read_event()? {
            Event::Eof => break,
            Event::Start(elem) if elem.name().as_ref() == b"mxCell" => {
                let patched = patch_cell(elem, transforms);
                writer.write_event(Event::Start(patched))?;
            }
            Event::Empty(elem) if elem.name().as_ref() == b"mxCell" => {
                let patched = patch_cell(elem, transforms);
                writer.write_event(Event::Empty(patched))?;
            }
            other => {
                writer.write_event(other)?;
            }
        }
    }

    Ok(String::from_utf8(writer.into_inner())?)
}

/// For one `<mxCell …>` element, apply any Color transforms that target its id.
fn patch_cell(elem: BytesStart, transforms: &[Transform]) -> BytesStart<'static> {
    // Read the current id attribute so we know which transforms apply.
    let id = elem
        .attributes()
        .filter_map(|a| a.ok())
        .find(|a| a.key.as_ref() == b"id")
        .and_then(|a| String::from_utf8(a.value.to_vec()).ok())
        .unwrap_or_default();

    // Collect all Color transforms targeting this cell.
    let color_transforms: Vec<&str> = transforms
        .iter()
        .filter_map(|t| match t {
            Transform::Color { id: tid, color } if tid == &id => Some(color.as_str()),
            _ => None,
        })
        .collect();

    // Rebuild the element with an owned (static) name.
    let name = String::from_utf8_lossy(elem.name().as_ref()).into_owned();
    let mut new_elem = BytesStart::new(name);

    if color_transforms.is_empty() {
        // No transforms apply — copy attributes verbatim.
        for a in elem.attributes().filter_map(|a| a.ok()) {
            new_elem.push_attribute((a.key.as_ref(), a.value.as_ref()));
        }
        return new_elem;
    }

    // The last Color transform wins.
    let new_color = color_transforms.last().unwrap();

    // Rebuild the attribute list, patching `style`.
    for a in elem.attributes().filter_map(|a| a.ok()) {
        let key = a.key.as_ref().to_vec();
        let value: Vec<u8> = if key == b"style" {
            patch_fill_color(&String::from_utf8_lossy(&a.value), new_color).into_bytes()
        } else {
            a.value.to_vec()
        };
        new_elem.push_attribute((key.as_slice(), value.as_slice()));
    }
    new_elem
}

/// Replace (or append) the `fillColor` token inside a draw.io style string.
fn patch_fill_color(style: &str, color: &str) -> String {
    let token = "fillColor=";
    if let Some(start) = style.find(token) {
        let after = start + token.len();
        // The value ends at `;` or end-of-string.
        if let Some(semi) = style[after..].find(';') {
            let end = after + semi; // points at the ';'
            // Keep the existing semicolon; replace only the value.
            format!("{}{}{}", &style[..after], color, &style[end..])
        } else {
            // No trailing semicolon.
            format!("{}{}", &style[..after], color)
        }
    } else {
        // No fillColor present — append it.
        if style.ends_with(';') {
            format!("{}fillColor={};", style, color)
        } else {
            format!("{};fillColor={};", style, color)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::patch_fill_color;

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
}
