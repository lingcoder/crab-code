# Changelog

All notable changes to Crab Code are listed here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The TUI's welcome panel parses the entries under the **most recent version**
and lists its bullets under "What's new". Keep bullets short (≤ 80 chars) and
imperative.

## [Unreleased]

### Added
- Welcome panel shown at startup on version change or new project
- `reasoning_content` parsing for DeepSeek-reasoner / DeepSeek-R1
- Collapsed display for parallel read-only tool runs (Read / Grep / Glob)
- `CRAB_USE_POWERSHELL_TOOL` env var to opt-in PowerShell tool on Windows

### Changed
- Bash tool now strictly requires POSIX shell (bash/zsh); no cmd.exe fallback
- Read-only tool classification now queried from `Tool::is_read_only()` trait
  rather than a hardcoded TUI list
- System prompt Shell line no longer reports `COMSPEC` on Windows
- Environment variables consolidated under the `CRAB_` prefix
  (was `CRAB_CODE_*` for new variables)
- Header logo redesigned and moved to `crates/tui/assets/header-logo.txt`

### Removed
- Hardcoded read-only tool list in TUI layer
