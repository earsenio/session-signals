# Session Signals — Specification

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
- Clicking a row focuses that session's terminal tab, via a tty captured at
  session start (shipped in 0.2.0; originally deferred past v1).
- Rows carry a per-session descriptor (Claude Code's session title, else the
  latest prompt) and a "N agents running" sub-line when subagents are active.
- Sessions hidden by a filter rule (§5.7) never appear in the list.

### 5.4 Notifications
- Per-state toggles; sound on/off per state.
- Fire on state **transitions** only, never repeatedly while idle.
- Defaults: Red on (no sound); Orange/Green off.
- Focus-aware: suppress a notification when that session's terminal is already
  focused (shipped in 0.2.0; originally deferred past v1).
- A filtered-out session (§5.7) never notifies — unless it blocks on you, which
  reveals it first.

### 5.5 Hook setup
- One-time installer writes the HTTP hook block into `~/.claude/settings.json`,
  merging non-destructively.
- Copy-paste fallback shown in-app.
- Clean uninstall that removes only Session Signals' hook entries.

### 5.6 Themes
- A theme = an icon set + a state→appearance map, defined as data.
- Switching themes requires no code change. Ship at least two (classic traffic
  light + one alternate).

### 5.7 Session filtering

Background tooling launches Claude Code headlessly (`claude --print`). Those runs
carry a normal `session_id` and no `agent_id`, so they are indistinguishable from
the user's own sessions and flood both surfaces. Filtering is the answer — and it
is **opt-in and reversible**, because a session that silently disappears is the
worst failure this app has.

**Rules.** Two data-driven matcher kinds, both case-insensitive:
`first_prompt_prefix` (anchored to the session's **opening** prompt, so merely
mentioning a phrase later never hides you) and `cwd_contains`. They live in
`config.ignore_rules`. **Ships empty — nothing is hidden until the user opts in.**

**Precedence, failing open.** `config.never_hide` uses the same two matcher
shapes but inverts the outcome: a `never_hide` match keeps a session visible
regardless of `ignore_rules`. A session matching both stays **visible**. Extra
noise is recoverable; a session you needed that vanished isn't.

**Built-in human markers.** An opening preceded by a Claude Code interaction
marker — `<command-…>`, `<local-command-…>`, `<ide_opened_file>`,
`<ide_selection>` — is never matched, never observed, and never proposed. Those
are evidence of a human at the keyboard. They are immutable by design.

**Reveal-on-block (safety valve).** A session hidden by `ignore_rules` that hits
a real block on the user — a permission prompt, a plan to approve — is un-hidden
until it restarts, notifies normally, and colours the tray. A filter is a guess;
a guess must never swallow a request for the user specifically. `SessionStart`
clears the reveal. `never_hide` is unaffected (it was never hidden).

**Observation (privacy-constrained).** With `observe_enabled` (default on), the
app reads each session's first prompt once and records a **salted hash** of it —
128-bit fingerprints at a few prefix lengths, a count, and first/last-seen
timestamps. **Never the prompt text.** Records prune after `observe_retain_days`
(default 30). Stated limitation: hashing a short, low-entropy prompt is not
anonymity against a dictionary attack; what it defeats is a readable prompt log
sitting in JSON that could be synced, backed up, or attached to a bug report.

**Proposals.** An opening seen at least `propose_threshold` times (default 3,
floored at 3 in code) becomes a suggested rule, offered one card at a time with
its sample text and the live sessions that would disappear on accept. Three
explicit actions, none automatic: **accept** (writes the `ignore_rules` entry,
idempotent), **dismiss** (this run; returns if the cluster grows), **never
suggest** (writes `never_hide` and purges the related fingerprints). A proposal
left alone changes nothing.

**Measured threshold.** A cluster's sample must be ≥ 60 characters to be
proposal-eligible (`config::MIN_PROPOSE_SAMPLE_LEN`). This is measured, not
guessed: a sweep over a 568-prompt local corpus found human/machine openings
colliding at short prefix lengths (peaking at ~a fifth of clusters at 8 chars),
dropping to zero from 57 characters onward and staying there through 120. See
[measurements.md](measurements.md).

