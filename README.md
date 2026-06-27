# Beacon (`cc-beacon`)

A lightweight desktop status indicator for Claude Code. A tray/menu-bar icon
shows a rollup status that changes color as your sessions move between
**Needs-you 🔴 / Working 🟠 / Ready 🟢 / Idle ⚪**. Detection is via Claude Code
**hooks** that POST to a local listener — no terminal scraping, fully local, no
network egress.

> **Status: Phase 1 (Foundation) complete.** Tray rollup driven by a real
> detection chain. Floating widget, notifications, settings UI, and themes come
> in later phases. See `docs/SPEC.md` and `CLAUDE.md`.

## Stack

Tauri 2 · React 19 · TypeScript · Vite. Rust owns the shell (tray, windows,
listener, state engine); React is a thin renderer.

## How it works

```
Claude Code hooks ──POST /hook──▶ listener.rs ──▶ engine.rs ──▶ tray.rs
   (HTTP, per event)              127.0.0.1:4317   (state map)    (icon color)
                                                        │
                                                        └──emit──▶ React webview
```

- **listener.rs** — tiny_http server bound to `127.0.0.1:4317`. `POST /hook`
  ingests hook JSON; `GET /state` is a loopback-only readback used in tests.
- **engine.rs** — session map keyed by `session_id`, applies the derivation
  rules in `CLAUDE.md`, computes the rollup (Red > Orange > Green > Grey), and
  sweeps stale sessions.
- **tray.rs** — colored tray icon + menu (install/uninstall hooks, open, quit).
- **hooks.rs** — non-destructively merges Beacon's HTTP hooks into
  `~/.claude/settings.json`; removes only its own entries on uninstall.

## Develop

```bash
npm install
npm run tauri dev      # run the app (tray-only; no main window)
```

Then open the tray menu → **Install Claude Code hooks**, start a Claude Code
session, and watch the icon change color. A copy-paste fallback for the hook
config lives in the settings window (tray → **Open Beacon…**).

## Test

```bash
npm run build                 # typecheck + bundle frontend
cd src-tauri && cargo test    # engine, hook-merge, and install integration tests
```

## Privacy

The listener binds `127.0.0.1` only and rejects non-loopback peers. No
telemetry, no outbound network calls. The hook installer always backs up
`settings.json` and offers a clean uninstall.
