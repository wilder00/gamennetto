# Changelog

All notable changes to Gamennetto are listed here. This file is written by
release-plz from Conventional Commits; see docs/VERSIONING.md.

## [Unreleased]

## [0.2.0](https://github.com/wilder00/gamennetto/compare/v0.1.0...v0.2.0) - 2026-11-25

### Added

- [**breaking**] approve each PC once; hosts check every viewer

### Fixed

- keyboard capture works in the viewer (Alt+Tab, Windows key)
- viewer takes Alt+Tab and the Windows key while controlling the host
- host finds a free port and ignores VPN adapters
- viewer keys reach the host even when the keyboard hook misses them

## [0.1.0](https://github.com/wilder00/gamennetto/releases/tag/v0.1.0) - 2026-11-24

### Added

- mirror or extend a PC screen to another PC

### Other

- sign update installers with a Rust tool instead of Node
- Add doctor command: runtime hardware, encoder and network detection
