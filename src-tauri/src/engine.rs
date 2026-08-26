//! State engine: the single source of truth for session status.
//!
//! Sessions are keyed by `session_id`. Hook events mutate per-session state
//! following the derivation rules in CLAUDE.md. The engine also computes the
//! tray rollup and sweeps stale (silent) sessions. It holds no Tauri handles —
//! `lib.rs` owns it behind a `Mutex` and reacts to changes by refreshing the
//! tray and emitting to the webview. The UI never derives state itself.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

/// Per-session status. Maps to the traffic-light colors in the spec.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// 🔴 Blocked on the user (permission / choice / answer).
    NeedsYou,
    /// 🟠 Actively running.
    Working,
    /// 🟢 Finished its turn — okay to give new instructions.
    Ready,
}

/// The tray rollup across all live sessions.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Rollup {
    Red,
    Orange,
    Green,
    Grey,
}

/// One live subagent under a session.
///
/// Claude Code gives us the agent's identity and type on `SubagentStart`
/// (`agent_id`, `agent_type`), but **not** what it was asked to do — that text
/// only ever appears in the *main* agent's `PreToolUse` for the `Agent` tool.
/// Verified empirically against Claude Code 2.1.x: `SubagentStart` carries
/// exactly `agent_id`, `agent_type`, `prompt_id`, `session_id`, `cwd`,
/// `transcript_path`, and nothing that joins back to that `PreToolUse` —
/// `agent_id` is not the tool call's `tool_use_id`, and `prompt_id` is shared by
/// every agent in the turn. Hence `Session::pending_agents` and the FIFO pairing
/// in the `SubagentStart` arm.
#[derive(Clone, Debug)]
struct AgentRun {
    /// `agent_id` from the hook. Unique per running agent; used to remove the
    /// right one on `SubagentStop`.
    id: String,
    /// e.g. "Explore", "Plan". `None` if the hook omitted it.
    agent_type: Option<String>,
    /// The task's short description, paired over from `pending_agents`. `None`
    /// when no unclaimed description matched (see the pairing caveat there).
    description: Option<String>,
    /// When this agent started — anchors its own elapsed timer.
    started: Instant,
}

/// A description from `PreToolUse(Agent)` waiting for its `SubagentStart`.
#[derive(Clone, Debug)]
struct PendingAgent {
    /// The `subagent_type` requested in the tool call; matched against the
    /// `agent_type` reported by `SubagentStart`.
    subagent_type: Option<String>,
    description: Option<String>,
}

/// Cap on unclaimed descriptions. A `PreToolUse(Agent)` whose `SubagentStart`
/// never arrives (denied by a permission gate, or the turn was interrupted)
/// would otherwise sit in the queue forever; the oldest is dropped past this.
const MAX_PENDING_AGENTS: usize = 32;

/// A single tracked Claude Code session.
#[derive(Clone, Debug)]
struct Session {
    cwd: String,
    state: State,
    /// Last time we heard anything from this session.
    last_seen: Instant,
    /// When the current `state` was entered (for time-in-state display).
    state_since: Instant,
    /// True once it has gone silent past the stale timeout (display grey,
    /// excluded from rollup) but before the grace drop.
    stale: bool,
    /// Live subagents fanned out from this session, in start order: pushed on
    /// `SubagentStart`, removed by `agent_id` on `SubagentStop`. Drives the
    /// row's per-agent sub-lines independently of `state`. Its length is the
    /// old "N agents running" count — there is no separate counter, so the two
    /// can never disagree.
    agents: Vec<AgentRun>,
    /// Task descriptions from the main agent's `PreToolUse(Agent)` that have not
    /// yet been claimed by a `SubagentStart`. See `AgentRun::description` for
    /// why this queue exists at all.
    pending_agents: VecDeque<PendingAgent>,
    /// Descriptions of agents that have already run, keyed by `agent_id`.
    /// A resumed agent — one messaged after it finished, or waking to report a
    /// background task — emits a *second* `SubagentStart` with no fresh
    /// `PreToolUse` behind it (observed live). Without this it would come back
    /// as an unlabelled row. Bounded like `pending_agents`.
    known_agents: VecDeque<(String, Option<String>)>,
    /// When the agent list last became non-empty — the anchor for the aggregate
    /// elapsed timer. `None` whenever no agents are running.
    sub_since: Option<Instant>,
    /// The main agent's turn ended (`Stop`) while subagents were still running,
    /// so the row is held at `Working` until the last one finishes. Cleared when
    /// that happens, and whenever a new turn takes ownership of the row.
    pending_ready: bool,
    /// PID of the terminal *application* hosting this session, captured by the
    /// `SessionStart` command hook (see `capture.rs`). Drives click-to-focus and
    /// focus-aware notifications. `None` until/unless capture resolves it.
    terminal_pid: Option<i32>,
    /// Human name of that terminal app (e.g. "iTerm2", "WindowsTerminal.exe"),
    /// for display/debugging. Best-effort.
    terminal_app: Option<String>,
    /// Controlling tty of this session (e.g. "/dev/ttys003"), captured by walking
    /// the parent chain. Lets `focus.rs` select the exact tab/window on macOS
    /// terminals that expose a per-tab tty (Terminal.app, iTerm2), rather than
    /// only raising the app. `None` on Windows / when unresolved.
    terminal_tty: Option<String>,
    /// Repo root name for `cwd` — the **main** repo's name when `cwd` is a
    /// linked worktree. `None` when the session isn't in a repo, or before the
    /// capture hook has reported (fall back to `fallback_basename(cwd)`).
    ///
    /// Reported *to* us by the capture hook rather than read from disk here, and
    /// that's load-bearing, not incidental: resolving it in-process meant opening
    /// paths under an arbitrary user directory, and macOS gates those per
    /// protected category (Desktop / Documents / Downloads / network volumes are
    /// each a separate TCC grant), so every session in a not-yet-granted folder
    /// popped a permission prompt. See `capture.rs`. Don't reintroduce a
    /// filesystem read here.
    git_base: Option<String>,
    /// Current branch, or `None` when detached / not a repo / not yet reported.
    git_branch: Option<String>,
    /// True when `cwd` is a linked `git worktree` (not a submodule).
    git_worktree: bool,
    /// Short human descriptor of what this session is about — Claude Code's own
    /// generated session title (the `ai-title` in the transcript), falling back
    /// to the first human prompt. Derived locally from `transcript_path` by
    /// `descriptor::extract` and cached here so `snapshot` never does file I/O.
    /// `None` until the transcript yields one (e.g. a brand-new session).
    descriptor: Option<String>,
    /// When we last *attempted* to (re)derive `descriptor`. Debounces the
    /// transcript read so we don't re-scan the file on every hook event.
    descriptor_checked_at: Option<Instant>,
}

/// The part of an `Agent`/`Task` tool call we surface.
///
/// **Deliberately has no `prompt` field.** The real `tool_input` is
/// `{description, prompt, subagent_type}`, and `prompt` is the entire task text
/// — often the contents of files, or whatever the user just asked for. Omitting
/// the field means serde drops it during parsing and it is never allocated, let
/// alone stored or shipped to the webview: the privacy guarantee is structural
/// rather than a filter somebody has to remember to apply. Don't add it.
#[derive(Debug, serde::Deserialize, Default, Clone)]
pub struct ToolInput {
    /// The spawner's own short summary of the task ("Map subagent UI in widget").
    #[serde(default)]
    pub description: Option<String>,
    /// The requested agent type ("Explore", "Plan", ...).
    #[serde(default)]
    pub subagent_type: Option<String>,
}

/// Parsed, transport-agnostic hook event. The listener deserializes the raw
/// JSON into this; the engine never sees HTTP.
#[derive(Debug, serde::Deserialize, Default)]
pub struct HookEvent {
    pub hook_event_name: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
    /// Absolute path to this session's transcript JSONL. Present on every real
    /// Claude Code hook event (not on the synthetic `BeaconTerminal`). The source
    /// for the session descriptor — see `descriptor::extract`.
    #[serde(default)]
    pub transcript_path: Option<String>,
    /// Present only on `Notification` events.
    #[serde(default)]
    pub notification_type: Option<String>,
    /// Identifies the agent that emitted the event. **Present (non-null) only on
    /// subagent events; null/absent on the main agent.** A subagent shares its
    /// parent's `session_id`, so this is the only signal that an event came from
    /// a fanned-out subagent rather than the main agent. Verified empirically on
    /// Claude Code 2.1.x (subagent `PreToolUse`/`PostToolUse`/`SubagentStart`/
    /// `SubagentStop` all carry it; main-agent events do not). Used to keep
    /// subagent activity from overwriting the session's traffic-light state.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// The subagent's type (e.g. "Explore"), present alongside `agent_id`.
    /// Captured for display/debugging; not currently used for state logic.
    #[serde(default)]
    pub agent_type: Option<String>,
    /// The tool being invoked, present on `PreToolUse` / `PostToolUse`. Drives
    /// detection of user-blocking tools (`AskUserQuestion`, `ExitPlanMode`) that
    /// halt on the user the instant they start yet emit no `Notification` the
    /// listener can see — so the session must be read as NeedsYou, not Working,
    /// while one is pending. See `is_blocking_tool` and the `PreToolUse` /
    /// `PostToolUse` arms in `apply`.
    #[serde(default)]
    pub tool_name: Option<String>,
    /// Present on `PreToolUse`/`PostToolUse`. Only parsed for the `Agent`/`Task`
    /// tool, and only for the two fields on [`ToolInput`].
    #[serde(default)]
    pub tool_input: Option<ToolInput>,
    /// Present only on the synthetic `BeaconTerminal` event from the capture
    /// hook: the owning terminal app's pid and name.
    #[serde(default)]
    pub terminal_pid: Option<i32>,
    #[serde(default)]
    pub terminal_app: Option<String>,
    /// Present only on `BeaconTerminal`: the session's controlling tty.
    #[serde(default)]
    pub terminal_tty: Option<String>,
    /// Capture-script protocol version. `>= 2` means the git fields below are
    /// authoritative and replace whatever we hold. Absent means an older script
    /// that reports no git facts at all — it must not wipe what we already have.
    #[serde(default)]
    pub capture_version: Option<u32>,
    /// `"full"` (SessionStart) or `"turn"` (Stop). A `"turn"` report may not
    /// create a session row: it can land after `SessionEnd` and would otherwise
    /// resurrect a dead session. See the `BeaconTerminal` arm in `apply`.
    #[serde(default)]
    pub capture_mode: Option<String>,
    /// Repo root name for the session's cwd (main repo's name for a worktree),
    /// empty when it isn't a repo. Resolved by the capture hook — see the
    /// `git_base` field on `Session` for why it isn't read here.
    #[serde(default)]
    pub git_base: Option<String>,
    /// Current branch, empty when detached or not a repo.
    #[serde(default)]
    pub git_branch: Option<String>,
    /// Whether the cwd is a linked git worktree.
    #[serde(default)]
    pub git_worktree: Option<bool>,
}

/// A state change for one session, reported by `apply` so the notification
/// engine can react. `from` is `None` when the session is brand new.
#[derive(Serialize, Clone, Debug)]
pub struct Transition {
    pub session_id: String,
    pub label: String,
    /// The label's folder part alone (no branch) — what notifications title
    /// with, shipped separately so nothing re-parses the combined label.
    pub folder: String,
    pub from: Option<State>,
    pub to: State,
    /// The session's captured terminal pid at transition time, if known. Lets
    /// the notifier suppress alerts when that terminal is already frontmost.
    pub terminal_pid: Option<i32>,
}

