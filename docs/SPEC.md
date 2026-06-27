# Beacon (cc-beacon) — Specification

Single source of truth for requirements. `CLAUDE.md` is the condensed standing
context; this document is the full version.

## 1. Purpose

Give Claude Code users an at-a-glance, always-available signal of what each of
their sessions is doing — so they know when to step in, when to wait, and when
they can hand over a new task — without watching the terminal.

## 2. Concept

- **Tray / menu-bar icon** = rollup. One glyph answering "does anything need
  me?" across all sessions.
- **Floating widget** = detail. Always-on-top panel with one row per live
  session.
- **Settings** = configuration surface (notifications, port, theme, etc.).

## 3. State model

| State | Color | Meaning | Source event |
|---|---|---|---|
| Needs you | 🔴 Red | Claude can't proceed without you (permission, choice, answer) | `Notification` (permission_prompt / elicitation_dialog) |
| Working | 🟠 Orange | Actively running — don't interrupt | `UserPromptSubmit`, `PostToolUse` heartbeat |
| Ready | 🟢 Green | Turn finished — okay to give new instructions | `Stop`, `SubagentStop`, `SessionStart` |
| None / stale | ⚪ Grey | No live session, or session went silent | `SessionEnd`, stale timeout |

Rollup priority: Red > Orange > Green > Grey.

> **Note:** `Notification` with `notification_type = idle_prompt` is **ignored**
> (no state change). It fires when a session has merely been sitting idle, which
> does not mean Claude is blocked on you — so an idle session stays Ready (green)
> and only goes grey via the stale timeout. Only `permission_prompt` and
> `elicitation_dialog` mean "Needs you".

## 4. Detection architecture

```
Claude Code session ──(hooks, async HTTP POST)──▶ 127.0.0.1:4317/hook
                                                        │
                                                        ▼
                                              Rust listener + engine
                                          (session map keyed by session_id)
                                                        │
                                       emits state ─────┼───── computes rollup
                                                        ▼
                                          Tray icon  +  Floating widget (React)
```

- Hooks carry `session_id`, `cwd`, `hook_event_name` (+ event-specific fields).
- The engine is the single source of truth; the UI is a thin renderer fed by
  Tauri events.
- See `CLAUDE.md` → "Hook contract" for the event→state derivation rules and the
  Phase-1 verification note.

## 5. Functional requirements

### 5.1 Status engine
- Maintain per-session state from the hook stream.
- Compute the tray rollup on every change.
- Sweep stale sessions (`now − lastSeen > staleTimeoutMin`), then drop after a
  short grace.
- Resolve session label: `basename(cwd)` + branch from `<cwd>/.git/HEAD`.

### 5.2 Tray / menu-bar
- Rollup glyph reflects the priority state.
- Menu: show/hide widget · open settings · quit. A short session summary in the
  menu is a plus.

### 5.3 Floating widget
- Always-on-top, frameless, transparent, draggable; remembers position;
  multi-monitor aware.
- One row per session: status dot • label • state text • time-in-state.
- Compact mode (dots only) vs expanded (full rows); opacity control; show/hide.
- **v1 is display-only.** "Click a row to focus that terminal" is deferred —
  reliable cross-platform terminal focusing is fiddly and out of scope for v1.

### 5.4 Notifications
- Per-state toggles; sound on/off per state.
- Fire on state **transitions** only, never repeatedly while idle.
- Defaults: Red on (no sound); Orange/Green off.
- Later (not v1): "only notify if that terminal isn't focused."

### 5.5 Hook setup
- One-time installer writes the HTTP hook block into `~/.claude/settings.json`,
  merging non-destructively.
- Copy-paste fallback shown in-app.
- Clean uninstall that removes only Beacon's hook entries.

### 5.6 Themes
- A theme = an icon set + a state→appearance map, defined as data.
- Switching themes requires no code change. Ship at least two (classic traffic
  light + one alternate).

## 6. Non-functional requirements

- Cross-platform: Windows 10/11 (system tray) and macOS (menu bar) from one
  codebase.
- Tiny footprint; near-instant startup; negligible idle CPU.
- Fully local; loopback-only listener; no telemetry.
- Resilient to malformed/unknown hook payloads (ignore, don't crash).

## 7. Edge cases to handle

- Terminal closed/crashed without `SessionEnd` → stale sweep.
- Claude Code not running at all → tray grey, widget empty state.
- Multiple sessions changing state simultaneously.
- Port 4317 already in use → surface a clear error + let user change port.
- `~/.claude/settings.json` missing, malformed, or already has hooks → merge
  safely or fail loud without corrupting the file.
- Two app instances → single-instance lock.
- Notification storms on rapid transitions → debounce.

## 8. Open / deferred (not v1)

- Click-to-focus terminal.
- Focus-aware notifications.
- Shared-token auth on the listener.
- Auto-update.

## 9. Phasing

| Phase | Outcome |
|---|---|
| 1 — Foundation | Hooks → listener → engine → tray rollup. Proves the chain end-to-end. |
| 2 — Widget | Floating per-session breakdown window. |
| 3 — Notifications | Settings surface + configurable per-state notifications. |
| 4 — Themes | Data-driven themes + packaging/installers + polish. |

Each phase ends runnable and demoable.
