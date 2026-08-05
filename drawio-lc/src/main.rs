mod gif_export;
mod model;
mod transform;

use std::{collections::HashSet, env, fs, path::Path, process};

use image::GenericImageView;
use model::Config;

fn validate_config(config: &Config) {
    let mut available: HashSet<&str> = HashSet::new();
    available.insert(&config.original);

    for (i, derived) in config.derived.iter().enumerate() {
        if !available.contains(derived.from.as_str()) {
            eprintln!(
                "Error in step {}: 'from' field '{}' is neither the original file nor the output of a previous step.\n\
                 Available sources at this point: [{}]",
                i + 1,
                derived.from,
                available
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            process::exit(1);
        }
        available.insert(&derived.output);
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <config.yaml>", args[0]);
        process::exit(1);
    }

    let content = fs::read_to_string(&args[1]).unwrap_or_else(|e| {
        eprintln!("Error reading {}: {}", args[1], e);
        process::exit(1);
    });

    let config: Config = serde_yaml::from_str(&content).unwrap_or_else(|e| {
        eprintln!("Error parsing YAML: {}", e);
        process::exit(1);
    });

    validate_config(&config);

    // Export the original file to a temp PNG to get the reference dimensions.
    let ref_png = Path::new(&config.original).with_extension("_ref.png");
    transform::export_reference_png(Path::new(&config.original), &ref_png).unwrap_or_else(|e| {
        eprintln!("Error exporting reference PNG: {}", e);
        process::exit(1);
    });
    let ref_size = image::open(&ref_png)
        .unwrap_or_else(|e| {
            eprintln!("Error reading reference PNG: {}", e);
            process::exit(1);
        })
        .dimensions();
    fs::remove_file(&ref_png).unwrap_or_else(|e| {
        eprintln!("Warning: could not remove temp file {}: {}", ref_png.display(), e);
    });
    println!("Reference size: {}x{}px", ref_size.0, ref_size.1);

    let input_xml = fs::read_to_string(&config.original).unwrap_or_else(|e| {
        eprintln!("Error reading original file {}: {}", config.original, e);
        process::exit(1);
    });

    for derived in &config.derived {
        let from_xml = if derived.from == config.original {
            input_xml.clone()
        } else {
            fs::read_to_string(&derived.from).unwrap_or_else(|e| {
                eprintln!("Error reading {}: {}", derived.from, e);
                process::exit(1);
            })
        };

        transform::transform(&from_xml, derived, Some(ref_size)).unwrap_or_else(|e| {
            eprintln!("Error transforming into {}: {}", derived.output, e);
            process::exit(1);
        });

        println!("Generated {}", derived.output);
    }

    // Build animated GIF from all generated PNGs in step order.
    let png_paths: Vec<_> = config
        .derived
        .iter()
        .map(|d| Path::new(&d.output).with_extension("png"))
        .collect();
    let png_path_refs: Vec<&Path> = png_paths.iter().map(|p| p.as_path()).collect();

    let gif_path = Path::new(&config.original).with_extension("gif");
    gif_export::build_animated_gif(&png_path_refs, &gif_path, 1000).unwrap_or_else(|e| {
        eprintln!("Error building GIF: {}", e);
        process::exit(1);
    });
    println!("Generated {}", gif_path.display());

    // Convert GIF to MP4 for Confluence video player (play/pause controls).
    let mp4_path = Path::new(&config.original).with_extension("mp4");
    export_mp4(&gif_path, &mp4_path).unwrap_or_else(|e| {
        eprintln!("Error building MP4: {}", e);
        process::exit(1);
    });
    println!("Generated {}", mp4_path.display());
}

fn export_mp4(gif_path: &Path, mp4_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // yuv420p: required for broad browser/Confluence compatibility.
    // vf "scale=trunc(iw/2)*2:trunc(ih/2)*2": libx264 requires even dimensions.
    // -r 25: encode at 25 fps (duplicating frames) so players handle it correctly.
    // -movflags +faststart: places metadata at the front for streaming.
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            gif_path.to_str().ok_or("invalid gif path")?,
            "-vf",
            "scale=trunc(iw/2)*2:trunc(ih/2)*2",
            "-r", "25",
            "-c:v", "libx264",
            "-pix_fmt", "yuv420p",
            "-movflags", "+faststart",
            mp4_path.to_str().ok_or("invalid mp4 path")?,
        ])
        .status();

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("ffmpeg exited with status {}", s).into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err("ffmpeg not found — install ffmpeg and ensure it is on PATH".into())
        }
        Err(e) => Err(e.into()),
    }
}