/// Result of applying one hook event.
pub struct ApplyOutcome {
    /// True if the rollup / session list may have changed (worth a UI refresh).
    pub changed: bool,
    /// Present only when a session actually moved to a new state.
    pub transition: Option<Transition>,
}

/// Result of a stale sweep.
pub struct SweepOutcome {
    pub changed: bool,
    /// Sessions that newly went stale this sweep: `(session_id, label)`.
    pub went_stale: Vec<(String, String)>,
}

/// One running subagent, as shown on a widget row's sub-line.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct AgentView {
    pub agent_id: String,
    /// e.g. "Explore". `None` if the hook didn't report one.
    pub agent_type: Option<String>,
    /// What this agent was asked to do. `None` when it couldn't be paired to a
    /// spawn — the row falls back to showing the type alone.
    pub description: Option<String>,
    /// Seconds this agent has been running.
    pub seconds: u64,
}

/// A flattened, serializable view of one session for the webview / tray menu.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SessionView {
    pub session_id: String,
    /// Combined one-line label (`folder (branch)` or `folder`) — used for
    /// sorting and plain-text surfaces (tray tooltips, notifications).
    pub label: String,
    /// The label's structured parts, so the widget's two-tone row never has to
    /// re-parse `label` (a folder literally named `foo (bar)` would misparse).
    pub folder: String,
    pub branch: Option<String>,
    /// True when the session's cwd is a linked git worktree. The UI shows a
    /// subtle marker so a worktree session is distinguishable from a checkout of
    /// the same repo. Display-only.
    pub worktree: bool,
    pub state: State,
    pub stale: bool,
    /// Seconds the session has been in its current state.
    pub seconds_in_state: u64,
    /// Live subagents running under this session (`SubagentStart` − `SubagentStop`).
    /// Always equal to `agents.len()`; kept as its own field so the UI can gate
    /// the halo without walking the list.
    pub subagent_count: u32,
    /// Seconds since the subagent count rose from 0 (0 when none are running).
    pub subagent_seconds: u64,
    /// One entry per running subagent, in start order.
    pub agents: Vec<AgentView>,
    /// Whether Session Signals resolved the owning terminal window — gates the widget's
    /// click-to-focus affordance (no handle ⇒ no focus button).
    pub can_focus: bool,
    /// Short human descriptor of what the session is about (Claude Code's own
    /// session title, else the first prompt). `None` until derivable. Display-only.
    pub descriptor: Option<String>,
}

/// A terminal handle remembered across a Session Signals restart. Capture lives only in
/// memory and only fires on `SessionStart` (see `capture.rs`), so a restart
/// would otherwise lose click-to-focus for every already-running session until
/// it happens to start a new turn. `lib.rs` persists these to the store and
/// seeds them back in here at startup; they are a *side table* — they attach to
/// a session only when a real hook event (re)creates its row, and never conjure
/// a row on their own.
/// Fields are individually defaulted so entries written by an older build
/// (which stored only the terminal handle) still deserialize.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedTerminal {
    #[serde(default)]
    pub pid: Option<i32>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub tty: Option<String>,
    /// Repo root name, remembered so a Session Signals restart shows the right
    /// label immediately instead of falling back to `basename(cwd)` until the
    /// session's next turn.
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub worktree: bool,
}

/// How long after `SessionEnd` a *heartbeat* for that session id is ignored
/// instead of resurrecting the row. Subagent stragglers (`PostToolUse`,
/// `PostToolBatch`, `SubagentStop`) can arrive moments after the main agent
/// ends the session; without this they'd recreate it as Working until the
/// stale sweep. Real restarts are unaffected: state-setting events
/// (`SessionStart`, `UserPromptSubmit`, …) clear the tombstone.
const END_TOMBSTONE: Duration = Duration::from_secs(30);

pub struct Engine {
    sessions: HashMap<String, Session>,
    /// Remembered terminal handles keyed by `session_id`, rehydrated at startup.
    /// Consulted when a session is (re)created so a restart keeps click-to-focus;
    /// never iterated to build the session list (it cannot create rows).
    pending_captures: HashMap<String, CapturedTerminal>,
    /// Recently-ended session ids (`SessionEnd` time), so straggler heartbeats
    /// can't resurrect them. Entries expire after [`END_TOMBSTONE`]; pruned on
    /// sweep.
    recent_ends: HashMap<String, Instant>,
    stale_timeout: Duration,
    /// Total silence before a session is removed from the list entirely. An
    /// idle session is visibly greyed ("No response") for the whole window
    /// between `stale_timeout` and this; only then is it dropped. Removal is
    /// otherwise driven by an explicit `SessionEnd`. Kept long (config default
    /// 60 min) so an idle session persists rather than blinking out, while a
    /// terminal killed without firing `SessionEnd` still eventually self-clears.
    drop_timeout: Duration,
}

impl Engine {
    pub fn new(stale_timeout: Duration, drop_timeout: Duration) -> Self {
        Engine {
            sessions: HashMap::new(),
            pending_captures: HashMap::new(),
            recent_ends: HashMap::new(),
            stale_timeout,
            drop_timeout,
        }
    }

    /// Apply a hook event. Reports whether a UI refresh is worthwhile and, if a
    /// session actually changed state, the transition (for notifications).
    pub fn apply(&mut self, ev: &HookEvent) -> ApplyOutcome {
        // An empty session_id is unusable as a key; ignore but don't crash.
        if ev.session_id.is_empty() && ev.hook_event_name != "SessionEnd" {
            return ApplyOutcome {
                changed: false,
                transition: None,
            };
        }

        // A subagent (Task tool) shares its parent's `session_id`, so without
        // this its events would mutate the parent row's traffic-light state — a
        // running subagent could clear a real "Needs you" (verified: subagent
        // events carry `agent_id`, the main agent's do not). The rule: only the
        // MAIN agent moves a session into Working/Ready; subagent events are
        // heartbeat-only (they keep the session live + drive the subagent count,
        // but never change its color). A genuine *block* is the one exception —
        // see `Notification` below — because a subagent hitting a permission gate
        // still needs the user, so it must be allowed to escalate to NeedsYou.
        let is_subagent = ev.agent_id.is_some();

        match ev.hook_event_name.as_str() {
            // A fresh (or resumed) session: clear any leftover subagent count so a
            // restart never inherits a stale "N agents running" sub-line. (Main
            // agent only; a subagent never legitimately starts a session — if one
            // somehow does, treat it as a heartbeat, not a reset.)
            "SessionStart" => {
                if is_subagent {
                    self.heartbeat(ev)
                } else {
                    let out = self.transition_to(ev, State::Ready);
                    self.reset_subagents(&ev.session_id);
                    out
                }
            }
            // Any work-start signal means the session is actively running. We
            // bracket "Working" between these and the terminal events below, so
            // activity that doesn't begin with a typed prompt — slash-command
            // expansion, a tool call, context compaction — still shows up.
            // (`/compact` fires PreCompact, never UserPromptSubmit, which is why
            // it used to stay green.) A subagent's tool call must NOT flip the
            // parent's color, so it heartbeats instead.
            "UserPromptSubmit" | "UserPromptExpansion" | "PreCompact" => {
                if is_subagent {
                    self.heartbeat(ev)
                } else {
                    // A new turn owns the row now. Drop any deferred Ready left
                    // over from the previous turn, or an agent of that turn
                    // finishing mid-way through this one would yank the session
                    // to green while it is actively working.
                    let out = self.transition_to(ev, State::Working);
                    if let Some(s) = self.sessions.get_mut(&ev.session_id) {
                        s.pending_ready = false;
                    }
                    out
                }
            }
            // A tool call is starting. Most tools mean the agent is actively
            // running (→ Working), but the *blocking* tools — `AskUserQuestion`,
            // `ExitPlanMode` — halt on the user the moment they fire and emit no
            // Notification the listener sees, so the session sits silently at
            // Working ("orange") while it's really blocked. Escalate those to
            // NeedsYou. Like a permission gate (see `Notification`), a block is a
            // block regardless of `is_subagent`. Their `PostToolUse` (the user
            // answered) resumes Working below.
            "PreToolUse" => {
                if is_blocking_tool(ev.tool_name.as_deref()) {
                    self.transition_to(ev, State::NeedsYou)
                } else if is_subagent {
                    self.heartbeat(ev)
                } else {
                    // The main agent is spawning a subagent. This is the ONLY
                    // event that carries what the agent was asked to do, so
                    // stash it for the `SubagentStart` that follows.
                    let out = self.transition_to(ev, State::Working);
                    if is_agent_tool(ev.tool_name.as_deref()) {
                        self.queue_agent_description(ev);
                    }
                    out
                }
            }
            // A spawned subagent: record it (the first one anchors the
            // aggregate elapsed timer). This does NOT change the session's
            // state — the main agent's own `PreToolUse` for the `Agent` tool
            // already moved it to Working; the list drives the independent
            // per-agent sub-lines.
            "SubagentStart" => {
                let out = self.heartbeat(ev);
                let now = Instant::now();
                if let Some(s) = self.sessions.get_mut(&ev.session_id) {
                    if s.agents.is_empty() {
                        s.sub_since = Some(now);
                    }
                    // A resumed agent keeps the label it had the first time
                    // round; only a genuinely new one consumes a pending spawn.
                    let description = recall_description(&s.known_agents, ev.agent_id.as_deref())
                        .unwrap_or_else(|| {
                            claim_description(&mut s.pending_agents, ev.agent_type.as_deref())
                        });
                    s.agents.push(AgentRun {
                        id: ev.agent_id.clone().unwrap_or_default(),
                        agent_type: ev.agent_type.clone(),
                        description,
                        started: now,
                    });
                }
                out
            }
            // Heartbeat: keep current state, just refresh last_seen. Also the
            // landing spot for any main-agent work-start / terminal event that
            // arrived from a subagent (`is_subagent`) — those keep the session
            // live without recoloring it.
            // A blocking tool returning means the user just answered (chose an
            // option / approved the plan) and the MAIN agent resumes work — flip
            // the row off NeedsYou back to Working. Every other PostToolUse is a
            // plain heartbeat (keep the current state). A subagent's PostToolUse
            // never recolors the parent (it can't own a blocking tool anyway).
            "PostToolUse" => {
                if !is_subagent && is_blocking_tool(ev.tool_name.as_deref()) {
                    self.transition_to(ev, State::Working)
                } else {
                    self.heartbeat(ev)
                }
            }
            "PostToolUseFailure" | "PostToolBatch" => self.heartbeat(ev),
            "Notification" => match ev.notification_type.as_deref() {
                // Only a genuine block on the user is "Needs you". This is the one
                // state a subagent IS allowed to set: if a fanned-out subagent hits
                // a permission gate, the user still has to act, so we escalate to
                // NeedsYou regardless of `is_subagent`.
                Some("permission_prompt") | Some("elicitation_dialog") => {
                    self.transition_to(ev, State::NeedsYou)
                }
                // `idle_prompt` fires when a session has merely been sitting
                // idle — it is NOT blocked on the user. Leave its state alone
                // (a finished turn stays Ready/green; a pending permission stays
                // red); the stale sweep greys it out after the timeout.
                // auth_success, elicitation_complete, etc. are likewise ignored.
                _ => ApplyOutcome {
                    changed: false,
                    transition: None,
                },
            },
            // Terminal: the turn (or compaction) ended. `PostCompact` returns a
            // standalone `/compact` to Ready; mid-turn it briefly shows Ready
            // until the next work event flips it back (self-healing).
            // `StopFailure` is a turn ended by an API error. Only the MAIN agent's
            // turn ending means the row is Ready — a subagent's `Stop` must not
            // clear the parent's state (esp. a pending "Needs you").
            "Stop" | "StopFailure" | "PostCompact" => {
                if is_subagent {
                    self.heartbeat(ev)
                } else if self.has_live_agents(&ev.session_id) {
                    // The main agent's turn ended but its subagents are still
                    // running — and they run in the background by default, so
                    // this is the *common* fan-out case, not an edge one.
                    // Calling the session Ready here would say "finished, your
                    // turn" (and fire that notification) while the work is
                    // still going. Hold it at Working; the last `SubagentStop`
                    // below releases it.
                    let out = self.transition_to(ev, State::Working);
                    if let Some(s) = self.sessions.get_mut(&ev.session_id) {
                        s.pending_ready = true;
                    }
                    out
                } else {
                    self.transition_to(ev, State::Ready)
                }
            }
            // A subagent finished: drop it from the list (by `agent_id`, which
            // this event does carry), and when the last one leaves, drop the
            // elapsed anchor so the sub-lines disappear.
            //
            // This still does NOT move a session to Ready on its own — that
            // previously flipped a still-working (or still-blocked) parent to
            // green the instant *any* subagent stopped. The one exception is a
            // Ready deferred by the `Stop` arm above: the main agent already
            // finished and was only waiting on these agents, so the last one
            // leaving is exactly the moment the session really is ready.
            "SubagentStop" => {
                let out = self.heartbeat(ev);
                let release = match self.sessions.get_mut(&ev.session_id) {
                    Some(s) => {
                        if let Some(done) = remove_agent(&mut s.agents, ev.agent_id.as_deref()) {
                            remember_agent(&mut s.known_agents, done);
                        }
                        if s.agents.is_empty() {
                            s.sub_since = None;
                            // Nothing left to wait for; the queue can only hold
                            // spawns that never started.
                            s.pending_agents.clear();
                            std::mem::take(&mut s.pending_ready)
                        } else {
                            false
                        }
                    }
                    None => false,
                };
                if release {
                    self.transition_to(ev, State::Ready)
                } else {
                    out
                }
            }
            // Synthetic event from the terminal-capture hook: record which
            // terminal owns this session. No state change — a session can be in
            // any color and still get (or refresh) its terminal mapping. Creates
            // the session if it raced ahead of SessionStart so the pid isn't lost.
            "BeaconTerminal" => {
                let now = Instant::now();
                // A per-turn report belongs to a session that is already live, so
                // it may only update an existing row. Creating one would let a
                // report that lost the race with `SessionEnd` resurrect a dead
                // session — the same hazard `recent_ends` guards for heartbeats,
                // which applies here too now that capture fires every turn.
                //
                // A *full* report accompanies `SessionStart`, so it may create one
                // even for an id that was just ended: `claude --resume <id>` reuses
                // the session id, and that row is legitimately starting again. (An
                // end-tombstone here would silently drop the terminal handle for
                // the resumed run.)
                let known = self.sessions.contains_key(&ev.session_id);
                let may_create = ev.capture_mode.as_deref() != Some("turn");
                if !known && !may_create {
                    return ApplyOutcome {
                        changed: false,
                        transition: None,
                    };
                }
                let s = self
                    .sessions
                    .entry(ev.session_id.clone())
                    .or_insert_with(|| Session {
                        cwd: ev.cwd.clone(),
                        state: State::Ready,
                        last_seen: now,
                        state_since: now,
                        stale: false,
                        agents: Vec::new(),
                        pending_agents: VecDeque::new(),
                        known_agents: VecDeque::new(),
                        sub_since: None,
                        pending_ready: false,
                        terminal_pid: None,
                        terminal_app: None,
                        terminal_tty: None,
                        git_base: None,
                        git_branch: None,
                        git_worktree: false,
                        descriptor: None,
                        descriptor_checked_at: None,
                    });
                if ev.terminal_pid.is_some() {
                    s.terminal_pid = ev.terminal_pid;
                }
                if ev.terminal_app.is_some() {
                    s.terminal_app = ev.terminal_app.clone();
                }
                if ev.terminal_tty.as_deref().is_some_and(|t| !t.is_empty()) {
                    s.terminal_tty = ev.terminal_tty.clone();
                }
                // Git facts are reported wholesale, so a v2 report REPLACES them:
                // checking out a detached HEAD has to clear the branch, not leave
                // a stale one on the row. A pre-v2 script reports none and must
                // leave ours alone.
                if ev.capture_version.unwrap_or(0) >= 2 {
                    s.git_base = ev.git_base.clone().filter(|v| !v.is_empty());
                    s.git_branch = ev.git_branch.clone().filter(|v| !v.is_empty());
                    s.git_worktree = ev.git_worktree.unwrap_or(false);
                }
                s.last_seen = now;
                // Only adopt the capture's cwd when we have none. The script
                // lifts it out of the raw stdin JSON with `sed`, which can't
                // unescape a path containing `"` or `\`; the real hooks parse
                // properly, so theirs always wins. See `capture.rs`'s
                // `quotes_and_backslashes_in_cwd_stay_valid_json`.
                if s.cwd.is_empty() && !ev.cwd.is_empty() {
                    s.cwd = ev.cwd.clone();
                }
                ApplyOutcome {
                    changed: true,
                    transition: None,
                }
            }
            "SessionEnd" => {
                // The terminal is gone; forget its remembered handle too so a
                // stale entry can't outlive the session in the persisted store.
                self.pending_captures.remove(&ev.session_id);
                // Tombstone the id briefly so straggler heartbeats (a subagent's
                // late PostToolUse) can't resurrect the row — see `heartbeat`.
                self.recent_ends
                    .insert(ev.session_id.clone(), Instant::now());
                ApplyOutcome {
                    changed: self.sessions.remove(&ev.session_id).is_some(),
                    transition: None,
                }
            }
            // Unknown / unhandled event: ignore.
            _ => ApplyOutcome {
                changed: false,
                transition: None,
            },
        }
    }

