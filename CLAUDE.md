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
| Needs you | 🔴 Red | Blocked on you — permission, choice, or answer | `Notification` (type ∈ permission_prompt, elicitation_dialog) |
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
PostToolUse             → lastSeen = now (heartbeat; keep current state)
Notification:
  permission_prompt   |
  elicitation_dialog    → state = NEEDS_YOU,              lastSeen = now
  idle_prompt           → ignore (idle ≠ blocked; stays Ready until stale)
  auth_success / other  → ignore (no state change)
Stop                    → state = READY,                  lastSeen = now
SubagentStart           → subagentCount++,                lastSeen = now (no state change)
SubagentStop            → subagentCount--,                lastSeen = now (no state change)
SessionEnd              → remove session
(no event for staleTimeout) → mark stale, subagentCount = 0 → drop after grace
```

**Main agent vs. subagent (`agent_id`):** a subagent (Task tool) shares its
parent's `session_id`, so its events would otherwise mutate the parent row. Every
hook carries `agent_id` **only when emitted by a subagent** (null/absent for the
main agent). Rule: **only the main agent moves a session into WORKING/READY;
subagent events are heartbeat-only** (keep it live + drive `subagentCount`, never
recolor the row). The sole exception is a *block*: a subagent hitting a permission
gate still needs the user, so `Notification(permission_prompt|elicitation_dialog)`
escalates to NEEDS_YOU regardless of `agent_id`. This is why a session can be
RED/NEEDS_YOU with subagents running and not get cleared by their activity. The
row's color tracks the main agent; the "N agents running" sub-line tracks
subagents — fully independent. See `engine.rs` (`is_subagent`, `heartbeat`).

> ⚠️ **Verify before building (Phase 1):** confirm exact event names, the
> `Notification` payload's type field, and that `http` + `async` hooks are
> supported on the installed Claude Code version (`claude --help hooks`). Some
> events are version-gated. If `http` hooks are unavailable, fall back to a
> `command` hook that forwards stdin to the listener (e.g. a tiny bundled
> `curl`/forwarder). Do not assume the schema — read it.

> ✅ **Verified (Claude Code 2.1.195):** All seven event names are valid.
> `type: "http"` hooks are natively supported (Claude POSTs the stdin JSON to
> the URL) — **no command-hook fallback needed**. The `Notification` payload
> carries a `notification_type` field; `permission_prompt` and
> `elicitation_dialog` mean NEEDS_YOU. `idle_prompt` is **ignored** (it fires on
> mere idleness, not a real block — an idle session stays Ready/green until the
> stale sweep greys it), as are `auth_success` / `elicitation_complete`. Every
> event includes `session_id`, `cwd`,
> `transcript_path`, `hook_event_name`. HTTP hooks are non-blocking by nature
> (a non-2xx/timeout is a non-blocking error), and Beacon's listener answers
> instantly, so an explicit `async` flag is unnecessary for `http` hooks. The
> installed block uses an empty matcher (`""`) per event. See `hooks.rs`.

> ✅ **Verified (Claude Code 2.1.x) — `agent_id` distinguishes subagents:**
> empirically captured raw hook bodies (spawned an Explore subagent against the
> live listener). On a **single `session_id`**, the main agent's events carry
> `agent_id: null`, while the subagent's `PreToolUse` / `PostToolUse` /
> `PostToolBatch` / `SubagentStart` / `SubagentStop` all carry a non-null
> `agent_id` plus `agent_type` (e.g. `"Explore"`). This is the signal Beacon uses
> to stop subagent activity from overwriting the parent's traffic-light state (the
> `NEEDS_YOU`-masking bug). `HookEvent` now parses `agent_id`/`agent_type`.

## Session presentation

- **Label:** `basename(cwd)` + git branch if resolvable. Resolve branch by
  reading `<cwd>/.git/HEAD` (no subprocess); fall back to none.
- **Row:** status dot • label • state text • time-in-state.
- **Expiry:** removed on `SessionEnd`; otherwise marked stale after
  `staleTimeoutMin` (default 10) of silence, then dropped after a short grace.
- **Fork/resume duplicates:** a session launched with `--fork-session --resume
  <parent>.jsonl` (e.g. computer-use automation) can emit hook events under
  *both* the new and the parent `session_id`, so Beacon may briefly show a
  duplicate "twin" row for the parent. This is a byproduct of forking, not a
  bug: the hook payload carries no fork/parent linkage, and detecting it would
  require process inspection (a locked-out decision), so we don't suppress it
  while active. Once the fork stops emitting, the parent greys out via the
  normal stale sweep. Ordinary terminal sessions don't fork and never duplicate.

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
- Versioning: SemVer, releases-only. `package.json` is the single source of
  truth; bump via `npm run release:{patch,minor,major}`. Never hand-edit the
  version in `tauri.conf.json` / `Cargo.toml`. See `docs/VERSIONING.md`.

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
