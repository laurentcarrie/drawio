use printpdf::image_crate::GenericImageView;
use printpdf::*;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

/// Build a PDF with one page per slide.
/// Each PNG is placed on a page sized to match the image at 96 dpi.
pub fn build_pdf(png_paths: &[&Path], pdf_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if png_paths.is_empty() {
        return Err("no slides to export".into());
    }

    // Screen resolution assumed when none is embedded in the PNG.
    const DPI: f32 = 96.0;

    let px_to_mm = |px: u32| -> Mm { Mm((px as f32 * 25.4 / DPI) as f32) };

    // Helper: load a PNG and return (printpdf Image, width_mm, height_mm).
    let load = |path: &Path| -> Result<(Image, Mm, Mm), Box<dyn std::error::Error>> {
        let dyn_img = printpdf::image_crate::open(path)?;
        let (w_px, h_px) = dyn_img.dimensions();
        let img = Image::from_dynamic_image(&dyn_img);
        Ok((img, px_to_mm(w_px), px_to_mm(h_px)))
    };

    let (first_img, first_w, first_h) = load(png_paths[0])?;

    let (doc, first_page_idx, first_layer_idx) =
        PdfDocument::new("Slides", first_w, first_h, "Layer 1");

    let add_image = |img: Image, layer: PdfLayerReference| {
        img.add_to_layer(
            layer,
            ImageTransform {
                translate_x: Some(Mm(0.0)),
                translate_y: Some(Mm(0.0)),
                dpi: Some(DPI),
                ..Default::default()
            },
        );
    };

    add_image(first_img, doc.get_page(first_page_idx).get_layer(first_layer_idx));

    for path in &png_paths[1..] {
        let (img, w, h) = load(path)?;
        let (page_idx, layer_idx) = doc.add_page(w, h, "Layer 1");
        add_image(img, doc.get_page(page_idx).get_layer(layer_idx));
    }

    doc.save(&mut BufWriter::new(File::create(pdf_path)?))?;
    Ok(())
}
