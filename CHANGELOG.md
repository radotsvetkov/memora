# Changelog

## [Unreleased]

### Added
- Initial project scaffolding.

## [0.1.4] - 2026-04-29

### Fixed
- Embed SQLite migration SQL files directly into release binaries so `memora index` and `memora query` no longer fail on installed builds with missing CI-only migration paths.
- Format embedded migration constant in `sqlite.rs` to satisfy `cargo fmt --check` in release CI.

## [0.1.5] - 2026-04-29

### Fixed
- Re-release the migration hotfix with rustfmt-clean source so the tag-triggered Release workflow passes end-to-end.
