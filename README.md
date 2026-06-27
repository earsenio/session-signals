# Beacon (`cc-beacon`)

A lightweight desktop status indicator for Claude Code. A tray/menu-bar icon
shows a rollup status that changes color as your sessions move between
**Needs-you 🔴 / Working 🟠 / Ready 🟢 / Idle ⚪**. Detection is via Claude Code
**hooks** that POST to a local listener — no terminal scraping, fully local, no
network egress.

> **Status: Phase 3 (Notifications + settings) complete.** Tray rollup driven by
> a real detection chain, a frameless always-on-top widget, configurable
> per-state OS notifications fired on transitions, and a full settings window
> (port, stale timeout, launch-on-login, hook status). Themes/packaging come in
> Phase 4. See `docs/SPEC.md` and `CLAUDE.md`.

## Stack

Tauri 2 · React 19 · TypeScript · Vite. Rust owns the shell (tray, windows,
listener, state engine); React is a thin renderer.

## How it works

```
Claude Code hooks ──POST /hook──▶ listener.rs ──▶ engine.rs ──▶ tray.rs
   (HTTP, per event)              127.0.0.1:4317   (state map)    (icon color)
                                                        │
                                                        └─emit "sessions-updated"─▶ React
                                                                                    ├─ widget
                                                                                    └─ settings
```

- **listener.rs** — tiny_http server bound to `127.0.0.1:4317`. `POST /hook`
  ingests hook JSON; `GET /state` is a loopback-only readback used in tests.
- **engine.rs** — session map keyed by `session_id`, applies the derivation
  rules in `CLAUDE.md`, computes the rollup (Red > Orange > Green > Grey), and
  sweeps stale sessions.
- **tray.rs** — colored tray icon + menu (show/hide widget, install/uninstall
  hooks, open, quit).
- **hooks.rs** — non-destructively merges Beacon's HTTP hooks into
  `~/.claude/settings.json`; removes only its own entries on uninstall.
- **windows.rs** — the floating widget: a frameless, transparent, always-on-top,
  draggable window. One row per live session (dot • label • state •
  time-in-state), with compact (dot-strip) and expanded modes plus an opacity
  control. Position and view prefs persist via `tauri-plugin-store`; on restore
  the position is clamped to a currently-connected monitor.
- **config.rs** — user configuration (notification prefs, port, stale timeout,
  launch-on-login), persisted as one object in the store with `#[serde(default)]`
  fields for forward-compatible migration.
- **notify.rs** — fires OS notifications on state *transitions only* (the engine
  reports a transition only when `from != to`, so a prompt that merely sits in
  Needs-you never repeats), with a short debounce to collapse storms.

The settings window (`src/settings/`) edits config live: per-state notify/sound
toggles, listener port (rebinds the live listener and reinstalls hooks; a busy
port surfaces a clear error), stale timeout, launch-on-login, and a hook status
panel. Changing the port tears down the old `tiny_http` server (`unblock`) and
swaps in a new one without restarting Beacon.

## Develop

```bash
npm install
npm run tauri dev      # run the app (tray + floating widget; no dock icon)
```

Then open the tray menu → **Install Claude Code hooks**, start a Claude Code
session, and watch the tray icon and the floating widget change color. Drag the
widget anywhere (it remembers where), toggle compact/expanded, adjust opacity,
or hide it (tray → **Show / hide widget**). A copy-paste fallback for the hook
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
