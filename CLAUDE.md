# CLAUDE.md — cc-beacon

> Working name: **Beacon** (`cc-beacon`). Rename freely.

A lightweight desktop status indicator for Claude Code users. A tray/menu-bar
icon shows a rollup status; a floating always-on-top widget shows a per-session
breakdown. Traffic-light semantics. Themes are swappable.

This file is the standing context for every Claude Code turn in this repo. The
full requirements live in `docs/SPEC.md`. The build runs in four phases; each
phase has a `/goal` prompt in `prompts/`.

---

## Locked decisions

- **Stack:** Tauri 2 + React 19 + TypeScript + Vite. Rust only for the Tauri
  shell (tray, windows, local listener). All UI in React.
- **Detection:** Claude Code **hooks** POST to a localhost HTTP listener owned by
  the app. No terminal scraping, no process inspection.
- **Multi-session:** tracked per `session_id`; the floating widget shows one row
  per live session. The tray shows a single rollup.
- **Form factor:** tray/menu-bar icon **and** a floating, always-on-top,
  draggable widget window.
- **Notifications:** configurable, per-state.
- **Privacy:** fully local. Listener binds `127.0.0.1` only. No telemetry, no
  network egress, ever.

## State model

| State | Color | Meaning (user POV) | Set by |
|---|---|---|---|
| Needs you | 🔴 Red | Blocked on you — permission, choice, or answer | `Notification` (type ∈ permission_prompt, elicitation_dialog, idle_prompt) |
| Working | 🟠 Orange | Actively running — don't interrupt | `UserPromptSubmit`; `PostToolUse` heartbeat |
| Ready | 🟢 Green | Finished its turn — okay to give new instructions | `Stop`, `SubagentStop`, `SessionStart` |
| None / stale | ⚪ Grey | No live session / session went silent | `SessionEnd`, or stale timeout |

**Tray rollup priority:** Red > Orange > Green > Grey. (Tray is red if *any*
session needs you; orange if any is working and none needs you; etc.)

## Hook contract

The app installs HTTP hooks into `~/.claude/settings.json` (merged
non-destructively). Every relevant event POSTs the same JSON Claude Code would
pass on stdin to a single endpoint:

```
POST http://127.0.0.1:4317/hook
body: { hook_event_name, session_id, cwd, ... }   // async, non-blocking
```

Events wired: `SessionStart`, `UserPromptSubmit`, `PostToolUse` (heartbeat),
`Notification`, `Stop`, `SubagentStop`, `SessionEnd`.

**Listener derivation logic** (keyed by `session_id`):

```
SessionStart            → upsert session, state = READY,  lastSeen = now
UserPromptSubmit        → state = WORKING,                lastSeen = now
PostToolUse             → lastSeen = now (heartbeat; keep current WORKING state)
Notification:
  permission_prompt   |
  elicitation_dialog  | → state = NEEDS_YOU,              lastSeen = now
  idle_prompt           |
  auth_success / other  → ignore (no state change)
Stop | SubagentStop     → state = READY,                  lastSeen = now
SessionEnd              → remove session
(no event for staleTimeout) → mark stale → drop after grace
```

> ⚠️ **Verify before building (Phase 1):** confirm exact event names, the
> `Notification` payload's type field, and that `http` + `async` hooks are
> supported on the installed Claude Code version (`claude --help hooks`). Some
> events are version-gated. If `http` hooks are unavailable, fall back to a
> `command` hook that forwards stdin to the listener (e.g. a tiny bundled
> `curl`/forwarder). Do not assume the schema — read it.

## Session presentation

- **Label:** `basename(cwd)` + git branch if resolvable. Resolve branch by
  reading `<cwd>/.git/HEAD` (no subprocess); fall back to none.
- **Row:** status dot • label • state text • time-in-state.
- **Expiry:** removed on `SessionEnd`; otherwise marked stale after
  `staleTimeoutMin` (default 10) of silence, then dropped after a short grace.

## Defaults (all user-overridable in settings)

- Listener port: `4317`.
- Stale timeout: `10` min.
- Notifications: Red → OS notification **on**, sound **off**. Orange/Green
  silent. Fire on **state transitions only**, never while idle.
- Widget: remembers position; expanded by default; opacity adjustable.
- Launch on login: off by default.

## Suggested project structure

```
cc-beacon/
├─ src/                 # React UI (widget, settings, tray menu views)
│  ├─ widget/
│  ├─ settings/
│  ├─ state/            # session store, derivation client-side mirror
│  └─ themes/           # data-driven theme definitions
├─ src-tauri/
│  ├─ src/
│  │  ├─ listener.rs    # 127.0.0.1 HTTP server, parses hook payloads
│  │  ├─ engine.rs      # session state map, rollup, stale sweep
│  │  ├─ tray.rs        # tray icon + menu
│  │  ├─ hooks.rs       # settings.json install/uninstall (non-destructive)
│  │  └─ windows.rs     # floating widget + settings windows
│  └─ tauri.conf.json
├─ docs/SPEC.md
└─ prompts/phase-*.md
```

## Conventions

- TypeScript strict. No `any`. Functional React, hooks-based.
- State flows one way: Rust engine is source of truth → emits events to the
  webview → React renders. UI never derives state independently.
- Keep Rust surface minimal and well-commented; prefer doing logic in the engine
  so the UI stays a thin renderer.
- No browser storage APIs. Persist via `tauri-plugin-store` (JSON in app config
  dir): settings, window position, theme.

## Guardrails

- Bind the listener to `127.0.0.1` only. Reject non-loopback. Optional shared
  token header is a later hardening, not v1.
- Hooks must be `async: true` so they never slow Claude Code down. Keep the
  endpoint's response immediate.
- The hook installer must **merge** into existing `~/.claude/settings.json`,
  never overwrite the user's other hooks. Always offer a copy-paste fallback and
  a clean uninstall.
- Local only. If you ever find yourself adding a network call out, stop.

## Build phases

1. `prompts/phase-1-foundation.md` — hooks → listener → engine → tray rollup.
2. `prompts/phase-2-widget.md` — floating widget + per-session breakdown.
3. `prompts/phase-3-notifications.md` — configurable notifications + settings.
4. `prompts/phase-4-themes.md` — data-driven themes + packaging/polish.

Build phases in order. Each phase must end runnable and demoable.
