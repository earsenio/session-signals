# Changelog

All notable changes to Session Signals are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Session ignore rules — hide machine-spawned Claude Code sessions (headless
  `claude --print` runs launched by background tooling) from the widget list and
  the tray rollup. Hidden sessions stay tracked but never colour the tray or fire
  notifications, and removing a rule brings them straight back.
- Rules are data-driven via the new `config.ignore_rules`, with two matcher kinds:
  `cwd_contains` and an anchored `first_prompt_prefix`. **Ships empty — nothing
  is hidden until you opt in.** See
  [docs/IGNORING_BOT_SPAWNED_SESSIONS.md](docs/IGNORING_BOT_SPAWNED_SESSIONS.md)
  for ready-made patterns and how to write your own.
- Observation store: Session Signals now reads each session's opening prompt
  once and records a **salted hash** of it (never plaintext) so a future
  release can offer a filter rule built from a pattern it actually saw.
  Counts only, pruned after `observe_retain_days` (30 by default); toggle
  with `observe_enabled` (on by default). Sessions a human marker precedes —
  a slash command, `<ide_opened_file>`, `<ide_selection>` — are never
  observed.
- `never_hide` allowlist and reveal-on-block safety valve. `never_hide`
  entries always outrank `ignore_rules` for the same session, and are also
  never observed. Separately, any `ignore_rules`-hidden session that hits a
  real block on you (a permission prompt, plan approval) is un-hidden until
  it restarts and notifies as normal — a filter should never be able to
  swallow a request that needs you specifically. See
  [docs/IGNORING_BOT_SPAWNED_SESSIONS.md](docs/IGNORING_BOT_SPAWNED_SESSIONS.md).
- Suggested filters: Session Signals can now offer a ready-made ignore rule
  built from an opening it actually observed repeating, instead of you
  writing one by hand. Accept it to write the rule, dismiss it for this run,
  or declare it your own permanently (which also purges it from the
  observation store). Backed by a new `propose_threshold` setting (3 by
  default, the minimum cluster size before a pattern is offered). See the
  "Suggested filters" section of
  [docs/IGNORING_BOT_SPAWNED_SESSIONS.md](docs/IGNORING_BOT_SPAWNED_SESSIONS.md).
- A minimum sample length (60 characters) before a cluster is proposal-
  eligible, backed by a real measurement rather than a guess — a sweep over a
  local corpus found human/machine openings colliding at short prefix
  lengths, dropping to zero collisions from 57 characters onward. See
  [docs/measurements.md](docs/measurements.md).
- A committed fixture corpus (`src-tauri/tests/fixtures/`) and an end-to-end
  replay test (`corpus_replay.rs`) driving the real ingest pipeline —
  marker guard, `never_hide`, observation, clustering, and the engine's
  hidden-ness verdict — against authored cases covering the PRD's named
  adversarial scenarios (a quoted spawner phrase, an IDE-marked opening, a
  repeated human opening, array-content prompts, SHA-named worktree cwds,
  shared cwds). Replaces trust in one machine's ad-hoc scripts with a test
  suite that fails loudly if a future change breaks discrimination.
