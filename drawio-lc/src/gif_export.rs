use std::{fs::File, path::Path};

use gif::{Encoder, Frame, Repeat};
use image::{DynamicImage, GenericImageView, ImageBuffer, Rgba, RgbaImage};

/// Build an animated GIF from a list of PNG paths.
/// The GIF is written to `output_path`.
/// `delays_ms` must have the same length as `png_paths`; each entry is the
/// display duration of the corresponding frame in milliseconds (rounded to
/// centiseconds).  If a single uniform delay is needed, pass a slice where
/// every element is the same value.
///
/// All frames are placed on a white canvas sized to the maximum width and
/// height across all input PNGs. Frames smaller than the canvas are centred
/// rather than stretched, so aspect ratios are always preserved.
pub fn build_animated_gif(
    png_paths: &[&Path],
    output_path: &Path,
    delays_ms: &[u32],
) -> Result<(), Box<dyn std::error::Error>> {
    if png_paths.is_empty() {
        return Err("No PNG frames provided for GIF".into());
    }
    if delays_ms.len() != png_paths.len() {
        return Err(format!(
            "build_animated_gif: {} paths but {} delays",
            png_paths.len(),
            delays_ms.len()
        )
        .into());
    }

    // Load all images first so we can determine the common canvas size.
    let images: Vec<DynamicImage> = png_paths
        .iter()
        .map(|p| image::open(p).map_err(|e| format!("Failed to load {}: {}", p.display(), e)))
        .collect::<Result<_, _>>()?;

    // Use the maximum width and height across all frames so no frame needs to
    // be stretched (smaller frames are centred on a white background).
    let width = images.iter().map(|i| i.dimensions().0).max().unwrap();
    let height = images.iter().map(|i| i.dimensions().1).max().unwrap();

    let output_file = File::create(output_path)?;
    let mut encoder = Encoder::new(output_file, width as u16, height as u16, &[])?;
    encoder.set_repeat(Repeat::Infinite)?;

    for (img, &delay_ms) in images.iter().zip(delays_ms.iter()) {
        let (iw, ih) = img.dimensions();
        let canvas: RgbaImage = if (iw, ih) == (width, height) {
            img.to_rgba8()
        } else {
            // Centre the frame on a white canvas without stretching.
            let mut canvas: RgbaImage =
                ImageBuffer::from_pixel(width, height, Rgba([255u8, 255, 255, 255]));
            let x_off = (width - iw) / 2;
            let y_off = (height - ih) / 2;
            for (x, y, pixel) in img.to_rgba8().enumerate_pixels() {
                canvas.put_pixel(x + x_off, y + y_off, *pixel);
            }
            canvas
        };

        // GIF delay is in centiseconds.
        let delay_cs = (delay_ms / 10) as u16;
        let mut pixels = canvas.into_raw();
        let mut frame = Frame::from_rgba_speed(width as u16, height as u16, &mut pixels, 10);
        frame.delay = delay_cs;
        encoder.write_frame(&frame)?;
    }

    Ok(())
}
