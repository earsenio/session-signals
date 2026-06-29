# Changelog

All notable changes to Beacon (`cc-beacon`) are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Per-session descriptor (Claude Code's session title) shown on widget rows.
- Project meta files for open-source release: `LICENSE` (MIT),
  `THIRD_PARTY_LICENSES.md`, `.editorconfig`, `README` (end-user rewrite),
  `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, and this changelog.

### Changed
- Relocated internal scaffolding (design brief, build prompt, readiness plan)
  into the gitignored `docs/internal/`; reconciled `CLAUDE.md`'s build-phase
  references.

### Fixed
- Subagent activity no longer masks a session's "Needs you" state.
- Widget background no longer blinks between opaque and set opacity (dropped the
  CSS `backdrop-filter`).
- Widget no longer restores to a ~0-height list when expanded, and no longer
  sticks on a stale snapshot (reconciles against the engine).
- Click-to-focus survives an app restart (captured terminal handles persist).

### Removed
- Unused Vite/Tauri starter assets (`src/assets/react.svg`, `public/tauri.svg`).

### Security
- Added a repo-level gitignore for `.claude/settings.local.json` so a fresh clone
  can't accidentally commit a per-user permission allowlist / machine path.

## [0.2.0] - 2026-06-28

### Added
- Listener auth token: every `POST /hook` must carry a matching `X-Beacon-Token`
  header; live regeneration and stale-token repair supported.
- Click-to-focus: clicking a widget row focuses the exact terminal tab via a
  captured tty.
- Focus-aware notifications.
- Surface 9: per-session subagent activity indicator ("N agents running").
- Manual `workflow_dispatch` for the release workflow (including building an
  existing tag).

### Fixed
- Windows build (import `BOOL` from `windows::core`).

## [0.1.1] - 2026-06-28

### Added
- Phase 1 — detection chain: Claude Code hooks → loopback listener → state engine
  → colored tray rollup (Red > Orange > Green > Grey).
- Phase 2 — floating, always-on-top, draggable widget with a per-session
  breakdown; auto-width collapsed pill and an expanded mode.
- Phase 3 — configurable per-state OS notifications (fired on transitions only)
  and a full settings window (port, stale timeout, launch-on-login, hook status).
- "Instrument" visual restyle and full work-event coverage.
- SemVer versioning system with `package.json` as the single source of truth, and
  a tag-triggered release CI (macOS universal + Windows matrix).

### Fixed
- Concurrent sessions no longer block one another; idle sessions no longer turn
  red (they stay visible until a configurable drop window).

[Unreleased]: https://github.com/earsenio/cc-beacon/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/earsenio/cc-beacon/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/earsenio/cc-beacon/releases/tag/v0.1.1
