# Changelog

All notable changes to Session Signals are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-06-29

First open-source-ready release: the per-session descriptor feature plus a full
OSS-readiness pass (licensing, docs, tooling, CI, and security hardening).

### Added
- Per-session descriptor (Claude Code's session title, else the latest prompt)
  shown on widget rows.
- Restrictive webview Content-Security-Policy (previously disabled).
- Lint/format toolchain: ESLint + Prettier (frontend) and rustfmt + clippy
  (Rust shell), with `lint` / `format` / `typecheck` npm scripts.
- PR CI workflow (lint, typecheck, test, and a build smoke on macOS + Windows).
- Engine tests for event→state derivation, rollup priority, and the stale sweep.
- OSS docs & meta: `LICENSE` (MIT), `THIRD_PARTY_LICENSES.md`, `.editorconfig`,
  an end-user `README` rewrite, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`,
  `SECURITY.md`, this changelog, GitHub issue/PR templates, `FUNDING.yml`, and
  Dependabot.

### Changed
- Real package metadata (license, repository, authors) across `Cargo.toml`,
  `package.json`, and `tauri.conf.json`.
- Release workflow modernized to `tauri-action@v1` and current support actions.
- Relocated internal scaffolding into the gitignored `docs/internal/`; reconciled
  `CLAUDE.md`'s build-phase references.

### Fixed
- Subagent activity no longer masks a session's "Needs you" state.
- Widget background no longer blinks between opaque and set opacity (dropped the
  CSS `backdrop-filter`).
- Widget no longer restores to a ~0-height list when expanded, and no longer
  sticks on a stale snapshot (reconciles against the engine).
- Settings window no longer presents blank after a dev-server reload.
- Click-to-focus survives an app restart (captured terminal handles persist).

### Removed
- Unused Vite/Tauri starter assets (`src/assets/react.svg`, `public/tauri.svg`).

### Security
- Repo-level gitignore for `.claude/settings.local.json` so a fresh clone can't
  accidentally commit a per-user permission allowlist / machine path.

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

[Unreleased]: https://github.com/earsenio/session-signals/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/earsenio/session-signals/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/earsenio/session-signals/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/earsenio/session-signals/releases/tag/v0.1.1