    /// Refresh a session's liveness (`last_seen`, un-stale, `cwd`) WITHOUT
    /// changing its traffic-light state. This is the heartbeat used by
    /// `PostToolUse` and by every subagent event — the latter must keep the
    /// session alive and feed the subagent count without recoloring the row. If
    /// the session is unknown (we never saw it start), fall back to creating it
    /// as Working via `transition_to`, which also rehydrates any remembered
    /// terminal capture — unless the id was just `SessionEnd`ed, in which case
    /// the event is a straggler and is dropped rather than resurrecting the row.
    fn heartbeat(&mut self, ev: &HookEvent) -> ApplyOutcome {
        if let Some(s) = self.sessions.get_mut(&ev.session_id) {
            s.last_seen = Instant::now();
            s.stale = false;
            if !ev.cwd.is_empty() {
                s.cwd = ev.cwd.clone();
            }
            ApplyOutcome {
                changed: true,
                transition: None,
            }
        } else if self
            .recent_ends
            .get(&ev.session_id)
            .is_some_and(|t| t.elapsed() < END_TOMBSTONE)
        {
            ApplyOutcome {
                changed: false,
                transition: None,
            }
        } else {
            self.transition_to(ev, State::Working)
        }
    }

    /// Upsert the session and move it to `state`, refreshing timers. Returns a
    /// transition only when the state actually changed (or the session is new),
    /// so callers/notifications never fire on a same-state repeat.
    fn transition_to(&mut self, ev: &HookEvent, state: State) -> ApplyOutcome {
        let now = Instant::now();
        // A state-setting event is real activity — a genuine restart/resume of
        // this id, never a straggler — so any end-tombstone is void.
        self.recent_ends.remove(&ev.session_id);
        // A terminal handle remembered across a restart (seeded at startup). It's
        // attached only when this session's row is (re)created or is still
        // missing a handle — so a Session Signals restart keeps click-to-focus for
        // already-running sessions, which never re-fire `SessionStart`.
        let remembered = self.pending_captures.get(&ev.session_id).cloned();
        // `from`: Some(prev) on a real change, None on a same-state repeat. The
        // label parts ride along so the notification below can be built without
        // re-borrowing the session (and, since they're plain cached scalars,
        // without touching the filesystem).
        let (from, cwd, terminal_pid, base, branch) =
            match self.sessions.entry(ev.session_id.clone()) {
                std::collections::hash_map::Entry::Occupied(mut o) => {
                    let s = o.get_mut();
                    let prev = s.state;
                    let changed_state = prev != state;
                    if changed_state {
                        s.state = state;
                        s.state_since = now;
                    }
                    s.last_seen = now;
                    s.stale = false;
                    if !ev.cwd.is_empty() {
                        s.cwd = ev.cwd.clone();
                    }
                    // Backfill a remembered handle if this row never resolved one.
                    if s.terminal_pid.is_none() {
                        if let Some(cap) = &remembered {
                            s.terminal_pid = cap.pid;
                            s.terminal_app = cap.app.clone();
                            s.terminal_tty = cap.tty.clone();
                        }
                    }
                    // Likewise the remembered git identity: without this, a row
                    // recreated after a Session Signals restart would sit on
                    // `basename(cwd)` with no branch until the session's next turn.
                    if s.git_base.is_none() {
                        if let Some(cap) = &remembered {
                            s.git_base = cap.base.clone();
                            s.git_branch = cap.branch.clone();
                            s.git_worktree = cap.worktree;
                        }
                    }
                    (
                        if changed_state {
                            Some(Some(prev))
                        } else {
                            None
                        },
                        s.cwd.clone(),
                        s.terminal_pid,
                        s.git_base.clone(),
                        s.git_branch.clone(),
                    )
                }
                std::collections::hash_map::Entry::Vacant(v) => {
                    let cwd = ev.cwd.clone();
                    let cap = remembered.unwrap_or_default();
                    let pid = cap.pid;
                    let base = cap.base.clone();
                    let branch = cap.branch.clone();
                    v.insert(Session {
                        cwd: cwd.clone(),
                        state,
                        last_seen: now,
                        state_since: now,
                        stale: false,
                        agents: Vec::new(),
                        pending_agents: VecDeque::new(),
                        known_agents: VecDeque::new(),
                        sub_since: None,
                        pending_ready: false,
                        terminal_pid: cap.pid,
                        terminal_app: cap.app,
                        terminal_tty: cap.tty,
                        git_base: cap.base,
                        git_branch: cap.branch,
                        git_worktree: cap.worktree,
                        descriptor: None,
                        descriptor_checked_at: None,
                    });
                    (Some(None), cwd, pid, base, branch)
                }
            };

        let transition = from.map(|prev| {
            let (folder, branch) = label_parts(&cwd, base.as_deref(), branch.as_deref());
            Transition {
                session_id: ev.session_id.clone(),
                label: combine_label(folder.clone(), branch.as_deref()),
                folder,
                from: prev,
                to: state,
                terminal_pid,
            }
        });
        ApplyOutcome {
            changed: true,
            transition,
        }
    }

    /// Clear a session's live subagents, unclaimed spawns, elapsed anchor and
    /// any deferred Ready. Used on `SessionStart` so a (re)started session never
    /// carries a stale sub-line. A no-op if the session isn't tracked yet.
    fn reset_subagents(&mut self, id: &str) {
        if let Some(s) = self.sessions.get_mut(id) {
            s.agents.clear();
            s.pending_agents.clear();
            s.known_agents.clear();
            s.sub_since = None;
            s.pending_ready = false;
        }
    }

    /// Does this session have at least one subagent still running?
    fn has_live_agents(&self, id: &str) -> bool {
        self.sessions.get(id).is_some_and(|s| !s.agents.is_empty())
    }

