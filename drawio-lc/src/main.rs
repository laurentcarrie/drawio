mod confluence_export;
mod gif_export;
mod html_export;
mod model;
mod transform;

use std::{collections::HashMap, env, fs, path::Path, process};

use hex;
use image::GenericImageView;
use model::{Config, Derived, Transform};
use sha2::{Digest, Sha256};

fn validate_config(config: &Config) {
    if config.derived.is_empty() {
        eprintln!("Error: 'derived' list is empty — nothing to do.");
        process::exit(1);
    }

    // The first step must have an explicit 'from'.
    if config.derived[0].from.is_none() {
        eprintln!("Error: the first derived step must have an explicit 'from' field.");
        process::exit(1);
    }

    let mut available: std::collections::HashSet<&str> = std::collections::HashSet::new();
    // The first step's 'from' is the root source; seed it as available.
    available.insert(config.derived[0].from.as_deref().unwrap());

    for (i, derived) in config.derived.iter().enumerate() {
        let from = derived.from.as_deref().unwrap_or("");
        if !from.is_empty() && !available.contains(from) && !std::path::Path::new(from).is_file() {
            eprintln!(
                "Error in step {}: 'from' field '{}' is not the source file, a local file, nor the output of a previous step.\n\
                 Available sources at this point: [{}]",
                i + 1,
                from,
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

fn is_title_slide(derived: &model::Derived) -> bool {
    derived.transforms.len() == 1
        && matches!(derived.transforms[0], model::Transform::TitleSlide { .. })
}

/// Fill in missing `from` fields by walking backwards to find the nearest
/// non-TitleSlide predecessor's output.  If all predecessors are TitleSlides,
/// falls back to the root source (the `from` of the first step).
/// Must be called before `validate_config`.
fn resolve_from_fields(config: &mut Config) {
    for i in 1..config.derived.len() {
        if config.derived[i].from.is_some() {
            continue;
        }
        // Walk backwards to find the nearest non-TitleSlide predecessor.
        let resolved = (0..i)
            .rev()
            .find(|&j| !is_title_slide(&config.derived[j]))
            .map(|j| config.derived[j].output.clone())
            // All predecessors were TitleSlides — fall back to the root source.
            .or_else(|| config.derived[0].from.clone());
        config.derived[i].from = resolved;
    }
}

/// Split a transform list into sections separated by `Animation` markers.
/// Empty sections (consecutive `Animation`, leading or trailing) are discarded.
/// Returns a `Vec` of sections; each section is a `Vec<Transform>` without
/// any `Animation` element.
fn split_transform_sections(transforms: &[Transform]) -> Vec<Vec<Transform>> {
    let mut sections: Vec<Vec<Transform>> = Vec::new();
    let mut current: Vec<Transform> = Vec::new();
    for t in transforms {
        if matches!(t, Transform::Animation) {
            if !current.is_empty() {
                sections.push(current);
                current = Vec::new();
            }
        } else {
            current.push(t.clone());
        }
    }
    if !current.is_empty() {
        sections.push(current);
    }
    sections
}

/// Generate an animated GIF for one section of transforms belonging to `derived`.
///
/// * `before_xml`   – XML state *before* any transform of this section is applied.
/// * `section`      – the transforms belonging to this section (no `Animation`).
/// * `gif_path`     – output path for the section GIF.
/// * `tmp_dir`      – temporary directory for intermediate files.
/// * `stem`         – base name used to build unique tmp file names.
/// * `section_idx`  – 1-based section index (for unique tmp names).
///
/// Returns the XML state after all section transforms have been applied.
#[allow(clippy::too_many_arguments)]
fn generate_section_gif(
    before_xml: &str,
    section: &[Transform],
    gif_path: &Path,
    tmp_dir: &Path,
    stem: &str,
    section_idx: usize,
    ref_size: Option<(u32, u32)>,
    heading_margin_bottom: u32,
    list_item_spacing: u32,
    list_item_indent: u32,
    config_dir: &Path,
    delay_ms: u32,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut frame_pngs: Vec<std::path::PathBuf> = Vec::new();
    let mut current_xml = before_xml.to_string();

    // frame 0: state before this section
    let frame0_png = tmp_dir.join(format!("{}.s{}.f0.png", stem, section_idx));
    let frame0_drawio = tmp_dir.join(format!("{}.s{}.f0.drawio", stem, section_idx));
    let frame0_derived = Derived {
        output: frame0_drawio.to_string_lossy().into_owned(),
        from: Some(String::new()),
        bounding_box_tag: None,
        transforms: vec![],
        delay: None,
    };
    transform::transform(
        &current_xml,
        &frame0_derived,
        &frame0_drawio,
        &frame0_png,
        ref_size,
        heading_margin_bottom,
        list_item_spacing,
        list_item_indent,
        config_dir,
    )?;
    frame_pngs.push(frame0_png);

    // one frame per transform in the section
    for (t_idx, _t) in section.iter().enumerate() {
        let frame_png = tmp_dir.join(format!("{}.s{}.f{}.png", stem, section_idx, t_idx + 1));
        let frame_drawio = tmp_dir.join(format!("{}.s{}.f{}.drawio", stem, section_idx, t_idx + 1));

        // Build a Derived that applies all transforms up to and including t_idx.
        let partial_transforms: Vec<Transform> = section[..=t_idx].to_vec();
        let partial_derived = Derived {
            output: frame_drawio.to_string_lossy().into_owned(),
            from: Some(String::new()),
            bounding_box_tag: None,
            transforms: partial_transforms,
            delay: None,
        };
        // Apply the partial set on top of `before_xml` (not cumulative on
        // current_xml) so each frame is a clean application.
        let xml_after = transform::transform(
            before_xml,
            &partial_derived,
            &frame_drawio,
            &frame_png,
            ref_size,
            heading_margin_bottom,
            list_item_spacing,
            list_item_indent,
            config_dir,
        )?;

        // Track the final XML (after all transforms) for the caller.
        if t_idx == section.len() - 1 {
            current_xml = xml_after;
        }
        frame_pngs.push(frame_png);
    }

    // Build the section GIF with a uniform delay for all frames.
    let frame_refs: Vec<&Path> = frame_pngs.iter().map(|p| p.as_path()).collect();
    let delays: Vec<u32> = vec![delay_ms; frame_refs.len()];
    gif_export::build_animated_gif(&frame_refs, gif_path, &delays)?;

    // Clean up temporary frame PNGs.
    for p in &frame_pngs {
        let _ = fs::remove_file(p);
    }

    Ok(current_xml)
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // Parse arguments: mandatory <config.yaml>, optional --step <output>, --no-confluence
    let mut yaml_arg: Option<&str> = None;
    let mut step_filter: Option<&str> = None;
    let mut no_confluence = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--step" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --step requires a value");
                    process::exit(1);
                }
                step_filter = Some(&args[i]);
            }
            "--no-confluence" => {
                no_confluence = true;
            }
            arg if !arg.starts_with('-') => {
                yaml_arg = Some(arg);
            }
            other => {
                eprintln!("Unknown option: {}", other);
                process::exit(1);
            }
        }
        i += 1;
    }

    let yaml_arg = yaml_arg.unwrap_or_else(|| {
        eprintln!("Usage: {} <config.yaml> [--step <output>] [--no-confluence]", args[0]);
        process::exit(1);
    });

    let yaml_path = Path::new(yaml_arg);

    let content = fs::read_to_string(yaml_path).unwrap_or_else(|e| {
        eprintln!("Error reading {}: {}", yaml_path.display(), e);
        process::exit(1);
    });

    let mut config: Config = serde_yaml::from_str(&content).unwrap_or_else(|e| {
        eprintln!("Error parsing YAML: {}", e);
        process::exit(1);
    });

    resolve_from_fields(&mut config);
    validate_config(&config);

    // Directory containing the config file — used to resolve relative paths
    // in transforms such as ImportMarkdown.
    let config_dir = yaml_path.parent().unwrap_or(Path::new("."));

    // Validate --step value if provided.
    if let Some(step) = step_filter {
        if !config.derived.iter().any(|d| d.output == step) {
            eprintln!(
                "Error: --step '{}' does not match any output. Available outputs: [{}]",
                step,
                config.derived.iter().map(|d| d.output.as_str()).collect::<Vec<_>>().join(", ")
            );
            process::exit(1);
        }
    }

    // The root source file is the 'from' of the first step.
    let source_file = config.derived[0].from.as_deref().unwrap_or_else(|| {
        eprintln!("Internal error: first step 'from' not resolved");
        process::exit(1);
    });
    let source_file = source_file.to_string();

    // Export the source file to a temp PNG to get the reference dimensions.
    let ref_png = Path::new(&source_file).with_extension("_ref.png");
    transform::export_reference_png(Path::new(&source_file), &ref_png).unwrap_or_else(|e| {
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
    println!("{} -> {}", source_file, ref_png.display());
    println!("Reference size: {}x{}px", ref_size.0, ref_size.1);

    let input_xml = fs::read_to_string(&source_file).unwrap_or_else(|e| {
        eprintln!("Error reading source file {}: {}", source_file, e);
        process::exit(1);
    });

    // Keep transformed XML in memory keyed by the logical output name so
    // chained steps never need to read back from disk.
    let mut xml_cache: HashMap<String, String> = HashMap::new();
    xml_cache.insert(source_file.clone(), input_xml);

    // Collect section GIF paths across all steps, in order — converted to MP4
    // after the main loop and pushed to Confluence.
    let mut section_gif_paths: Vec<std::path::PathBuf> = Vec::new();

    // Use a temp directory for intermediate .drawio files; only PNGs are kept.
    let tmp_dir = std::env::temp_dir().join("drawio-lc");
    fs::create_dir_all(&tmp_dir).unwrap_or_else(|e| {
        eprintln!("Error creating temp dir: {}", e);
        process::exit(1);
    });

    // ── Cache directory & existing digests ───────────────────────────────────
    // Cache dir is named after the yaml file, sitting alongside it.
    let mut new_digests: HashMap<String, String> = HashMap::new();
    let cache_dir = yaml_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!(
            ".drawio.{}",
            yaml_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
    fs::create_dir_all(&cache_dir).unwrap_or_else(|e| {
        eprintln!("Error creating cache dir {}: {}", cache_dir.display(), e);
        process::exit(1);
    });
    let digest_path = cache_dir.join("digest");

    // Load previously stored digests keyed by output name.
    let stored_digests: HashMap<String, String> = fs::read_to_string(&digest_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            // Format: "<hash>  <output>"
            let mut parts = line.splitn(2, "  ");
            let hash = parts.next()?.to_string();
            let name = parts.next()?.to_string();
            Some((name, hash))
        })
        .collect();

    for derived in &config.derived {
        let from_key = derived.from.as_deref().unwrap_or_else(|| {
            eprintln!("Internal error: 'from' not resolved for step '{}'", derived.output);
            process::exit(1);
        });

        // Load from file if not already in cache (e.g., when 'from' points to an
        // external .drawio file that is not the root source).
        if !xml_cache.contains_key(from_key) {
            match fs::read_to_string(from_key) {
                Ok(xml) => { xml_cache.insert(from_key.to_string(), xml); }
                Err(e) => {
                    eprintln!("Error: cannot read 'from' file '{}': {}", from_key, e);
                    process::exit(1);
                }
            }
        }

        let from_xml = xml_cache.get(from_key).unwrap_or_else(|| {
            eprintln!("Internal error: XML for '{}' not in cache", from_key);
            process::exit(1);
        });

        // Compute the pre-generation digest: SHA256(item_json || from_xml).
        let item_json = serde_json::to_string(derived).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(item_json.as_bytes());
        hasher.update(from_xml.as_bytes());
        let current_hash = hex::encode(hasher.finalize());

        let png_path = Path::new(&derived.output).with_extension("png");

        let is_target = step_filter.map_or(true, |s| s == derived.output);

        let stem = Path::new(&derived.output)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let tmp_drawio = tmp_dir.join(format!("{}.drawio", stem));

        // Skip generation if digest matches, PNG still exists, and this is not the forced step.
        if !is_target && stored_digests.get(&derived.output) == Some(&current_hash) && png_path.exists() {
            println!("Unchanged {}", png_path.display());
            let out_xml =
                transform::transform(from_xml, derived, &tmp_drawio, &png_path, Some(ref_size), config.heading_margin_bottom, config.list_item_spacing, config.list_item_indent, config_dir)
                    .unwrap_or_else(|e| {
                        eprintln!("Error transforming into {}: {}", derived.output, e);
                        process::exit(1);
                    });
            xml_cache.insert(derived.output.clone(), out_xml);
            new_digests.insert(derived.output.clone(), current_hash);
            continue;
        }

        let out_xml = transform::transform(from_xml, derived, &tmp_drawio, &png_path, Some(ref_size), config.heading_margin_bottom, config.list_item_spacing, config.list_item_indent, config_dir)
            .unwrap_or_else(|e| {
                eprintln!("Error transforming into {}: {}", derived.output, e);
                process::exit(1);
            });

        // ── Section GIFs ─────────────────────────────────────────────────────
        // Only generate section GIFs when the step contains at least one
        // Animation marker (i.e. more than one section after splitting).
        let has_animation = derived.transforms.iter().any(|t| matches!(t, Transform::Animation));
        if has_animation {
            let sections = split_transform_sections(&derived.transforms);
            let mut section_xml = from_xml.to_string();
            for (sec_idx, section) in sections.iter().enumerate() {
                let gif_path = Path::new(&derived.output)
                    .with_extension(format!("section{}.gif", sec_idx + 1));
                let result = generate_section_gif(
                    &section_xml,
                    section,
                    &gif_path,
                    &tmp_dir,
                    &stem,
                    sec_idx + 1,
                    Some(ref_size),
                    config.heading_margin_bottom,
                    config.list_item_spacing,
                    config.list_item_indent,
                    config_dir,
                    config.delay_between_slides,
                );
                match result {
                    Ok(xml_after) => {
                        println!("Generated {}", gif_path.display());
                        section_gif_paths.push(gif_path.clone());
                        section_xml = xml_after;
                    }
                    Err(e) => {
                        eprintln!("Error generating {}: {}", gif_path.display(), e);
                        process::exit(1);
                    }
                }
            }
        }

        xml_cache.insert(derived.output.clone(), out_xml);
        new_digests.insert(derived.output.clone(), current_hash);
        println!("Generated {}", png_path.display());
    }

    // Write updated digests in step order.
    let digest_lines: Vec<String> = config
        .derived
        .iter()
        .filter_map(|d| {
            new_digests
                .get(&d.output)
                .map(|h| format!("{}  {}", h, d.output))
        })
        .collect();
    fs::write(&digest_path, digest_lines.join("\n") + "\n").unwrap_or_else(|e| {
        eprintln!("Error writing digest file {}: {}", digest_path.display(), e);
        process::exit(1);
    });
    println!("Written {}", digest_path.display());

    // Skip GIF/MP4/HTML/Confluence when only a single step was requested.
    if step_filter.is_some() {
        return;
    }

    // Convert section GIFs to MP4 for Confluence embedding.
    let mut section_mp4_paths: Vec<std::path::PathBuf> = Vec::new();
    for gif_path in &section_gif_paths {
        let mp4_path = gif_path.with_extension("mp4");
        export_mp4(gif_path, &mp4_path).unwrap_or_else(|e| {
            eprintln!("Error building section MP4 {}: {}", mp4_path.display(), e);
            process::exit(1);
        });
        println!("Generated {}", mp4_path.display());
        section_mp4_paths.push(mp4_path);
    }

    // Build animated GIF from all generated PNGs in step order.
    // Each slide uses its own `delay` if set, otherwise `delay_between_slides`.
    let png_paths: Vec<_> = config
        .derived
        .iter()
        .map(|d| Path::new(&d.output).with_extension("png"))
        .collect();
    let png_path_refs: Vec<&Path> = png_paths.iter().map(|p| p.as_path()).collect();
    let slide_delays: Vec<u32> = config
        .derived
        .iter()
        .map(|d| d.delay.unwrap_or(config.delay_between_slides))
        .collect();

    // Output files (gif, mp4, html) are named after the yaml file.
    let yaml_stem = yaml_path.with_extension("");

    let gif_path = yaml_stem.with_extension("gif");
    gif_export::build_animated_gif(&png_path_refs, &gif_path, &slide_delays).unwrap_or_else(|e| {
        eprintln!("Error building GIF: {}", e);
        process::exit(1);
    });
    println!("Generated {}", gif_path.display());

    let mp4_path = yaml_stem.with_extension("mp4");
    export_mp4(&gif_path, &mp4_path).unwrap_or_else(|e| {
        eprintln!("Error building MP4: {}", e);
        process::exit(1);
    });
    println!("Generated {}", mp4_path.display());

    let html_path = yaml_stem.with_extension("html");
    html_export::build_html_slideshow(&png_path_refs, &html_path).unwrap_or_else(|e| {
        eprintln!("Error building HTML slideshow: {}", e);
        process::exit(1);
    });
    println!("Generated {}", html_path.display());

    // Optionally push slides to Confluence if configured and not suppressed.
    if !no_confluence {
        if let Some(ref cf) = config.confluence {
            let section_mp4_refs: Vec<&Path> = section_mp4_paths.iter().map(|p| p.as_path()).collect();
            confluence_export::push_to_confluence(&png_path_refs, &mp4_path, &section_mp4_refs, cf).unwrap_or_else(|e| {
                eprintln!("Error pushing to Confluence: {}", e);
                process::exit(1);
            });
        }
    }
}

fn export_mp4(gif_path: &Path, mp4_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use model::Transform;

    #[test]
    fn split_sections_normal() {
        let transforms = vec![
            Transform::Color { tags: vec!["a".into()], color: "#FF0000".into() },
            Transform::Animation,
            Transform::Color { tags: vec!["b".into()], color: "#00FF00".into() },
            Transform::Color { tags: vec!["c".into()], color: "#0000FF".into() },
        ];
        let sections = split_transform_sections(&transforms);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].len(), 1);
        assert_eq!(sections[1].len(), 2);
    }

    #[test]
    fn split_sections_leading_trailing_animation() {
        let transforms = vec![
            Transform::Animation,
            Transform::Color { tags: vec!["a".into()], color: "#FF0000".into() },
            Transform::Animation,
        ];
        let sections = split_transform_sections(&transforms);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].len(), 1);
    }

    #[test]
    fn split_sections_consecutive_animation() {
        let transforms = vec![
            Transform::Color { tags: vec!["a".into()], color: "#FF0000".into() },
            Transform::Animation,
            Transform::Animation,
            Transform::Color { tags: vec!["b".into()], color: "#00FF00".into() },
        ];
        let sections = split_transform_sections(&transforms);
        assert_eq!(sections.len(), 2);
    }

    #[test]
    fn split_sections_no_animation() {
        let transforms = vec![
            Transform::Color { tags: vec!["a".into()], color: "#FF0000".into() },
            Transform::Color { tags: vec!["b".into()], color: "#00FF00".into() },
        ];
        let sections = split_transform_sections(&transforms);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].len(), 2);
    }

    #[test]
    fn split_sections_only_animation() {
        let transforms = vec![Transform::Animation, Transform::Animation];
        let sections = split_transform_sections(&transforms);
        assert_eq!(sections.len(), 0);
    }
}
