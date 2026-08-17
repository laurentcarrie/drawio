use std::{fs, io::BufWriter, path::Path};

use printpdf::{Image, ImageTransform, Mm, PdfDocument};

/// Build a PDF where every PNG in `png_paths` becomes one page.
/// Each page is sized to the PNG's pixel dimensions converted to mm at 96 dpi,
/// and the image is placed to fill the whole page.
pub fn build_pdf(
    png_paths: &[&Path],
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if png_paths.is_empty() {
        return Err("No PNG frames provided for PDF export".into());
    }

    // Read the first image to get the reference dimensions for the document.
    let first_bytes = fs::read(png_paths[0])
        .map_err(|e| format!("Failed to read {}: {}", png_paths[0].display(), e))?;
    let (ref_w_px, ref_h_px) = png_dimensions(&first_bytes)?;

    // Convert pixel dimensions to mm at 96 dpi.
    let px_to_mm = |px: u32| -> f32 { px as f32 / 96.0 * 25.4 };
    let page_w_mm = px_to_mm(ref_w_px);
    let page_h_mm = px_to_mm(ref_h_px);

    let (doc, first_page, first_layer) = PdfDocument::new(
        "Slides",
        Mm(page_w_mm),
        Mm(page_h_mm),
        "Layer 1",
    );

    for (i, png_path) in png_paths.iter().enumerate() {
        let layer = if i == 0 {
            doc.get_page(first_page).get_layer(first_layer)
        } else {
            let (page_idx, layer_idx) =
                doc.add_page(Mm(page_w_mm), Mm(page_h_mm), "Layer 1");
            doc.get_page(page_idx).get_layer(layer_idx)
        };

        let bytes = fs::read(png_path)
            .map_err(|e| format!("Failed to read {}: {}", png_path.display(), e))?;

        let (w_px, h_px) = png_dimensions(&bytes)?;

        // printpdf places images at 300 dpi by default.
        // Scale factors to fill the page exactly: page_mm / (px / 300 * 25.4).
        let img_w_mm_at_300dpi = w_px as f32 / 300.0 * 25.4;
        let img_h_mm_at_300dpi = h_px as f32 / 300.0 * 25.4;
        let scale_x = page_w_mm / img_w_mm_at_300dpi;
        let scale_y = page_h_mm / img_h_mm_at_300dpi;

        // Use printpdf's bundled image crate to decode the PNG.
        let mut cursor = std::io::Cursor::new(&bytes);
        let decoder = printpdf::image_crate::codecs::png::PngDecoder::new(&mut cursor)?;
        let image = Image::try_from(decoder)?;

        image.add_to_layer(
            layer,
            ImageTransform {
                translate_x: Some(Mm(0.0)),
                translate_y: Some(Mm(0.0)),
                scale_x: Some(scale_x),
                scale_y: Some(scale_y),
                rotate: None,
                dpi: Some(300.0),
            },
        );
    }

    let file = fs::File::create(output_path)?;
    doc.save(&mut BufWriter::new(file))?;
    Ok(())
}

/// Extract width and height in pixels from raw PNG bytes without a full decode.
fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    // PNG header: 8-byte signature + IHDR chunk (4 len + 4 "IHDR" + 4 w + 4 h + ...)
    if bytes.len() < 24 {
        return Err("PNG file too short".into());
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Ok((w, h))
}
