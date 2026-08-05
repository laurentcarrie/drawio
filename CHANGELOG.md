# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-08-05

### Added

- Initial project structure as a Rust binary crate (`drawio-lc`)
- YAML config model (`Config`, `Derived`, `Transform`) backed by `serde` / `serde_yaml`
- `Transform::Color` variant to recolor a draw.io cell by id, targeting `fillColor` in the style attribute
- `transform` module that reads a draw.io XML file, applies a list of transforms, and writes the result to the configured output path
- CLI entry point: accepts a single YAML config file as argument, processes every derived entry and reports generated files
- GitHub Actions workflow to publish the crate to crates.io on version tag push, with tag/`Cargo.toml` version guard and test gate