    /// Stash the description from a main-agent `PreToolUse(Agent)` until the
    /// matching `SubagentStart` arrives. Bounded — see `MAX_PENDING_AGENTS`.
    fn queue_agent_description(&mut self, ev: &HookEvent) {
        let Some(input) = ev.tool_input.as_ref() else {
            return;
        };
        if input.description.is_none() && input.subagent_type.is_none() {
            return;
        }
        if let Some(s) = self.sessions.get_mut(&ev.session_id) {
            if s.pending_agents.len() >= MAX_PENDING_AGENTS {
                s.pending_agents.pop_front();
            }
            s.pending_agents.push_back(PendingAgent {
                subagent_type: input.subagent_type.clone(),
                description: input.description.clone(),
            });
        }
    }

    /// Seed a remembered terminal handle at startup (from the persisted store).
    /// It will be attached to the session the moment a real hook event recreates
    /// its row — never on its own, so this can't conjure a phantom session.
    /// Ignored if it carries no pid (nothing to focus).
    pub fn seed_capture(&mut self, session_id: String, cap: CapturedTerminal) {
        // Worth keeping if it carries either a focusable handle or a git identity
        // to label the row with.
        if cap.pid.is_some() || cap.base.is_some() {
            self.pending_captures.insert(session_id, cap);
        }
    }

    /// The captured terminal pid for a session, if Session Signals resolved one. Used by
    /// the click-to-focus command to know which window to raise.
    pub fn terminal_pid(&self, id: &str) -> Option<i32> {
        self.sessions.get(id).and_then(|s| s.terminal_pid)
    }

    /// The full focus target for a session: `(pid, tty, app)`. The tty + app let
    /// `focus.rs` select the exact tab on macOS terminals; the pid is the
    /// app-level fallback. `None` until Session Signals captured at least a pid.
    pub fn focus_target(&self, id: &str) -> Option<(i32, Option<String>, Option<String>)> {
        self.sessions.get(id).and_then(|s| {
            s.terminal_pid
                .map(|p| (p, s.terminal_tty.clone(), s.terminal_app.clone()))
        })
    }

    /// Whether the session's descriptor is worth (re)deriving from its transcript
    /// now. The caller does the (off-lock) file read only when this says so,
    /// keeping transcript I/O off the hot path. While we still have *no*
    /// descriptor we retry on the shorter `retry` cadence (so the title shows up
    /// soon after Claude Code writes it); once one is resolved we only re-check on
    /// the longer `refresh` cadence (it rarely changes). `None` if the session is
    /// gone or has never been checked (→ due immediately).
    pub fn descriptor_due(&self, id: &str, retry: Duration, refresh: Duration) -> bool {
        match self.sessions.get(id) {
            None => false,
            Some(s) => {
                let interval = if s.descriptor.is_none() {
                    retry
                } else {
                    refresh
                };
                match s.descriptor_checked_at {
                    None => true,
                    Some(t) => t.elapsed() >= interval,
                }
            }
        }
    }