**Surfaces.** Settings → **Session filtering** hosts both rule editors, the
read-only built-in markers, the proposal card, and an audit list of every
currently-hidden session *paired with the rule hiding it* plus the
reveal-on-block count — that audit is what makes the tray colour verifiable.
Discovery is a quiet tray line ("Session filtering: N suggestion…") that opens
Settings; it **never** changes the tray icon or colour, which encode rollup
state only.

**Behaviour of hidden sessions.** Still tracked — they simply never reach the
widget list, never colour the tray, and never notify. Remove the rule and they
reappear immediately; no state is lost.

Full user-facing guide: [IGNORING_BOT_SPAWNED_SESSIONS.md](IGNORING_BOT_SPAWNED_SESSIONS.md).

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
- Fork/resume sessions (`--fork-session --resume <parent>.jsonl`, e.g.
  computer-use automation) may emit under both the new and parent `session_id`,
  surfacing a transient duplicate "twin" row. Not suppressed while active (no
  fork linkage in the payload; process inspection is out of scope); the parent
  greys out via the stale sweep once it stops emitting.
- Widget must never get stuck on a stale snapshot: it reconciles against the
  engine's `get_snapshot` on an interval + on focus, not push events alone.
- Subagents share the parent's `session_id`, so their events must not overwrite
  the parent row's state. Events carry `agent_id` (non-null only for subagents);
  only main-agent events (`agent_id` absent) move a session into Working/Ready,
  while subagent events heartbeat + drive the "N agents running" count. A real
  block is the exception — `Notification(permission_prompt|elicitation_dialog)`
  escalates to NeedsYou regardless of `agent_id` (a subagent's permission gate
  still needs the user). Without this, a running subagent silently cleared a
  pending "Needs you."
- A session marked stale clears its subagent count, so a greyed "No response"
  row never keeps asserting "N agents running" (the matching `SubagentStop` may
  never have arrived before it went silent).
- A prompt sent as content blocks rather than a plain string must still resolve
  — a string-only reader silently blanks the opening prompt for a large share of
  sessions, disabling every `first_prompt_prefix` rule for them.
- The first-prompt read happens at `SessionStart`, before any prompt exists, so
  an empty result must be re-checked rather than latched permanently.
- A session that hits a permission prompt *before* its opening prompt is
  classified must stay visible — a late classification arriving after the block
  must not hide a session that is genuinely waiting on the user.
- `AskUserQuestion` / `ExitPlanMode` block on the user the moment they fire but
  emit no `Notification` the listener can see, so the engine escalates to
  Needs you on their `PreToolUse` and returns to Working once answered.
- An `ignore_rules` cluster that crossed the propose threshold entirely in a
  previous run surfaces only after one more matching session re-supplies the
  sample text — the count persists, the readable sample deliberately does not.

## 8. Open / deferred

Shipped since this list was first written: click-to-focus terminal, focus-aware
notifications, and shared-token auth on the listener (all 0.2.0).

Still open:

- Auto-update.
- Signed installers — CI builds are unsigned, so macOS Gatekeeper and Windows
  SmartScreen warn on first launch. See [VERSIONING.md](VERSIONING.md).
- A cross-user corpus for the §5.7 prefix measurement. The 57-character knee
  comes from a single developer's local tree; it cannot measure the
  same-polarity case (a user's own unmarked opening colliding with an unmarked
  machine one). See [measurements.md](measurements.md).
- A confirmed fast path for the first prompt. No wired hook payload carries it
  within the currently-verified schema, so the transcript-head read stays
  load-bearing; `UserPromptSubmit`'s documented `prompt` field is an unverified
  candidate that has never been captured live against this repo's listener.

## 9. Phasing

| Phase | Outcome |
|---|---|
| 1 — Foundation | Hooks → listener → engine → tray rollup. Proves the chain end-to-end. |
| 2 — Widget | Floating per-session breakdown window. |
| 3 — Notifications | Settings surface + configurable per-state notifications. |
| 4 — Themes | Data-driven themes + packaging/installers + polish. |

Each phase ends runnable and demoable. All four are complete.

**Session filtering** (§5.7) was built afterwards as its own six-phase effort —
foundation fixes, observation store, marker registry + allowlist, clustering +
proposals, settings UI, and fixtures + validation. All six are complete; phases
1–4 are committed and 5–6 are pending commit on `feat/headless-session-filter`.
