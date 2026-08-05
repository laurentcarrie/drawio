use std::{fs::File, path::Path};

use gif::{Encoder, Frame, Repeat};
use image::{imageops::FilterType, DynamicImage, GenericImageView};

/// Build an animated GIF from a list of PNG paths.
/// The GIF is written to `output_path`.
/// Each frame is displayed for `delay_ms` milliseconds (rounded to centiseconds).
pub fn build_animated_gif(
    png_paths: &[&Path],
    output_path: &Path,
    delay_ms: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    if png_paths.is_empty() {
        return Err("No PNG frames provided for GIF".into());
    }

    // Load all images first so we can determine the common canvas size.
    let images: Vec<DynamicImage> = png_paths
        .iter()
        .map(|p| image::open(p).map_err(|e| format!("Failed to load {}: {}", p.display(), e)))
        .collect::<Result<_, _>>()?;

    // Use the dimensions of the first frame as the canvas size;
    // resize others to match if they differ.
    let (width, height) = images[0].dimensions();

    let output_file = File::create(output_path)?;
    let mut encoder = Encoder::new(output_file, width as u16, height as u16, &[])?;
    encoder.set_repeat(Repeat::Infinite)?;

    // GIF delay is in centiseconds.
    let delay_cs = (delay_ms / 10) as u16;

    for img in &images {
        let img = if img.dimensions() != (width, height) {
            img.resize_exact(width, height, FilterType::Lanczos3)
        } else {
            img.clone()
        };

        let rgba = img.to_rgba8();
        let mut pixels = rgba.into_raw();

        let mut frame = Frame::from_rgba_speed(width as u16, height as u16, &mut pixels, 10);
        frame.delay = delay_cs;
        encoder.write_frame(&frame)?;
    }

    Ok(())
}
