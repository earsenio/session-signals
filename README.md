# Beacon (`cc-beacon`)

**A traffic-light for your Claude Code sessions — see at a glance which ones need you, without alt-tabbing.**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey.svg)](#install)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-24C8DB.svg)](https://tauri.app)
[![Release](https://img.shields.io/github/v/release/earsenio/cc-beacon?include_prereleases&sort=semver)](https://github.com/earsenio/cc-beacon/releases)

A lightweight desktop status indicator for [Claude Code](https://claude.com/claude-code). A
tray/menu-bar icon shows a rollup status, and a floating, always-on-top widget
shows one row per live session — each colored as it moves between
**Needs-you 🔴 / Working 🟠 / Ready 🟢 / Idle ⚪**. Detection is via Claude Code
**hooks** that POST to a local listener: no terminal scraping, no process
inspection, fully local.

<!-- TODO: replace with a real screenshot/GIF of the tray icon + floating widget -->
![Beacon's tray icon and floating widget showing per-session status](docs/assets/beacon-screenshot.png)

---

## Why

When you run several Claude Code sessions at once, you lose track of which one is
blocked on a permission prompt, which is still working, and which is done and
waiting. Beacon surfaces all of them in one place:

- **Tray rollup** — one icon, colored by the most urgent session
  (Red > Orange > Green > Grey). Red the moment *any* session needs you.
- **Floating widget** — a row per session: status dot • folder + git branch •
  state • time-in-state. Click a row to jump to that terminal.
- **Notifications** — configurable, per-state, fired on transitions only (never
  nags while idle).

## Install

**Download a release build** from the [Releases page](https://github.com/earsenio/cc-beacon/releases):

- **macOS** — `.dmg` (universal). Builds are currently **unsigned**: on first
  launch, right-click the app → **Open** to bypass Gatekeeper.
- **Windows** — `.msi` / `.exe`. Click through the SmartScreen "More info →
  Run anyway" prompt (unsigned).

Prefer to build it yourself? See [Build from source](#build-from-source).

## Set up Claude Code hooks

Beacon detects session activity through Claude Code's hook system. One-time setup:

1. Launch Beacon. It runs in the tray/menu bar (no dock icon).
2. Open the tray menu → **Install Claude Code hooks**.
3. Start (or restart) a Claude Code session and watch the tray + widget react.

This **non-destructively merges** Beacon's hooks into `~/.claude/settings.json`
(a backup is written first) and registers HTTP hooks for `SessionStart`,
`UserPromptSubmit`, `PostToolUse`, `Notification`, `Stop`, `SubagentStop`, and
`SessionEnd`. Remove them anytime via the tray → **Uninstall hooks**. A
copy-paste fallback for the hook block lives in the settings window
(tray → **Open Beacon…**).

## Configuration

Open the settings window (tray → **Open Beacon…**) to adjust:

| Setting | Default | Notes |
|---|---|---|
| Listener port | `4317` | Rebinds the live listener and reinstalls hooks; a busy port surfaces a clear error. |
| Stale timeout | `10` min | A silent session greys out, then drops after a short grace. |
| Notifications | Red on (no sound) | Per-state notify + sound toggles; fired on state *transitions* only. |
| Launch on login | off | — |
| Widget | remembered | Position, compact/expanded mode, and opacity persist. |

**Port & token.** The listener binds `127.0.0.1:<port>` only. On first run Beacon
generates a 64-hex-char auth token and stamps it into the installed hooks; every
`POST /hook` must carry a matching `X-Beacon-Token` header or it is rejected. The
token lives in the app-data store and in `~/.claude/settings.json` (both
user-readable, plaintext — appropriate for a loopback shared secret). If you
change the port, Beacon re-stamps the hooks automatically.

## Privacy

Beacon is **fully local**, by design and by construction:

- The listener **binds `127.0.0.1` only** and rejects any non-loopback peer.
- **No telemetry. No outbound network calls. Ever.** There is no HTTP client in
  the codebase — the only network surface is the inbound loopback listener.
- State mutations (`POST /hook`) are **token-gated**; the read-only `GET /state`
  is loopback-bound.
- The hook installer **backs up** `settings.json` before editing and offers a
  **clean uninstall** that removes only Beacon's own entries.

See [SECURITY.md](SECURITY.md) for the full threat model.

## Build from source

**Prerequisites:** Node.js 18+, the Rust toolchain (via [rustup](https://rustup.rs)),
and the platform's Tauri dependencies (see [docs/BUILD.md](docs/BUILD.md)).

```bash
npm install
npm run tauri dev      # run the app (tray + floating widget; no dock icon)
```

To produce installers:

```bash
npm run tauri build    # see docs/BUILD.md for per-OS bundles & signing
```

Run the checks:

```bash
npm run build                 # typecheck (tsc) + bundle (vite)
cd src-tauri && cargo test    # engine, hook-merge, listener, install tests
```

## Architecture

```
Claude Code hooks ──POST /hook──▶ listener.rs ──▶ engine.rs ──▶ tray.rs
   (HTTP, per event)              127.0.0.1:4317   (state map)    (icon color)
                                                        │
                                                        └─emit "sessions-updated"─▶ React
                                                                                    ├─ widget
                                                                                    └─ settings
```

The Rust shell owns the truth: the **listener** ingests hook JSON, the **engine**
keys sessions by `session_id`, applies the derivation rules, computes the rollup,
and sweeps stale sessions; React is a thin renderer of the engine's state.

- **Stack:** Tauri 2 · React 19 · TypeScript · Vite.
- **Full spec:** [docs/SPEC.md](docs/SPEC.md). **Standing context for contributors:** [CLAUDE.md](CLAUDE.md).
- **Contributing:** [CONTRIBUTING.md](CONTRIBUTING.md). **Releases:** [docs/VERSIONING.md](docs/VERSIONING.md), [CHANGELOG.md](CHANGELOG.md).

## License

[MIT](LICENSE) © 2026 Beacon contributors. Bundled third-party components
(including the Geist font under SIL OFL-1.1) are listed in
[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
