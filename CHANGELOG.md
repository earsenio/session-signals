# Changelog

All notable changes to Session Signals are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Widget rows now show **what** each subagent is doing, not just how many are
  running. Every live agent gets its own line: its type (`Explore`, `Plan`, …),
  the task it was given, and its own elapsed timer. Long tasks truncate with the
  full text on hover.
- The copy-paste hook config in Settings now includes the `beacon-capture`
  command hook, so setting Session Signals up by hand gives you repo and branch
  labels (and click-to-focus) instead of a bare folder name. Thanks
  [@VedantMadane](https://github.com/VedantMadane) — [#36].

### Fixed
- A session no longer turns green while its agents are still working. Agents run
  in the background by default, so the ordinary case was: the main turn ends,
  the row goes **Ready**, and Session Signals announces "Finished — your turn"
  with the work still in flight. The row now stays **Working** from the end of
  the turn until the last agent finishes, and the notification fires at that
  moment instead.
- Uninstalling the hooks now also removes the capture script, which embeds the
  listener token. It used to be left on disk, and reopening Settings immediately
  recreated it.
- Regenerating the listener token now rewrites the capture script even when the
  hooks were installed by hand, instead of leaving it POSTing a stale token
  until the next launch.
- Session Signals no longer holds ~29% CPU (spread across its three processes)
  while sitting idle with a working session on screen — measured at ~1-3% after
  the fix. The widget's breathing ring was drawn as an SVG `<circle>`, and WebKit
  can't give a non-root SVG child its own compositing layer, so an animation that
  should have cost nothing instead re-resolved style and repainted the widget on
  every frame — forever, at up to 120fps, over a transparent always-on-top window
  that is never occluded and so never throttled. The ring is now a plain element
  the compositor can drive on its own, and the subagent halo's per-frame
  `filter: blur()` became a static gradient.
- The widget's once-a-second age tick no longer runs while collapsed, where no
  elapsed time is displayed.
- Heartbeat events (`PostToolUse`, `SubagentStart`/`Stop`) no longer re-render the
  tray icon and push a fresh payload to every window when nothing visible changed
  — during a busy turn that fired many times a second.

## [0.4.1] - 2026-08-20

### Fixed
- macOS no longer asks for permission to read your project folders. Session
  Signals used to resolve each row's git branch by reading `<cwd>/.git/HEAD`
  itself, and macOS grants folder access per protected category (Desktop,
  Documents, Downloads, network volumes are each a separate grant) — so the
  first session under a folder you hadn't yet approved popped a prompt, on a
  2-second poll. The repo and branch are now resolved by the capture hook, which
  runs in your own shell under the terminal's existing access, and arrive in the
  hook payload. The app reads nothing outside its own data directory,
  `~/.claude/settings.json`, and each session's transcript.
- The capture hook now also runs on `Stop` (macOS/Linux), so switching branches
  mid-session updates the widget within one turn instead of at the next restart.
- Click-to-focus no longer breaks after a restart once per-turn capture starts:
  the stored terminal handle is merged rather than overwritten.
- A working directory containing a quote or backslash no longer produces a
  malformed capture payload, which previously dropped that session's terminal
  handle silently.
- A session blocked on an `AskUserQuestion` prompt or plan approval
  (`ExitPlanMode`) now turns **Needs you** (red) and notifies, instead of
  sitting at **Working** (orange). These tools block on you the moment they fire
  but emit no notification the listener can see, so the engine now escalates on
  their `PreToolUse` and returns to **Working** once you answer.

### Changed
- Dependency refresh: Tauri 2.11.5, `tauri-plugin-store` 2.4.4,
  `tauri-plugin-single-instance` 2.4.3, serde 1.0.229, serde_json 1.0.151,
  and the frontend toolchain (Vite 8.2.1, ESLint 10.7.0, Prettier 3.9.5).

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

[#36]: https://github.com/earsenio/session-signals/pull/36
[Unreleased]: https://github.com/earsenio/session-signals/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/earsenio/session-signals/compare/v0.3.0...v0.4.1
[0.3.0]: https://github.com/earsenio/session-signals/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/earsenio/session-signals/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/earsenio/session-signals/releases/tag/v0.1.1