- Settings → **Session filtering** section: editable `ignore_rules` and
  `never_hide` lists, the built-in interaction markers (shown read-only), an
  audit view listing every currently-hidden session with the rule hiding it
  plus the reveal-on-block count, and the suggested-filter card rendered in
  place. A quiet tray line ("Session filtering: N suggestion…") is the only
  discovery surface for a waiting proposal — it never changes the tray icon
  or colour. See
  ["Managing rules in Settings"](docs/IGNORING_BOT_SPAWNED_SESSIONS.md#managing-rules-in-settings).

### Fixed
- Prompts sent as content blocks (rather than a plain string) are now read
  correctly. The previous string-only assumption made the opening prompt of a
  large share of sessions invisible, which silently disabled first-prompt ignore
  rules for them.
- A session's first prompt is re-checked after an empty read instead of latching
  permanently. The first read happens at `SessionStart`, before any prompt
  exists, so a one-shot check meant first-prompt rules could never fire.
- `<ide_opened_file>` / `<ide_selection>` are recognised as interaction markers
  alongside slash commands, so IDE-context sessions are never treated as
  machine-spawned.
- A session blocked on an `AskUserQuestion` prompt or plan approval
  (`ExitPlanMode`) now turns **Needs you** (red) and notifies, instead of
  sitting at **Working** (orange). These tools block on you the moment they fire
  but emit no notification the listener can see, so the engine now escalates on
  their `PreToolUse` and returns to **Working** once you answer.
- Dismissing a suggested filter ("Not now") no longer re-offers the same
  opening at the next prefix length. A long opening fingerprints at several
  lengths; the dismissal filter used to run before shortest-prefix-wins
  dedup, so it only cleared the fingerprint shown and the next-longest record
  of the same opening got promoted immediately, making "Not now" look like it
  didn't work.
- A session that hit a permission prompt before its opening prompt was
  classified could be hidden by a first-prompt ignore rule while genuinely
  waiting on you — the late classification arriving after the block used to
  hide it instead of leaving it revealed. It now stays visible.

## [0.4.0] - 2026-07-06

Rebrand to **Session Signals**, plus the pre-open-source hardening backlog.

### Changed
- **Renamed Beacon → Session Signals** across the user-facing surface: product
  name, window titles, tray tooltip and menu, notification titles, widget
  header, and settings copy. Internal codenames are deliberately unchanged
  (`cc-beacon`, `beacon_lib`, `beacon.json`, `X-Beacon-Token`, the
  `com.beacon.cc` bundle identifier), so no wire or persistence contract moved
  and **no migration is needed** — the app-data directory is stable.
- Session labels ship structured (`{folder, branch}`) instead of one combined
  string, so the widget no longer regex-splits a label and directories named
  `foo (bar)` render correctly.
- Dependency upgrades: TypeScript 5.8 → 6.0, Vite 7 → 8 (with
  `@vitejs/plugin-react` 4 → 6), `getrandom` 0.2 → 0.4, `windows` 0.61 → 0.62,
  `png` 0.17 → 0.18, `@tauri-apps/cli` 2.11.4, Prettier 3.9.4.

### Added
- Git worktree sessions resolve to the **main repo name + branch** with a
  "worktree" tag, instead of showing the worktree folder's (often random) name
  with no branch.
- The expanded widget auto-fits its height to its content.
- Widget opacity slider in settings; the widget applies it live.
- Release assets now carry `SHA256SUMS-<os>.txt` per platform and signed
  build-provenance attestations; the README documents verification.
- CI: version-consistency check (`package.json` vs `Cargo.toml`) and a
  `cargo-deny` license gate.

### Fixed
- A 30-second `SessionEnd` tombstone stops straggler subagent heartbeats from
  resurrecting an ended session as **Working**; a genuine restart clears it.
- The widget footer no longer shows a stale port after a port change.
- The hook installer no longer claims a foreign loopback `/hook` entry that
  carries its own headers.
- Mutex poisoning is recovered from throughout, and the events worker and stale
  sweep catch panics, so one panic can't zombify the app.

### Security
- `GET /state` is token-gated — the snapshot leaked labels, cwds, and
  descriptors to any local reader.
- Constant-time (XOR-fold) token comparison.
- `token::generate` fails closed instead of minting a time-seeded fallback.
- `~/.claude/settings.json` is written atomically (write-temp-then-rename).
- `terminal_tty` is validated against a device-path shape before being spliced
  into AppleScript (command-injection guard).
- The capture script is created `0o700` from the first byte (it embeds the
  token), and looser permissions from earlier versions are repaired on rewrite.

### Removed
- Unused dependencies: `objc2`, `objc2-foundation`, `tauri-plugin-opener`, and
  the Tauri `image-png` feature. `withGlobalTauri` disabled.

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

[Unreleased]: https://github.com/earsenio/session-signals/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/earsenio/session-signals/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/earsenio/session-signals/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/earsenio/session-signals/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/earsenio/session-signals/releases/tag/v0.1.1
