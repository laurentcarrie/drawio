# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-08-05

### Added

- `Transform::Color` now accepts `ids: Vec<String>` — multiple cells can be recolored in a single transform entry
- `Transform::ColorEdges` — recolor all edges not connected to any node in an `exclude` list (both `fillColor` and `strokeColor`)
- `Transform::ReplaceText` — replace the display label of a cell by id
- `Transform::TitleSlide` — generate a centered title slide from Markdown text, sized to match the reference PNG dimensions; must be the only transform in its step
- PNG export for every derived step via `drawio --export`, constrained to the reference dimensions (width × height from the original file)
- Animated GIF assembled from all step PNGs in order (1 s per frame, infinite loop)
- MP4 export via `ffmpeg` (H.264, yuv420p, 25 fps, faststart) for Confluence video player support
- Config validation: `from` field of each step must reference the original file or a previous step's output
- Reference size auto-detection: exports the original file to a temporary PNG, reads its dimensions, removes the temp file
- `Makefile` with `all` and `clean` targets

### Changed

- `Transform::Color` renamed field `id: String` → `ids: Vec<String>`
- drawio CLI stderr suppressed to avoid spurious "Error: Export failed" noise
- `patch_cell` refactored: style patching extracted into `patch_style_color` helper used by both `Color` and `ColorEdges`

## [0.1.0] - 2026-08-05

### Added

- Initial project structure as a Rust binary crate (`drawio-lc`)
- YAML config model (`Config`, `Derived`, `Transform`) backed by `serde` / `serde_yaml`
- `Transform::Color` variant to recolor a draw.io cell by id, targeting `fillColor` in the style attribute
- `transform` module that reads a draw.io XML file, applies a list of transforms, and writes the result to the configured output path
- CLI entry point: accepts a single YAML config file as argument, processes every derived entry and reports generated files
- GitHub Actions workflow to publish the crate to crates.io on version tag push, with tag/`Cargo.toml` version guard and test gate