    /// Record the result of a descriptor derivation. Always stamps the check time
    /// (so a fruitless read still debounces); updates the cached descriptor when
    /// the value actually changed. Returns true if the displayed value changed
    /// (worth a UI refresh). A no-op if the session is gone.
    pub fn set_descriptor(&mut self, id: &str, value: Option<String>) -> bool {
        match self.sessions.get_mut(id) {
            None => false,
            Some(s) => {
                s.descriptor_checked_at = Some(Instant::now());
                if value.is_some() && value != s.descriptor {
                    s.descriptor = value;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Update the stale timeout at runtime (settings change). Existing sessions
    /// are re-evaluated on the next sweep.
    pub fn set_stale_timeout(&mut self, timeout: Duration) {
        self.stale_timeout = timeout;
    }

    /// Update the idle-drop window at runtime (settings change). Existing
    /// sessions are re-evaluated on the next sweep.
    pub fn set_drop_timeout(&mut self, timeout: Duration) {
        self.drop_timeout = timeout;
    }

    /// Mark sessions stale past the timeout and drop them past the grace
    /// window. Reports whether anything changed and which sessions newly went
    /// stale (so the caller can optionally notify on idle).
    pub fn sweep(&mut self) -> SweepOutcome {
        let now = Instant::now();
        let mut changed = false;
        let mut went_stale = Vec::new();

        // Expired end-tombstones are useless — prune so the map can't grow.
        self.recent_ends
            .retain(|_, t| now.duration_since(*t) < END_TOMBSTONE);

        // Drop sessions silent past the whole idle-drop window. Until then a
        // stale session stays in the list (greyed) — it is not removed just for
        // crossing the stale timeout.
        let before = self.sessions.len();
        let drop_after = self.drop_timeout;
        self.sessions
            .retain(|_, s| now.duration_since(s.last_seen) < drop_after);
        if self.sessions.len() != before {
            changed = true;
        }

        // Mark the remainder stale/fresh based on the timeout.
        for (id, s) in self.sessions.iter_mut() {
            let idle = now.duration_since(s.last_seen);
            let should_be_stale = idle >= self.stale_timeout;
            if should_be_stale != s.stale {
                if should_be_stale {
                    went_stale.push((
                        id.clone(),
                        label_for(&s.cwd, s.git_base.as_deref(), s.git_branch.as_deref()),
                    ));
                    // A session we've declared silent ("No response") must not keep
                    // asserting live subagents — the matching SubagentStop may simply
                    // never have arrived. Clear the count so a greyed row doesn't read
                    // "idle · 1 agent running".
                    s.agents.clear();
                    s.pending_agents.clear();
                    s.sub_since = None;
                    s.pending_ready = false;
                }
                s.stale = should_be_stale;
                changed = true;
            }
        }
        SweepOutcome {
            changed,
            went_stale,
        }
    }

    /// Compute the tray rollup. Stale sessions are excluded; if none remain
    /// live the rollup is Grey. Priority: Red > Orange > Green.
    pub fn rollup(&self) -> Rollup {
        let mut any_working = false;
        let mut any_ready = false;
        for s in self.sessions.values() {
            if s.stale {
                continue;
            }
            match s.state {
                State::NeedsYou => return Rollup::Red,
                State::Working => any_working = true,
                State::Ready => any_ready = true,
            }
        }
        if any_working {
            Rollup::Orange
        } else if any_ready {
            Rollup::Green
        } else {
            Rollup::Grey
        }
    }

    /// A serializable snapshot of all sessions, newest-active first.
    ///
    /// **Performs zero I/O.** Every displayed field is a cached scalar. This is
    /// load-bearing on macOS, not just a nicety: `snapshot` runs on every hook
    /// event *and* is polled every 2 s per session by the widget, so any read
    /// under a session's `cwd` would re-trigger a TCC Files-and-Folders prompt
    /// for each protected folder category the user happens to work in. The git
    /// identity is pushed in by the capture hook instead — see `Session::git_base`
    /// and `capture.rs`.
    pub fn snapshot(&self) -> Vec<SessionView> {
        let now = Instant::now();
        let mut views: Vec<SessionView> = self
            .sessions
            .iter()
            .map(|(id, s)| {
                let (folder, branch) =
                    label_parts(&s.cwd, s.git_base.as_deref(), s.git_branch.as_deref());
                SessionView {
                    session_id: id.clone(),
                    label: combine_label(folder.clone(), branch.as_deref()),
                    folder,
                    branch,
                    worktree: s.git_worktree,
                    state: s.state,
                    stale: s.stale,
                    seconds_in_state: now.duration_since(s.state_since).as_secs(),
                    subagent_count: s.agents.len() as u32,
                    subagent_seconds: s
                        .sub_since
                        .map(|t| now.duration_since(t).as_secs())
                        .unwrap_or(0),
                    agents: s
                        .agents
                        .iter()
                        .map(|a| AgentView {
                            agent_id: a.id.clone(),
                            agent_type: a.agent_type.clone(),
                            description: a.description.clone(),
                            seconds: now.duration_since(a.started).as_secs(),
                        })
                        .collect(),
                    can_focus: s.terminal_pid.is_some(),
                    descriptor: s.descriptor.clone(),
                }
            })
            .collect();
        // Stable, useful ordering: live before stale, then by label.
        views.sort_by(|a, b| a.stale.cmp(&b.stale).then_with(|| a.label.cmp(&b.label)));
        views
    }
}

/// Is this tool call spawning a subagent? Claude Code 2.1.x names the tool
/// `Agent`; older versions named it `Task`. Both are accepted so the sub-line
/// keeps working across versions.
fn is_agent_tool(tool_name: Option<&str>) -> bool {
    matches!(tool_name, Some("Agent") | Some("Task"))
}

/// Take the description for a starting subagent out of the pending queue.
///
/// Preference order: the oldest unclaimed spawn whose requested `subagent_type`
/// matches the type the hook reports, else the oldest unclaimed spawn of any
/// type. Matching on type first is what keeps a mixed fan-out (an Explore and a
/// Plan dispatched together) labelled correctly no matter which one starts
/// first; within a single type, starts are observed in dispatch order, so FIFO
/// is right.
///
/// This is a correlation heuristic, not an identity join — Claude Code exposes
/// no key linking a `PreToolUse` to the `SubagentStart` it caused (see
/// [`AgentRun`]). A mismatch mislabels a sub-line; it can't corrupt state.
fn claim_description(
    pending: &mut VecDeque<PendingAgent>,
    agent_type: Option<&str>,
) -> Option<String> {
    let idx = agent_type
        .and_then(|t| {
            pending
                .iter()
                .position(|p| p.subagent_type.as_deref() == Some(t))
        })
        .or(if pending.is_empty() { None } else { Some(0) })?;
    pending.remove(idx).and_then(|p| p.description)
}

/// Remove a finished subagent. Matches on `agent_id` — which `SubagentStop`
/// does carry — falling back to the oldest entry if the id is missing or
/// unknown, so a malformed stop still can't strand a phantom agent on the row.
fn remove_agent(agents: &mut Vec<AgentRun>, agent_id: Option<&str>) -> Option<AgentRun> {
    let idx = agent_id
        .and_then(|id| agents.iter().position(|a| a.id == id))
        .unwrap_or(0);
    (idx < agents.len()).then(|| agents.remove(idx))
}

/// Remember a finished agent's description so a later resume can reuse it.
/// Agents that never had one are recorded too — "we looked and there was no
/// label" is a real answer, and recording it stops a resume from claiming an
/// unrelated pending spawn.
fn remember_agent(known: &mut VecDeque<(String, Option<String>)>, done: AgentRun) {
    if done.id.is_empty() {
        return;
    }
    known.retain(|(id, _)| id != &done.id);
    if known.len() >= MAX_PENDING_AGENTS {
        known.pop_front();
    }
    known.push_back((done.id, done.description));
}

/// The description this agent had last time it ran, if we've seen it before.
/// The outer `Option` distinguishes "never seen" from "seen, had no label".
fn recall_description(
    known: &VecDeque<(String, Option<String>)>,
    agent_id: Option<&str>,
) -> Option<Option<String>> {
    let id = agent_id?;
    known
        .iter()
        .find(|(known_id, _)| known_id == id)
        .map(|(_, desc)| desc.clone())
}

/// Tools that block on the user the instant they start and emit no
/// `Notification` the listener can observe, so the session would otherwise sit
/// at Working ("orange") while it is really blocked waiting for you:
/// - `AskUserQuestion` — the multiple-choice prompt.
/// - `ExitPlanMode` — plan approval.
///
/// `PreToolUse` for these escalates the session to NeedsYou; their `PostToolUse`
/// (the user answered) resumes Working. Kept as a single predicate so the set is
/// defined in exactly one place. Matched case-sensitively against Claude Code's
/// canonical tool names.
fn is_blocking_tool(tool_name: Option<&str>) -> bool {
    matches!(tool_name, Some("AskUserQuestion") | Some("ExitPlanMode"))
}

/// Build the structured session label: the repo root's name plus its branch,
/// falling back to `basename(cwd)` when the session isn't in a repo (or the
/// capture hook hasn't reported yet).
///
/// **Pure** — `base` and `branch` are the values the capture hook resolved in
/// the user's shell and the engine cached. This used to walk `cwd`'s ancestors
/// and read `.git/HEAD` in-process, which is what made macOS demand a
/// Files-and-Folders grant for every protected folder a session lived in. See
/// `Session::git_base` and `capture.rs`.
///
/// Shipped as separate parts so renderers never re-parse the combined string (a
/// folder literally named `foo (bar)` would misparse).
pub fn label_parts(
    cwd: &str,
    base: Option<&str>,
    branch: Option<&str>,
) -> (String, Option<String>) {
    (
        base.map(str::to_string)
            .unwrap_or_else(|| fallback_basename(cwd)),
        branch.map(str::to_string),
    )
}

/// `basename(cwd)` for the no-repo case; "session" for an empty cwd. String work
/// only — `Path::file_name` parses, it does not stat.
fn fallback_basename(cwd: &str) -> String {
    if cwd.is_empty() {
        return "session".to_string();
    }
    Path::new(cwd)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(cwd)
        .to_string()
}

/// The combined one-line label: `folder (branch)` or just `folder`.
pub fn label_for(cwd: &str, base: Option<&str>, branch: Option<&str>) -> String {
    let (folder, branch) = label_parts(cwd, base, branch);
    combine_label(folder, branch.as_deref())
}

fn combine_label(folder: String, branch: Option<&str>) -> String {
    match branch {
        Some(b) => format!("{folder} ({b})"),
        None => folder,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(name: &str, sid: &str) -> HookEvent {
        HookEvent {
            hook_event_name: name.to_string(),
            session_id: sid.to_string(),
            cwd: "/tmp/proj".to_string(),
            ..Default::default()
        }
    }

    /// A subagent-emitted event: same `session_id` as the parent, but carrying an
    /// `agent_id` (and `agent_type`) the way real Claude Code subagent hooks do.
    fn sub_ev(name: &str, sid: &str) -> HookEvent {
        HookEvent {
            agent_id: Some("sub-1".to_string()),
            agent_type: Some("Explore".to_string()),
            ..ev(name, sid)
        }
    }

    /// A `PreToolUse`/`PostToolUse` event carrying a `tool_name` — used to test
    /// the blocking-tool (`AskUserQuestion`/`ExitPlanMode`) escalation.
    fn tool_ev(name: &str, sid: &str, tool: &str) -> HookEvent {
        HookEvent {
            tool_name: Some(tool.to_string()),
            ..ev(name, sid)
        }
    }

    fn notif(sid: &str, ty: &str) -> HookEvent {
        HookEvent {
            hook_event_name: "Notification".to_string(),
            session_id: sid.to_string(),
            cwd: "/tmp/proj".to_string(),
            notification_type: Some(ty.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn lifecycle_transitions() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(e.rollup(), Rollup::Green);
        e.apply(&ev("UserPromptSubmit", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
        e.apply(&notif("a", "permission_prompt"));
        assert_eq!(e.rollup(), Rollup::Red);
        e.apply(&ev("Stop", "a"));
        assert_eq!(e.rollup(), Rollup::Green);
        e.apply(&ev("SessionEnd", "a"));
        assert_eq!(e.rollup(), Rollup::Grey);
    }

    /// Labels ship structured (folder + branch), so a directory whose *name*
    /// looks like the combined form can never be misparsed by a renderer.
    ///
    /// Note the paths here don't exist: label building is pure now, which is the
    /// whole point — see `label_parts`.
    #[test]
    fn label_parts_are_structured_not_reparsed() {
        // A directory literally named "foo (bar)", not in a repo — folder only.
        let (folder, branch) = label_parts("/nope/foo (bar)", None, None);
        assert_eq!(folder, "foo (bar)");
        assert_eq!(branch, None);

        // In a repo: the reported repo root name wins over the cwd's basename,
        // so a subdirectory cwd still shows the project.
        let (folder, branch) = label_parts("/nope/proj/src-tauri", Some("proj"), Some("main"));
        assert_eq!(folder, "proj");
        assert_eq!(branch.as_deref(), Some("main"));
        assert_eq!(
            label_for("/nope/proj/src-tauri", Some("proj"), Some("main")),
            "proj (main)"
        );
    }

    /// The row must render from cached scalars alone. A cwd that does not exist
    /// on disk is the strongest available proxy for "no filesystem read": before
    /// this change the label came from walking that path for a `.git` entry,
    /// which is exactly what made macOS prompt for folder access.
    #[test]
    fn snapshot_reads_nothing_from_disk() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&HookEvent {
            hook_event_name: "SessionStart".into(),
            session_id: "s".into(),
            cwd: "/definitely/does/not/exist/deep/repo".into(),
            ..Default::default()
        });
        e.apply(&HookEvent {
            hook_event_name: "BeaconTerminal".into(),
            session_id: "s".into(),
            cwd: "/definitely/does/not/exist/deep/repo".into(),
            capture_version: Some(2),
            capture_mode: Some("full".into()),
            git_base: Some("myrepo".into()),
            git_branch: Some("feature-x".into()),
            git_worktree: Some(true),
            ..Default::default()
        });

        let v = &e.snapshot()[0];
        assert_eq!(v.folder, "myrepo");
        assert_eq!(v.branch.as_deref(), Some("feature-x"));
        assert!(v.worktree);
        assert_eq!(v.label, "myrepo (feature-x)");
    }

    /// Until the capture hook reports — hooks not rewired yet, capture removed,
    /// a session that predates the upgrade — the row still has to render.
    #[test]
    fn session_without_capture_falls_back_to_basename() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "s"));

        let v = &e.snapshot()[0];
        assert_eq!(v.folder, "proj", "basename of cwd");
        assert_eq!(v.branch, None);
        assert!(!v.worktree);
    }

    /// Git facts are reported wholesale, so a fresh report replaces them —
    /// otherwise checking out a detached HEAD would leave the previous branch
    /// stuck on the row forever.
    #[test]
    fn fresh_capture_replaces_stale_branch() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "s"));
        let cap = |branch: &str| HookEvent {
            hook_event_name: "BeaconTerminal".into(),
            session_id: "s".into(),
            cwd: "/tmp/proj".into(),
            capture_version: Some(2),
            capture_mode: Some("turn".into()),
            git_base: Some("proj".into()),
            git_branch: Some(branch.to_string()),
            ..Default::default()
        };
        e.apply(&cap("main"));
        assert_eq!(e.snapshot()[0].branch.as_deref(), Some("main"));

        // Detached HEAD reports an empty branch.
        e.apply(&cap(""));
        assert_eq!(e.snapshot()[0].branch, None);
    }

    /// An older capture script reports no git facts at all. It must not wipe
    /// what a newer one already established.
    #[test]
    fn pre_v2_capture_does_not_wipe_git_facts() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "s"));
        e.apply(&HookEvent {
            hook_event_name: "BeaconTerminal".into(),
            session_id: "s".into(),
            cwd: "/tmp/proj".into(),
            capture_version: Some(2),
            git_base: Some("proj".into()),
            git_branch: Some("main".into()),
            ..Default::default()
        });
        // No capture_version: the pre-git script's shape.
        e.apply(&HookEvent {
            hook_event_name: "BeaconTerminal".into(),
            session_id: "s".into(),
            cwd: "/tmp/proj".into(),
            terminal_pid: Some(42),
            ..Default::default()
        });

        assert_eq!(e.snapshot()[0].branch.as_deref(), Some("main"));
    }

    /// Capture now fires every turn, so a per-turn report can lose the race with
    /// `SessionEnd`. It must never bring the row back from the dead.
    #[test]
    fn turn_capture_does_not_resurrect_an_ended_session() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "s"));
        e.apply(&ev("SessionEnd", "s"));
        assert!(e.snapshot().is_empty());

        e.apply(&HookEvent {
            hook_event_name: "BeaconTerminal".into(),
            session_id: "s".into(),
            cwd: "/tmp/proj".into(),
            capture_version: Some(2),
            capture_mode: Some("turn".into()),
            git_base: Some("proj".into()),
            git_branch: Some("main".into()),
            ..Default::default()
        });
        assert!(
            e.snapshot().is_empty(),
            "a per-turn capture must not create a row"
        );
    }

    /// `claude --resume <id>` reuses the session id, so a `SessionStart` capture
    /// can legitimately arrive moments after that id's `SessionEnd`. The
    /// end-tombstone must not swallow it — doing so drops the terminal handle
    /// for the resumed run and silently disables click-to-focus.
    #[test]
    fn full_capture_survives_a_recent_session_end() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "s"));
        e.apply(&ev("SessionEnd", "s"));
        assert!(e.snapshot().is_empty());

        // Resume: the capture beats the http SessionStart back to the listener.
        e.apply(&HookEvent {
            hook_event_name: "BeaconTerminal".into(),
            session_id: "s".into(),
            cwd: "/tmp/proj".into(),
            capture_version: Some(2),
            capture_mode: Some("full".into()),
            git_base: Some("proj".into()),
            git_branch: Some("main".into()),
            terminal_pid: Some(4242),
            ..Default::default()
        });

        assert_eq!(
            e.terminal_pid("s"),
            Some(4242),
            "a resumed session keeps its terminal handle"
        );
        assert_eq!(e.snapshot()[0].branch.as_deref(), Some("main"));
    }

    /// A handle+identity remembered across a Session Signals restart attaches
    /// when the row is recreated, so the label is right immediately rather than
    /// after the session's next turn.
    #[test]
    fn seeded_capture_restores_git_identity() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.seed_capture(
            "s".into(),
            CapturedTerminal {
                pid: None,
                app: None,
                tty: None,
                base: Some("proj".into()),
                branch: Some("main".into()),
                worktree: true,
            },
        );
        e.apply(&ev("UserPromptSubmit", "s"));

        let v = &e.snapshot()[0];
        assert_eq!(v.folder, "proj");
        assert_eq!(v.branch.as_deref(), Some("main"));
        assert!(v.worktree);
    }

    /// A straggler heartbeat (e.g. a subagent's late `PostToolUse` /
    /// `SubagentStop`) arriving after `SessionEnd` must NOT resurrect the row
    /// — it would sit orange until the stale sweep. A real restart (any
    /// state-setting event, like `SessionStart` or `UserPromptSubmit`) clears
    /// the tombstone and recreates the session normally.
    #[test]
    fn straggler_heartbeat_after_session_end_does_not_resurrect() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "a"));
        e.apply(&ev("SessionEnd", "a"));
        assert!(e.snapshot().is_empty());

        // Late subagent + main-agent heartbeats: dropped, no row.
        let out = e.apply(&sub_ev("PostToolUse", "a"));
        assert!(!out.changed, "straggler must not report a change");
        let out = e.apply(&sub_ev("SubagentStop", "a"));
        assert!(!out.changed);
        e.apply(&ev("PostToolBatch", "a"));
        assert!(e.snapshot().is_empty(), "no resurrection from heartbeats");

        // A genuine restart of the same id still works.
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(e.snapshot().len(), 1);
        assert_eq!(e.rollup(), Rollup::Green);

        // And once restarted, heartbeats flow normally again.
        e.apply(&ev("UserPromptSubmit", "a"));
        e.apply(&sub_ev("PostToolUse", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
    }

    /// The tombstone also gives way to `UserPromptSubmit` directly (a resume
    /// that never re-fires SessionStart).
    #[test]
    fn prompt_after_session_end_recreates_session() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "a"));
        e.apply(&ev("SessionEnd", "a"));
        e.apply(&ev("UserPromptSubmit", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
    }

    #[test]
    fn rollup_priority_across_sessions() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a")); // working
        e.apply(&notif("b", "permission_prompt")); // needs you
                                                   // One Working + one Needs-you → Red.
        assert_eq!(e.rollup(), Rollup::Red);
        e.apply(&ev("Stop", "b")); // b now ready
        assert_eq!(e.rollup(), Rollup::Orange); // a still working
    }

    #[test]
    fn ignored_notifications_do_not_change_state() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
        e.apply(&notif("a", "auth_success"));
        assert_eq!(e.rollup(), Rollup::Orange);
    }

    #[test]
    fn transition_reported_once_then_suppressed() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        // New session entering Working: transition from None.
        let out = e.apply(&ev("UserPromptSubmit", "a"));
        let t = out.transition.expect("first transition");
        assert_eq!(t.from, None);
        assert_eq!(t.to, State::Working);

        // First permission prompt: Working → NeedsYou (one transition).
        let out = e.apply(&notif("a", "permission_prompt"));
        let t = out.transition.expect("transition into needs-you");
        assert_eq!(t.from, Some(State::Working));
        assert_eq!(t.to, State::NeedsYou);

        // A second permission prompt while already NeedsYou: NO transition.
        let out = e.apply(&notif("a", "permission_prompt"));
        assert!(out.transition.is_none(), "repeat must not re-notify");
    }

    #[test]
    fn idle_prompt_does_not_turn_red() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        // A finished turn is Ready/green.
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(e.rollup(), Rollup::Green);

        // An idle_prompt must NOT flip it to red, and must not be a transition.
        let out = e.apply(&notif("a", "idle_prompt"));
        assert!(
            out.transition.is_none(),
            "idle_prompt should not transition"
        );
        assert_eq!(e.rollup(), Rollup::Green, "idle session stays green");

        // A genuine permission prompt still turns it red.
        e.apply(&notif("a", "permission_prompt"));
        assert_eq!(e.rollup(), Rollup::Red);
        // And a later idle_prompt doesn't clear the real red either.
        e.apply(&notif("a", "idle_prompt"));
        assert_eq!(e.rollup(), Rollup::Red, "idle must not clear a pending red");
    }

    #[test]
    fn heartbeat_keeps_state() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        e.apply(&ev("PostToolUse", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
    }

    #[test]
    fn compaction_shows_working_then_ready() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        // A finished turn is Ready/green.
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(e.rollup(), Rollup::Green);
        // `/compact` fires PreCompact (never UserPromptSubmit) → Working/orange.
        e.apply(&ev("PreCompact", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
        // PostCompact ends compaction → back to Ready/green.
        e.apply(&ev("PostCompact", "a"));
        assert_eq!(e.rollup(), Rollup::Green);
    }

    #[test]
    fn pretooluse_starts_working() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        // A session first seen via a tool call is Working, even if we never
        // observed its UserPromptSubmit.
        let out = e.apply(&ev("PreToolUse", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
        let t = out.transition.expect("new session transitions");
        assert_eq!(t.from, None);
        assert_eq!(t.to, State::Working);
    }

    #[test]
    fn stale_sweep_excludes_then_drops() {
        // Zero timeout so everything is immediately stale; tiny grace.
        let mut e = Engine::new(Duration::from_millis(0), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(e.rollup(), Rollup::Green);
        let out = e.sweep();
        assert!(out.changed);
        assert_eq!(out.went_stale.len(), 1);
        // Stale → excluded from rollup → Grey, but still present.
        assert_eq!(e.rollup(), Rollup::Grey);
        assert_eq!(e.snapshot().len(), 1);

        // Now make drop happen immediately too.
        let mut e2 = Engine::new(Duration::from_millis(0), Duration::from_millis(0));
        e2.apply(&ev("SessionStart", "a"));
        e2.sweep();
        assert_eq!(e2.snapshot().len(), 0);
    }

    /// Live subagent count for a session id in the current snapshot.
    fn sub_count(e: &Engine, sid: &str) -> u32 {
        e.snapshot()
            .into_iter()
            .find(|v| v.session_id == sid)
            .map(|v| v.subagent_count)
            .unwrap_or(0)
    }

    fn state_of(e: &Engine, sid: &str) -> State {
        e.snapshot()
            .into_iter()
            .find(|v| v.session_id == sid)
            .map(|v| v.state)
            .expect("session present")
    }

    #[test]
    fn subagent_count_rises_and_falls() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        assert_eq!(sub_count(&e, "a"), 0);
        e.apply(&ev("SubagentStart", "a"));
        e.apply(&ev("SubagentStart", "a"));
        assert_eq!(sub_count(&e, "a"), 2);
        // Still actively working with subagents out.
        assert_eq!(e.rollup(), Rollup::Orange);
        e.apply(&ev("SubagentStop", "a"));
        assert_eq!(sub_count(&e, "a"), 1);
        e.apply(&ev("SubagentStop", "a"));
        assert_eq!(sub_count(&e, "a"), 0);
    }

    #[test]
    fn subagent_count_clamps_at_zero() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SubagentStart", "a"));
        // Two stops for one start must not underflow.
        e.apply(&ev("SubagentStop", "a"));
        e.apply(&ev("SubagentStop", "a"));
        assert_eq!(sub_count(&e, "a"), 0);
        // And a fresh start still works afterwards.
        e.apply(&ev("SubagentStart", "a"));
        assert_eq!(sub_count(&e, "a"), 1);
    }

    #[test]
    fn subagent_counts_are_per_session() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        // Interleaved starts/stops across two concurrent sessions.
        e.apply(&ev("SubagentStart", "a"));
        e.apply(&ev("SubagentStart", "b"));
        e.apply(&ev("SubagentStart", "a"));
        e.apply(&ev("SubagentStop", "b"));
        assert_eq!(sub_count(&e, "a"), 2);
        assert_eq!(sub_count(&e, "b"), 0);
    }

    #[test]
    fn sub_since_anchors_only_while_busy() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SubagentStart", "a"));
        // Busy → a real elapsed anchor (seconds is small but the field exists).
        let v = e
            .snapshot()
            .into_iter()
            .find(|v| v.session_id == "a")
            .unwrap();
        assert_eq!(v.subagent_count, 1);
        // Idle → seconds reported as 0.
        e.apply(&ev("SubagentStop", "a"));
        let v = e
            .snapshot()
            .into_iter()
            .find(|v| v.session_id == "a")
            .unwrap();
        assert_eq!(v.subagent_count, 0);
        assert_eq!(v.subagent_seconds, 0);
    }

    #[test]
    fn beacon_terminal_records_pid_and_survives_state_changes() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        // Capture arrives (possibly before SessionStart) and records the pid.
        let mut cap = ev("BeaconTerminal", "a");
        cap.terminal_pid = Some(4242);
        cap.terminal_app = Some("iTerm2".to_string());
        e.apply(&cap);
        assert_eq!(e.terminal_pid("a"), Some(4242));
        // can_focus is exposed in the snapshot.
        assert!(
            e.snapshot()
                .iter()
                .find(|v| v.session_id == "a")
                .unwrap()
                .can_focus
        );

        // State changes don't drop the captured terminal, and the transition
        // carries the pid for focus-aware notifications.
        let out = e.apply(&notif("a", "permission_prompt"));
        assert_eq!(e.terminal_pid("a"), Some(4242), "pid survives state change");
        assert_eq!(out.transition.unwrap().terminal_pid, Some(4242));
    }

    #[test]
    fn no_terminal_means_cannot_focus() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(e.terminal_pid("a"), None);
        assert!(!e.snapshot()[0].can_focus, "no pid ⇒ no focus affordance");
    }

    #[test]
    fn seeded_capture_rehydrates_on_next_event_but_never_conjures_a_row() {
        // Simulates a Session Signals restart: a handle was persisted last run and seeded
        // back in, but the session hasn't emitted anything yet.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.seed_capture(
            "a".to_string(),
            CapturedTerminal {
                pid: Some(7777),
                app: Some("Terminal".to_string()),
                tty: Some("/dev/ttys003".to_string()),
                ..Default::default()
            },
        );
        // A seed alone must NOT create a session row (no phantom rows).
        assert!(e.snapshot().is_empty(), "seeding alone creates no session");
        assert_eq!(e.terminal_pid("a"), None);

        // The first real event recreates the row AND picks up the handle, so
        // click-to-focus is back without a fresh SessionStart.
        e.apply(&ev("PostToolUse", "a"));
        assert_eq!(
            e.terminal_pid("a"),
            Some(7777),
            "handle rehydrated on first event"
        );
        let v = e
            .snapshot()
            .into_iter()
            .find(|v| v.session_id == "a")
            .unwrap();
        assert!(
            v.can_focus,
            "rehydrated handle restores the focus affordance"
        );
        assert_eq!(
            e.focus_target("a"),
            Some((
                7777,
                Some("/dev/ttys003".to_string()),
                Some("Terminal".to_string())
            ))
        );
    }

    #[test]
    fn session_end_forgets_seeded_capture() {
        // A seeded handle must not outlive an explicit SessionEnd, so a later
        // same-id session can't inherit a dead terminal's pid.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.seed_capture(
            "a".to_string(),
            CapturedTerminal {
                pid: Some(7777),
                app: None,
                tty: None,
                ..Default::default()
            },
        );
        e.apply(&ev("SessionEnd", "a"));
        // Recreating the row now must NOT resurrect the forgotten handle.
        e.apply(&ev("PostToolUse", "a"));
        assert_eq!(e.terminal_pid("a"), None, "SessionEnd dropped the seed");
    }

    // --- Subagent events must not overwrite the parent's traffic-light state ---

    #[test]
    fn subagent_tool_calls_do_not_clear_needs_you() {
        // The reported bug: blocked on a permission while subagents run, a
        // subagent's tool calls must NOT flip the row off red.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&notif("a", "permission_prompt"));
        assert_eq!(e.rollup(), Rollup::Red);
        e.apply(&sub_ev("PreToolUse", "a"));
        e.apply(&sub_ev("PostToolUse", "a"));
        assert_eq!(e.rollup(), Rollup::Red, "subagent activity kept NeedsYou");
        assert_eq!(state_of(&e, "a"), State::NeedsYou);
    }

    #[test]
    fn subagent_stop_does_not_clear_needs_you() {
        // SubagentStop used to call transition_to(Ready) — it must not, or the
        // last subagent finishing would turn a still-blocked parent green.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&sub_ev("SubagentStart", "a"));
        e.apply(&notif("a", "permission_prompt"));
        assert_eq!(e.rollup(), Rollup::Red);
        e.apply(&sub_ev("SubagentStop", "a"));
        assert_eq!(
            state_of(&e, "a"),
            State::NeedsYou,
            "subagent stop left red intact"
        );
        assert_eq!(sub_count(&e, "a"), 0, "but the count still decremented");
    }

    #[test]
    fn main_agent_pretooluse_clears_needs_you() {
        // The legitimate path off red: the user approves, the MAIN agent's tool
        // runs (agent_id absent) → Working.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&notif("a", "permission_prompt"));
        assert_eq!(e.rollup(), Rollup::Red);
        e.apply(&ev("PreToolUse", "a")); // main agent, no agent_id
        assert_eq!(
            state_of(&e, "a"),
            State::Working,
            "user-approved tool flips to Working"
        );
    }

    #[test]
    fn subagent_permission_prompt_still_escalates() {
        // A block is a block: a subagent hitting a permission gate must set
        // NeedsYou even though it's a subagent event.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
        let mut block = notif("a", "permission_prompt");
        block.agent_id = Some("sub-1".to_string());
        e.apply(&block);
        assert_eq!(
            state_of(&e, "a"),
            State::NeedsYou,
            "subagent block still needs the user"
        );
    }

    // --- Blocking tools (AskUserQuestion / ExitPlanMode) escalate to NeedsYou ---

    #[test]
    fn ask_user_question_turns_red() {
        // The reported bug: an AskUserQuestion prompt blocks on the user but emits
        // no Notification, so the session used to sit at Working ("orange"). Its
        // `PreToolUse` must now escalate to NeedsYou ("red").
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
        let out = e.apply(&tool_ev("PreToolUse", "a", "AskUserQuestion"));
        assert_eq!(state_of(&e, "a"), State::NeedsYou);
        assert_eq!(e.rollup(), Rollup::Red);
        // It's a real transition (Working → NeedsYou), so a notification fires once.
        let t = out.transition.expect("escalation is a transition");
        assert_eq!(t.from, Some(State::Working));
        assert_eq!(t.to, State::NeedsYou);
    }

    #[test]
    fn exit_plan_mode_turns_red() {
        // Plan approval blocks on the user the same way.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        e.apply(&tool_ev("PreToolUse", "a", "ExitPlanMode"));
        assert_eq!(state_of(&e, "a"), State::NeedsYou);
    }

    #[test]
    fn answering_blocking_tool_resumes_working() {
        // The user answers → the tool returns (PostToolUse for the same tool) →
        // the main agent resumes, so the row goes NeedsYou → Working, not stuck red.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&tool_ev("PreToolUse", "a", "AskUserQuestion"));
        assert_eq!(state_of(&e, "a"), State::NeedsYou);
        e.apply(&tool_ev("PostToolUse", "a", "AskUserQuestion"));
        assert_eq!(
            state_of(&e, "a"),
            State::Working,
            "answered question resumes Working"
        );
    }

    #[test]
    fn ordinary_tool_still_only_working() {
        // A non-blocking tool must behave exactly as before: PreToolUse → Working,
        // PostToolUse → heartbeat (keep state). No accidental red.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&tool_ev("PreToolUse", "a", "Bash"));
        assert_eq!(state_of(&e, "a"), State::Working);
        e.apply(&tool_ev("PostToolUse", "a", "Bash"));
        assert_eq!(state_of(&e, "a"), State::Working);
        // A tool-less PreToolUse (tool_name absent) is still plain Working.
        e.apply(&ev("PreToolUse", "b"));
        assert_eq!(state_of(&e, "b"), State::Working);
    }

    #[test]
    fn subagent_blocking_tool_still_escalates() {
        // A block is a block: even if a subagent somehow owns the blocking tool,
        // the user must still be flagged (mirrors the permission-gate exception).
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        let mut block = tool_ev("PreToolUse", "a", "AskUserQuestion");
        block.agent_id = Some("sub-1".to_string());
        e.apply(&block);
        assert_eq!(state_of(&e, "a"), State::NeedsYou);
    }

    #[test]
    fn subagent_post_blocking_tool_does_not_clear_needs_you() {
        // Guard the PostToolUse path: a subagent's PostToolUse for a blocking tool
        // must NOT flip a genuinely-blocked parent to Working (only the main
        // agent's answer does).
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&notif("a", "permission_prompt"));
        assert_eq!(state_of(&e, "a"), State::NeedsYou);
        let mut sub_post = tool_ev("PostToolUse", "a", "AskUserQuestion");
        sub_post.agent_id = Some("sub-1".to_string());
        e.apply(&sub_post);
        assert_eq!(
            state_of(&e, "a"),
            State::NeedsYou,
            "subagent PostToolUse kept the parent red"
        );
    }

    #[test]
    fn subagent_count_independent_of_main_state() {
        // State is pinned by main-agent events; the count moves only with
        // subagent start/stop — the two never interfere.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a")); // main → Working
        e.apply(&sub_ev("SubagentStart", "a"));
        e.apply(&sub_ev("SubagentStart", "a"));
        assert_eq!(state_of(&e, "a"), State::Working);
        assert_eq!(sub_count(&e, "a"), 2);
        e.apply(&sub_ev("SubagentStop", "a")); // a subagent ends mid-turn...
        assert_eq!(
            state_of(&e, "a"),
            State::Working,
            "...but the parent stays Working"
        );
        assert_eq!(sub_count(&e, "a"), 1);
    }

    #[test]
    fn stale_sweep_clears_subagent_count() {
        // A greyed "No response" row must not keep claiming running agents.
        let mut e = Engine::new(Duration::from_millis(0), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        e.apply(&sub_ev("SubagentStart", "a"));
        assert_eq!(sub_count(&e, "a"), 1);
        e.sweep(); // stale_timeout is 0 → goes stale immediately
        assert!(e.snapshot()[0].stale);
        assert_eq!(sub_count(&e, "a"), 0, "stale row drops its agent count");
    }

    #[test]
    fn descriptor_due_set_and_snapshot() {
        let retry = Duration::from_secs(5);
        let refresh = Duration::from_secs(45);
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        // Unknown session is never due.
        assert!(!e.descriptor_due("a", retry, refresh));
        e.apply(&ev("SessionStart", "a"));
        // Never checked → due immediately.
        assert!(e.descriptor_due("a", retry, refresh));
        // Setting a value reports a change, stamps checked-at, and surfaces in the
        // snapshot; a fresh check is no longer due.
        assert!(e.set_descriptor("a", Some("Audit the repo".to_string())));
        assert!(
            !e.descriptor_due("a", retry, refresh),
            "just checked → not due"
        );
        let v = e
            .snapshot()
            .into_iter()
            .find(|v| v.session_id == "a")
            .unwrap();
        assert_eq!(v.descriptor.as_deref(), Some("Audit the repo"));
        // Same value → no change reported.
        assert!(!e.set_descriptor("a", Some("Audit the repo".to_string())));
        // A fruitless re-derivation (None) must not clear an existing descriptor.
        assert!(!e.set_descriptor("a", None));
        let v = e
            .snapshot()
            .into_iter()
            .find(|v| v.session_id == "a")
            .unwrap();
        assert_eq!(
            v.descriptor.as_deref(),
            Some("Audit the repo"),
            "None doesn't clear"
        );
    }

    #[test]
    fn session_start_resets_subagents() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SubagentStart", "a"));
        e.apply(&ev("SubagentStart", "a"));
        assert_eq!(sub_count(&e, "a"), 2);
        // A (re)start of the same id clears the count.
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(sub_count(&e, "a"), 0);
    }

    #[test]
    fn idle_session_persists_until_drop_window() {
        // Stale immediately, but a long drop window: an idle session must stay
        // in the list (greyed) across repeated sweeps rather than blink out.
        let mut e = Engine::new(Duration::from_millis(0), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "a"));
        for _ in 0..3 {
            e.sweep();
        }
        assert_eq!(e.snapshot().len(), 1, "stale session stays visible");
        assert!(e.snapshot()[0].stale, "and is marked stale (grey)");
        assert_eq!(e.rollup(), Rollup::Grey);
    }

    // --- Added coverage: event→state derivation table, full rollup priority
    //     ordering, and stale-sweep exclusion / revival ---

    #[test]
    fn event_to_state_derivation_table() {
        // Each state-driving main-agent event, applied to a fresh session, lands
        // the row in the state documented in the CLAUDE.md hook contract.
        let cases: &[(&str, State)] = &[
            ("SessionStart", State::Ready),
            ("UserPromptSubmit", State::Working),
            ("PreToolUse", State::Working),
            ("PreCompact", State::Working),
            ("PostCompact", State::Ready),
            ("Stop", State::Ready),
        ];
        for (name, want) in cases {
            let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
            e.apply(&ev(name, "a"));
            assert_eq!(
                state_of(&e, "a"),
                *want,
                "event {name} should derive {want:?}"
            );
        }
        // Both NeedsYou notification types escalate identically; an ignored one
        // (idle_prompt) creates the row but leaves it Ready, never NeedsYou.
        for ty in ["permission_prompt", "elicitation_dialog"] {
            let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
            e.apply(&notif("a", ty));
            assert_eq!(state_of(&e, "a"), State::NeedsYou, "{ty} → NeedsYou");
        }
    }

    #[test]
    fn rollup_full_priority_ordering() {
        // Walk Grey → Green → Orange → Red, proving each higher-priority state
        // dominates regardless of how many lower-priority sessions are present,
        // then unwind back down as sessions end.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        assert_eq!(e.rollup(), Rollup::Grey, "no sessions → Grey");

        e.apply(&ev("SessionStart", "ready")); // Ready
        assert_eq!(e.rollup(), Rollup::Green);

        e.apply(&ev("UserPromptSubmit", "working")); // + Working
        assert_eq!(e.rollup(), Rollup::Orange, "Working outranks Ready");

        e.apply(&notif("needs", "permission_prompt")); // + NeedsYou
        assert_eq!(e.rollup(), Rollup::Red, "NeedsYou outranks all");

        // Unwind: dropping the red session falls back to Orange (Working remains),
        // then to Green (only Ready remains), then to Grey (empty).
        e.apply(&ev("SessionEnd", "needs"));
        assert_eq!(e.rollup(), Rollup::Orange);
        e.apply(&ev("SessionEnd", "working"));
        assert_eq!(e.rollup(), Rollup::Green);
        e.apply(&ev("SessionEnd", "ready"));
        assert_eq!(e.rollup(), Rollup::Grey);
    }

    #[test]
    fn rollup_excludes_stale_sessions() {
        // A stale (silent) session must not color the tray: a session that would
        // be Red while live is invisible to the rollup once swept stale, so an
        // all-stale engine reads Grey rather than Red.
        let mut e = Engine::new(Duration::from_millis(0), Duration::from_secs(3600));
        e.apply(&notif("old", "permission_prompt")); // live → would be Red
        assert_eq!(e.rollup(), Rollup::Red);
        e.sweep(); // stale_timeout 0 → immediately stale
        assert!(e.snapshot()[0].stale, "swept stale");
        assert_eq!(e.rollup(), Rollup::Grey, "all-stale → Grey, not Red");
    }

    #[test]
    fn stale_session_revives_on_next_event() {
        // After a sweep greys a session, the next hook event un-stales it and
        // restores its color in the rollup (heartbeat clears `stale`).
        let mut e = Engine::new(Duration::from_millis(0), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        e.sweep();
        assert!(e.snapshot()[0].stale, "went stale");
        assert_eq!(e.rollup(), Rollup::Grey);

        e.apply(&ev("PostToolUse", "a")); // heartbeat
        assert!(!e.snapshot()[0].stale, "event revived the row");
        assert_eq!(
            e.rollup(),
            Rollup::Orange,
            "back to its prior Working state"
        );
    }

    #[test]
    fn sweep_on_empty_engine_is_noop() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        let out = e.sweep();
        assert!(!out.changed, "nothing to sweep");
        assert!(out.went_stale.is_empty());
        assert_eq!(e.rollup(), Rollup::Grey);
    }

    // ---- Per-agent identity + descriptions -----------------------------

    /// A main-agent `PreToolUse(Agent)` carrying the task description, as
    /// Claude Code actually sends it.
    fn spawn_ev(sid: &str, agent_type: &str, description: &str) -> HookEvent {
        HookEvent {
            tool_input: Some(ToolInput {
                description: Some(description.to_string()),
                subagent_type: Some(agent_type.to_string()),
            }),
            ..tool_ev("PreToolUse", sid, "Agent")
        }
    }

    /// A subagent lifecycle event for a specific agent id + type.
    fn agent_ev(name: &str, sid: &str, id: &str, agent_type: &str) -> HookEvent {
        HookEvent {
            agent_id: Some(id.to_string()),
            agent_type: Some(agent_type.to_string()),
            ..ev(name, sid)
        }
    }

    fn agents_of(e: &Engine, sid: &str) -> Vec<AgentView> {
        e.snapshot()
            .into_iter()
            .find(|v| v.session_id == sid)
            .map(|v| v.agents)
            .unwrap_or_default()
    }

    #[test]
    fn subagent_start_records_type_and_description() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("SessionStart", "s1"));
        e.apply(&spawn_ev("s1", "Explore", "Map subagent UI in widget"));
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));

        let agents = agents_of(&e, "s1");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_id, "a1");
        assert_eq!(agents[0].agent_type.as_deref(), Some("Explore"));
        assert_eq!(
            agents[0].description.as_deref(),
            Some("Map subagent UI in widget")
        );
        assert_eq!(sub_count(&e, "s1"), 1, "count still tracks the list");
    }

    #[test]
    fn parallel_same_type_agents_pair_in_dispatch_order() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("SessionStart", "s1"));
        // Both spawns are observed before either agent starts — the real
        // ordering for a fan-out dispatched in one message.
        e.apply(&spawn_ev("s1", "Explore", "alpha"));
        e.apply(&spawn_ev("s1", "Explore", "bravo"));
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));
        e.apply(&agent_ev("SubagentStart", "s1", "a2", "Explore"));

        let agents = agents_of(&e, "s1");
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].description.as_deref(), Some("alpha"));
        assert_eq!(agents[1].description.as_deref(), Some("bravo"));
    }

    #[test]
    fn mixed_type_fan_out_matches_on_type_not_arrival_order() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("SessionStart", "s1"));
        e.apply(&spawn_ev("s1", "Explore", "explore work"));
        e.apply(&spawn_ev("s1", "Plan", "plan work"));
        // The Plan agent starts first — type matching must beat FIFO here.
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Plan"));
        e.apply(&agent_ev("SubagentStart", "s1", "a2", "Explore"));

        let agents = agents_of(&e, "s1");
        assert_eq!(agents[0].description.as_deref(), Some("plan work"));
        assert_eq!(agents[1].description.as_deref(), Some("explore work"));
    }

    #[test]
    fn agent_without_a_matching_spawn_has_no_description() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("SessionStart", "s1"));
        // No PreToolUse seen (e.g. Session Signals started mid-turn).
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));

        let agents = agents_of(&e, "s1");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].description, None);
        assert_eq!(agents[0].agent_type.as_deref(), Some("Explore"));
    }

    #[test]
    fn subagent_stop_removes_the_agent_that_stopped() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("SessionStart", "s1"));
        e.apply(&spawn_ev("s1", "Explore", "alpha"));
        e.apply(&spawn_ev("s1", "Explore", "bravo"));
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));
        e.apply(&agent_ev("SubagentStart", "s1", "a2", "Explore"));
        // The *first* one finishes; the survivor must be bravo, not alpha.
        e.apply(&agent_ev("SubagentStop", "s1", "a1", "Explore"));

        let agents = agents_of(&e, "s1");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_id, "a2");
        assert_eq!(agents[0].description.as_deref(), Some("bravo"));
    }

    #[test]
    fn legacy_task_tool_name_still_captures_descriptions() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("SessionStart", "s1"));
        e.apply(&HookEvent {
            tool_input: Some(ToolInput {
                description: Some("legacy spawn".to_string()),
                subagent_type: Some("Explore".to_string()),
            }),
            ..tool_ev("PreToolUse", "s1", "Task")
        });
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));
        assert_eq!(
            agents_of(&e, "s1")[0].description.as_deref(),
            Some("legacy spawn")
        );
    }

    #[test]
    fn a_non_agent_tool_call_queues_nothing() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("SessionStart", "s1"));
        // A Bash call carries no agent tool_input; if it somehow did, it must
        // not end up labelling a later agent.
        e.apply(&HookEvent {
            tool_input: Some(ToolInput {
                description: Some("not an agent".to_string()),
                subagent_type: Some("Explore".to_string()),
            }),
            ..tool_ev("PreToolUse", "s1", "Bash")
        });
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));
        assert_eq!(agents_of(&e, "s1")[0].description, None);
    }

    // ---- Stop is deferred until the agents finish ----------------------

    #[test]
    fn stop_with_agents_running_stays_working() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("UserPromptSubmit", "s1"));
        e.apply(&spawn_ev("s1", "Explore", "alpha"));
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));

        let out = e.apply(&ev("Stop", "s1"));
        assert_eq!(
            state_of(&e, "s1"),
            State::Working,
            "agents still running — the turn is not over"
        );
        assert!(
            out.transition.is_none(),
            "already Working; no transition, so no notification"
        );
        assert_eq!(e.rollup(), Rollup::Orange);
    }

    #[test]
    fn last_subagent_stop_after_stop_releases_ready() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("UserPromptSubmit", "s1"));
        e.apply(&spawn_ev("s1", "Explore", "alpha"));
        e.apply(&spawn_ev("s1", "Explore", "bravo"));
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));
        e.apply(&agent_ev("SubagentStart", "s1", "a2", "Explore"));
        e.apply(&ev("Stop", "s1"));

        // First agent leaving is not enough.
        let out = e.apply(&agent_ev("SubagentStop", "s1", "a1", "Explore"));
        assert_eq!(state_of(&e, "s1"), State::Working);
        assert!(out.transition.is_none());

        // The last one releases the deferred Ready — and this is the transition
        // that fires the "finished, your turn" notification.
        let out = e.apply(&agent_ev("SubagentStop", "s1", "a2", "Explore"));
        assert_eq!(state_of(&e, "s1"), State::Ready);
        let t = out.transition.expect("Working → Ready transition");
        assert_eq!(t.from, Some(State::Working));
        assert_eq!(t.to, State::Ready);
        assert_eq!(sub_count(&e, "s1"), 0);
    }

    #[test]
    fn stop_with_no_agents_is_ready_immediately() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("UserPromptSubmit", "s1"));
        let out = e.apply(&ev("Stop", "s1"));
        assert_eq!(state_of(&e, "s1"), State::Ready);
        assert!(
            out.transition.is_some(),
            "unchanged fast path still notifies"
        );
    }

    #[test]
    fn a_new_turn_cancels_a_deferred_ready() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("UserPromptSubmit", "s1"));
        e.apply(&spawn_ev("s1", "Explore", "alpha"));
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));
        e.apply(&ev("Stop", "s1"));

        // The user sends a new prompt while the old turn's agent is still going.
        e.apply(&ev("UserPromptSubmit", "s1"));
        // That agent finishing must NOT yank the live turn to green.
        e.apply(&agent_ev("SubagentStop", "s1", "a1", "Explore"));
        assert_eq!(state_of(&e, "s1"), State::Working);
    }

    #[test]
    fn a_subagent_stop_never_clears_a_block_on_its_own() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("UserPromptSubmit", "s1"));
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));
        // The subagent hits a permission gate: the parent goes red.
        e.apply(&HookEvent {
            notification_type: Some("permission_prompt".to_string()),
            ..agent_ev("Notification", "s1", "a1", "Explore")
        });
        assert_eq!(state_of(&e, "s1"), State::NeedsYou);

        // It then stops. With no deferred Ready pending, red must survive.
        e.apply(&agent_ev("SubagentStop", "s1", "a1", "Explore"));
        assert_eq!(state_of(&e, "s1"), State::NeedsYou);
    }

    #[test]
    fn stale_sweep_clears_agents_and_deferred_ready() {
        let mut e = Engine::new(Duration::from_millis(1), Duration::from_secs(600));
        e.apply(&ev("UserPromptSubmit", "s1"));
        e.apply(&spawn_ev("s1", "Explore", "alpha"));
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));
        e.apply(&ev("Stop", "s1"));
        std::thread::sleep(Duration::from_millis(5));
        e.sweep();

        assert!(
            agents_of(&e, "s1").is_empty(),
            "greyed row asserts no agents"
        );
        assert_eq!(sub_count(&e, "s1"), 0);
        // The deferred Ready went with them: a straggling stop can't resurrect it.
        e.apply(&agent_ev("SubagentStop", "s1", "a1", "Explore"));
        assert_eq!(state_of(&e, "s1"), State::Working);
    }

    #[test]
    fn session_start_clears_agents_and_pending_spawns() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("UserPromptSubmit", "s1"));
        e.apply(&spawn_ev("s1", "Explore", "never started"));
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));
        e.apply(&ev("SessionStart", "s1"));

        assert!(agents_of(&e, "s1").is_empty());
        // The unclaimed spawn is gone too, so it can't label an agent from the
        // new session's first fan-out.
        e.apply(&agent_ev("SubagentStart", "s1", "a2", "Explore"));
        assert_eq!(agents_of(&e, "s1")[0].description, None);
    }

    #[test]
    fn a_resumed_agent_keeps_its_description() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("UserPromptSubmit", "s1"));
        e.apply(&spawn_ev("s1", "Explore", "map the widget"));
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));
        e.apply(&agent_ev("SubagentStop", "s1", "a1", "Explore"));
        // Resumed later in the same session with no fresh PreToolUse behind it.
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));

        let agents = agents_of(&e, "s1");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].description.as_deref(), Some("map the widget"));
    }

    #[test]
    fn a_resumed_agent_does_not_steal_another_spawns_description() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("UserPromptSubmit", "s1"));
        // An agent we never saw spawned, so it has no label of its own.
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));
        e.apply(&agent_ev("SubagentStop", "s1", "a1", "Explore"));
        // A genuinely new agent is dispatched, then the old one resumes.
        e.apply(&spawn_ev("s1", "Explore", "the new task"));
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));
        e.apply(&agent_ev("SubagentStart", "s1", "a2", "Explore"));

        let agents = agents_of(&e, "s1");
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].agent_id, "a1");
        assert_eq!(
            agents[0].description, None,
            "resume must not take a2's label"
        );
        assert_eq!(agents[1].agent_id, "a2");
        assert_eq!(agents[1].description.as_deref(), Some("the new task"));
    }

    #[test]
    fn unclaimed_spawns_are_bounded() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(60));
        e.apply(&ev("SessionStart", "s1"));
        // Far more spawns than ever start — the queue must not grow forever.
        for i in 0..(MAX_PENDING_AGENTS * 3) {
            e.apply(&spawn_ev("s1", "Explore", &format!("task {i}")));
        }
        let pending = e.sessions.get("s1").map(|s| s.pending_agents.len());
        assert_eq!(pending, Some(MAX_PENDING_AGENTS));
        // Oldest dropped, so the next agent gets a recent description.
        e.apply(&agent_ev("SubagentStart", "s1", "a1", "Explore"));
        assert_eq!(
            agents_of(&e, "s1")[0].description.as_deref(),
            Some(&*format!("task {}", MAX_PENDING_AGENTS * 2))
        );
    }
}
