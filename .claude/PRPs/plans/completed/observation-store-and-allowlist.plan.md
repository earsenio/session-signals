# Plan: Observation store + marker registry & `never_hide` allowlist

## Summary

Teach Session Signals to *record what sessions open with* — as salted hashes, never plaintext — and
to keep known-human openings out of that record entirely. Phase 2 builds the store (fingerprints,
counts, expiry). Phase 3 builds the two guards that run **before** it (built-in structural markers,
user-authored `never_hide`) plus the reveal-on-block safety valve. Nothing is proposed and nothing
new is hidden in this plan: Phase 4 turns observations into offers.

## User Story

As someone whose tray is drowned out by machine-spawned sessions,
I want Session Signals to quietly learn which openings repeat,
So that it can later offer me a filter I wrote myself instead of one it guessed.

## Problem → Solution

Today the first prompt is read **only** when a `first_prompt_prefix` rule already exists
(`engine.rs:710`) — and rules ship empty, so on a fresh install the read never happens and the app
learns nothing → the first prompt is read whenever observation is on, hashed at five prefix
lengths, and counted, while marker-flagged and allowlisted openings are dropped before any hash is
computed.

## Metadata

- **Complexity**: **Large** (two new modules, two existing modules touched, config + TS mirror,
  ~35 new tests; no UI)
- **Source PRD**: [.claude/PRPs/prds/headless-session-filter.prd.md](.claude/PRPs/prds/headless-session-filter.prd.md)
- **PRD Phase**: 2 (Observation store) + 3 (Marker registry + allowlist) — planned together because
  they meet at one line of ingest ordering (PRD "Parallelism Notes")
- **Estimated Files**: 10 changed (4 created, 6 updated)
- **Branch**: continue on `feat/headless-session-filter`

---

## UX Design

### Before

```
┌──────────────────────────────────────────────────────────┐
│  Fresh install, no rules                                 │
│  · transcript head is never read (no prompt rules)       │
│  · beacon.json: config, auth_token, captures             │
│  · a hidden session stays hidden even when it turns red  │
└──────────────────────────────────────────────────────────┘
```

### After

```
┌──────────────────────────────────────────────────────────┐
│  Fresh install, no rules                                 │
│  · transcript head read once per session (observation)   │
│  · beacon.json gains:                                    │
│      observe_salt : "9f3c…"        (secret, 64 hex)      │
│      observations : { "<fp>": {len,n,first,last} }       │
│  · grep beacon.json for any prompt text → nothing        │
│  · a hidden session that turns red REAPPEARS and notifies│
└──────────────────────────────────────────────────────────┘
```

### Interaction Changes

| Touchpoint | Before | After | Notes |
| --- | --- | --- | --- |
| Widget / tray | unchanged | unchanged | No proposal surface until Phase 5 |
| Hidden session hits `NEEDS_YOU` | stays hidden, no notification | un-hidden, notification fires, counter increments | Reveal-on-block (PRD decision 2). Dead code if headless really never blocks |
| Session opening with `<ide_selection>` etc. | n/a | never observed | Built-in marker, additive config only |
| A prefix in `never_hide` | n/a | never observed **and** never hidden | Outranks `ignore_rules` — fail open |
| `beacon.json` size | ~2 KB | + ~90 bytes per distinct opening × 5 lengths, pruned at 30 days | Hex + integers only |
| Disk I/O per session | none (default config) | one bounded 64 KB head-read, already off-lock | Reuses the existing debounced path |

---

## Mandatory Reading

| Priority | File | Lines | Why |
| --- | --- | --- | --- |
| P0 | [src-tauri/src/engine.rs](src-tauri/src/engine.rs#L700-L768) | 700-768 | `first_prompt_due` / `set_first_prompt` / `is_hidden` / `hidden_count` — the gate to change and the split-borrow idiom |
| P0 | [src-tauri/src/engine.rs](src-tauri/src/engine.rs#L894-L903) | 894-903 | `session_hidden` free fn — where allowlist precedence lands |
| P0 | [src-tauri/src/descriptor.rs](src-tauri/src/descriptor.rs#L58-L101) | 58-101 | `first_prompt` / `first_prompt_from_str` — **wrapper records are skipped, not returned**; this is why `human_marked` must be propagated |
| P0 | [src-tauri/src/descriptor.rs](src-tauri/src/descriptor.rs#L167-L173) | 167-173 | `is_wrapper` — the four built-in markers, to be relocated |
| P0 | [src-tauri/src/token.rs](src-tauri/src/token.rs#L28-L58) | 28-58 | Exact pattern to mirror for the salt (CSPRNG → hex → store key, tolerant save) |
| P0 | [src-tauri/src/lib.rs](src-tauri/src/lib.rs#L267-L292) | 267-292 | `maybe_refresh_hidden` — the single ingest call site both phases modify |
| P1 | [src-tauri/src/ignore.rs](src-tauri/src/ignore.rs#L92-L136) | 92-136 | `cwd_hidden` / `prompt_hidden` / `contains_ci` / `starts_with_ci` — the allowlist reuses these verbatim |
| P1 | [src-tauri/src/config.rs](src-tauri/src/config.rs#L56-L137) | 56-137 | `Config` field + `Default` + `sanitized()` clamping convention |
| P1 | [src-tauri/src/engine.rs](src-tauri/src/engine.rs#L507-L599) | 507-599 | `transition_to` / `reset_subagents` — reveal-on-block hook point, and one of **two** `Session` construction sites |
| P1 | [src-tauri/src/engine.rs](src-tauri/src/engine.rs#L411-L431) | 411-431 | The `BeaconTerminal` arm — the **other** `Session` construction site |
| P1 | [src-tauri/src/lib.rs](src-tauri/src/lib.rs#L127-L146) | 127-146 | `AppState` — where the salt + observations go |
| P1 | [src-tauri/src/lib.rs](src-tauri/src/lib.rs#L747-L785) | 747-785 | The sweep thread — where the debounced store flush + prune belong |
| P2 | [src/state/config.ts](src/state/config.ts) | all | The TS mirror; every new Rust field needs a twin here |
| P2 | [src-tauri/src/ignore.rs](src-tauri/src/ignore.rs#L138-L236) | 138-236 | Test style: named scenarios, comments say *why*, not *what* |
| P2 | [docs/IGNORE-RULES.md](docs/IGNORE-RULES.md) | all | Doc voice for the new `never_hide` section |

## External Documentation

| Topic | Source | Key Takeaway |
| --- | --- | --- |
| `sha2` | Already at 0.10.9 in `src-tauri/Cargo.lock:3141` (transitive via Tauri) | Adding `sha2 = "0.10"` to `[dependencies]` reuses the compiled crate — no new build cost, no new licence entry needed in `THIRD_PARTY_LICENSES.md` beyond confirming it is listed |
| `getrandom` | Already a direct dep (`Cargo.toml`, used by `token.rs`) | `getrandom::fill(&mut [u8; 32])` — same call, no version work |

No other external research needed — everything else is established internal pattern.

---

## Patterns to Mirror

### SECRET_IN_STORE

```rust
// SOURCE: src-tauri/src/token.rs:28-58
const STORE_FILE: &str = "beacon.json";
const TOKEN_KEY: &str = "auth_token";

pub fn generate() -> String {
    use std::fmt::Write;
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS CSPRNG unavailable — refusing to mint a weak token");
    let mut s = String::with_capacity(64);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub fn load_or_create(app: &AppHandle) -> String {
    if let Ok(store) = app.store(STORE_FILE) {
        if let Some(v) = store.get(TOKEN_KEY) {
            if let Some(s) = v.as_str() {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
        let token = generate();
        store.set(TOKEN_KEY, serde_json::Value::String(token.clone()));
        let _ = store.save();   // save failure tolerated: this run still works
        return token;
    }
    generate()
}
```

### LENIENT_CONFIG_LIST

```rust
// SOURCE: src-tauri/src/ignore.rs:53-66
pub fn deserialize_lenient<'de, D>(d: D) -> Result<Vec<Matcher>, D::Error> {
    let raw = Vec::<serde_json::Value>::deserialize(d)?;
    let mut out = Vec::with_capacity(raw.len());
    for v in raw {
        match serde_json::from_value::<Matcher>(v.clone()) {
            Ok(m) => out.push(m),
            Err(_) => eprintln!("beacon: dropping unrecognized ignore rule: {v}"),
        }
    }
    Ok(out)
}
```

### CONFIG_FIELD_AND_CLAMP

```rust
// SOURCE: src-tauri/src/config.rs:91-92, 110, 123-130
#[serde(default, deserialize_with = "crate::ignore::deserialize_lenient")]
pub ignore_rules: Vec<crate::ignore::Matcher>,
// ... Default:
ignore_rules: crate::ignore::IgnoreRules::defaults(),
// ... sanitized():
if self.stale_timeout_min == 0 {
    self.stale_timeout_min = DEFAULT_STALE_MIN;
}
```

### SPLIT_BORROW_FOR_HIDDEN_CHECK

```rust
// SOURCE: src-tauri/src/engine.rs:733-750
pub fn set_first_prompt(&mut self, id: &str, value: Option<String>) -> bool {
    // Split-borrow the two fields we need so `session_hidden` can read the
    // rules while we mutate the session.
    let Engine { sessions, ignore, .. } = self;
    match sessions.get_mut(id) {
        None => false,
        Some(s) => {
            let was = session_hidden(ignore, s);
            if value.is_some() { s.first_prompt = value; }
            s.first_prompt_checked_at = Some(Instant::now());
            session_hidden(ignore, s) != was
        }
    }
}
```

### OFF_LOCK_FILE_READ

```rust
// SOURCE: src-tauri/src/lib.rs:275-292
fn maybe_refresh_hidden(app: &AppHandle, ev: &HookEvent) -> bool {
    let Some(path) = ev.transcript_path.as_deref() else { return false; };
    let state = app.state::<AppState>();
    {
        let eng = state.engine.lock_safe();
        if !eng.first_prompt_due(&ev.session_id, Duration::from_secs(FIRST_PROMPT_RETRY_SECS)) {
            return false;
        }
    }
    // Bounded head read — lock intentionally NOT held here.
    let value = descriptor::first_prompt(path);
    let mut eng = state.engine.lock_safe();
    eng.set_first_prompt(&ev.session_id, value)
}
```

### CHAR_SAFE_TRUNCATION

```rust
// SOURCE: src-tauri/src/descriptor.rs:216-228
fn clean(s: &str) -> Option<String> {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() { return None; }
    if collapsed.chars().count() > MAX_LEN {
        let mut out: String = collapsed.chars().take(MAX_LEN - 1).collect();
        out.push('…');
        Some(out)
    } else { Some(collapsed) }
}
```

### TEST_STRUCTURE

```rust
// SOURCE: src-tauri/src/ignore.rs:157-186
/// Nothing is hidden out of the box. A shipped pattern would name a specific
/// third-party tool and silently hide sessions for users who don't run it.
#[test]
fn empty_defaults_hide_nothing() {
    assert!(IgnoreRules::defaults().is_empty());
    let r = IgnoreRules::new(IgnoreRules::defaults());
    assert!(!r.cwd_hidden(r"C:\x\.local\share\ecc-homunculus\projects\b4807c9eabf7"));
    assert!(!r.has_prompt_rules());
}
```

Conventions to hold: doc comment above every test naming the *risk* it covers; module-level `//!`
docs that explain the decision, not the mechanics; `eprintln!("beacon: …")` for every tolerated
failure; **no `println!`, no `dbg!`, and never any prompt text in a log line**.

---

## Files to Change

| File | Action | Justification |
| --- | --- | --- |
| `src-tauri/src/observe.rs` | CREATE | Salt, normalize, fingerprint, `Observations` store |
| `src-tauri/src/markers.rs` | CREATE | `MarkerPolarity`, built-in prefixes, additive config merge |
| `src-tauri/src/descriptor.rs` | UPDATE | Return `human_marked` alongside the first prompt; source the four markers from `markers.rs` |
| `src-tauri/src/ignore.rs` | UPDATE | `matches_cwd` / `matches_prompt` / `matches` so the allowlist reads correctly |
| `src-tauri/src/engine.rs` | UPDATE | Observation gate in `first_prompt_due`; `never_hide` precedence; reveal-on-block + counter |
| `src-tauri/src/config.rs` | UPDATE | `never_hide`, `markers`, `observe_enabled`, `observe_retain_days` |
| `src-tauri/src/lib.rs` | UPDATE | `AppState` fields, setup load, ingest wiring, sweep flush/prune |
| `src-tauri/Cargo.toml` | UPDATE | `sha2 = "0.10"` |
| `src/state/config.ts` | UPDATE | Mirror the four new config fields (typed passthrough, no UI yet) |
| `docs/IGNORE-RULES.md`, `CHANGELOG.md` | UPDATE | Document `never_hide`, observation, and what is stored |

## NOT Building

- **No proposals.** No clustering, no threshold, no `list_proposals` / `accept_proposal` — Phase 4.
- **No UI.** No settings editor, no tray line, no audit view — Phase 5. `src/state/config.ts` gains
  types only, as a passthrough (same as `ignore_rules` today).
- **No new hiding behaviour.** After this plan, exactly the same sessions are hidden as before,
  minus any the user allowlists, plus any revealed by the block guard.
- **No regex** in `never_hide` or `markers` (PRD decision).
- **No shipped `never_hide` defaults** and **no shipped `markers` overrides** — both `[]`.
- **No short-entry length warning** — unmeasured until Phase 6 (PRD decision 6).
- **No plaintext prefixes on disk**, in logs, or in error messages. Ever.
- **No change to `descriptor::extract`** (the display descriptor) — only `first_prompt`.
- **No fast path from hook payload fields** — that measurement is Phase 6.

---

## Step-by-Step Tasks

### Task 1: Fix the ingest contract first (the one place Phases 2 and 3 meet)

- **ACTION**: Before touching either phase, write down and implement the ordering as a single
  function signature in `lib.rs`, so both halves build against it.
- **IMPLEMENT**: The pipeline, in exactly this order:

  ```
  hook event
    → transcript head-read (off-lock, bounded)          [exists]
    → FirstPrompt { text, human_marked }                [Task 2]
    → human_marked?           → do not observe          [Task 10]
    → never_hide matches?     → do not observe          [Task 10]
    → observe(session_id, text)                         [Task 6]
    → engine.set_first_prompt(text)                     [exists, unchanged]
  ```

- **GOTCHA**: `set_first_prompt` must stay **last and unconditional**. It feeds the user's own
  `ignore_rules`, which are independent of observation; skipping it for an allowlisted session
  would be correct-by-accident today and wrong the moment precedence changes.
- **VALIDATE**: No code to test yet — but every later task references a step number here. If a task
  can't be placed in this list, it belongs to a different phase.

### Task 2: `descriptor::first_prompt` reports whether a human marker preceded the prompt

- **ACTION**: Change the return type to carry the marker flag. **This is load-bearing** — see
  GOTCHA.
- **IMPLEMENT**: In [descriptor.rs](src-tauri/src/descriptor.rs#L58-L101):

  ```rust
  /// A session's opening prompt plus whether Claude Code's own *human*
  /// interaction markers preceded it in the transcript.
  pub struct FirstPrompt {
      pub text: String,
      /// True when a wrapper record (slash command, IDE injection) appeared
      /// before `text`. The session's true opening was a human at the keyboard,
      /// so `text` is that human's typed prompt — never a spawner's injection.
      pub human_marked: bool,
  }
  ```

  `first_prompt_from_str` keeps its skip logic but sets a local `saw_marker = true` whenever
  `is_wrapper` rejects a candidate, and returns `FirstPrompt { text, human_marked: saw_marker }`.
  Keep a thin `pub fn first_prompt(path) -> Option<FirstPrompt>`.
- **MIRROR**: the existing loop; do not restructure it.
- **GOTCHA**: **`first_prompt_from_str` does not return `None` for a wrapper-opened session** — it
  *skips* the wrapper and returns the next real prompt (`descriptor.rs:366-373` proves this). The
  backtest that produced "0 false positives" excluded wrapper-opened sessions entirely. Without
  this flag, Phase 3's marker guard would be a no-op — it would test the returned text, which by
  construction never starts with a marker — and the measured 0-FP figure would not transfer to the
  running app. Only `is_wrapper` rejections set the flag; `isMeta`, `isSidechain` and
  `tool_result` skips must **not** (they are not evidence of a human).
- **IMPORTS**: none new.
- **VALIDATE**: `cargo test --manifest-path src-tauri/Cargo.toml descriptor` — plus a new test
  asserting an `<ide_selection>` record followed by `"please review the listener"` yields
  `human_marked: true`, and a bare `"please review the listener"` yields `false`.

### Task 3: `markers.rs` — polarity registry, built-ins immutable

- **ACTION**: Create the module and make `descriptor::is_wrapper` read its prefix list from it, so
  the four markers exist in exactly one place.
- **IMPLEMENT**:

  ```rust
  #[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
  #[serde(rename_all = "snake_case")]
  pub enum MarkerPolarity { Human, Machine }

  /// Claude Code's own structural markers for a *human* interaction. Built in and
  /// immutable: a user who could delete `<ide_selection>` would silently
  /// reintroduce a known false-positive class (PRD decision).
  pub const BUILTIN_HUMAN: &[&str] =
      &["<command-", "<local-command-", "<ide_opened_file>", "<ide_selection>"];

  #[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
  pub struct MarkerRule { pub prefix: String, pub polarity: MarkerPolarity }

  pub struct Registry { extra: Vec<MarkerRule> }

  impl Registry {
      /// Config markers are **additive only**. An entry colliding with a built-in
      /// prefix is dropped with a note — overriding `<ide_selection>` to Machine
      /// would be worse than deleting it.
      pub fn new(extra: Vec<MarkerRule>) -> Self { /* filter + eprintln */ }
      /// `None` = unclassified: merely *eligible* for clustering, neither forced
      /// nor blocked (PRD decision 4).
      pub fn polarity_of(&self, text: &str) -> Option<MarkerPolarity> { /* built-ins first */ }
      pub fn is_human(&self, text: &str) -> bool { /* == Some(Human) */ }
  }
  ```

- **MIRROR**: LENIENT_CONFIG_LIST for the `extra` list; `starts_with` semantics identical to
  `is_wrapper` (trim_start, then prefix match — note built-ins are matched **case-sensitively**,
  as `is_wrapper` does today; do not silently switch to `starts_with_ci`).
- **GOTCHA**: `is_wrapper` is on the descriptor hot path for *display* too. Keep it a pure function
  over `BUILTIN_HUMAN` — do **not** give it access to config, or a pure parser starts depending on
  user state and Phase 1's shipped behaviour changes.
- **VALIDATE**: tests — built-in prefixes classify Human; a config entry for `<task-notification>`
  classifies Machine; a config entry for `<ide_selection>` with polarity Machine is **dropped** and
  the built-in verdict stands; unknown text yields `None`.

### Task 4: `sha2` dependency + `observe::salt`

- **ACTION**: Add `sha2 = "0.10"` under `[dependencies]` in `src-tauri/Cargo.toml` with a comment
  matching the file's style (why, not what). Create `observe.rs` with the salt half.
- **IMPLEMENT**: `observe::salt::load_or_create(app) -> String` (64 hex chars) under store key
  `observe_salt`, plus `fn bytes(hex: &str) -> Vec<u8>` for hashing.
- **MIRROR**: SECRET_IN_STORE, verbatim structure.
- **GOTCHA**: **Never reuse `auth_token`.** Different purpose, different lifetime — `regenerate_token`
  exists and would silently re-key every stored fingerprint, orphaning the whole observation history.
  Add a test asserting the two store keys differ and that a `regenerate_token` call leaves
  `observe_salt` untouched.
- **IMPORTS**: `sha2::{Digest, Sha256}`, `tauri::AppHandle`, `tauri_plugin_store::StoreExt`.
- **VALIDATE**: `cargo test observe::salt` — 64 hex chars, stable across two `load_or_create` calls,
  distinct from a freshly generated token.

### Task 5: `observe::normalize` + `fingerprint`

- **ACTION**: Implement the two pure functions that turn a prompt into fingerprints.
- **IMPLEMENT**:

  ```rust
  /// Prefix lengths fingerprinted per session. Identical clusters on today's
  /// corpus at every length — carried as insurance against a spawner that varies
  /// before char 120, not as measured gain (PRD).
  pub const PREFIX_LENS: &[usize] = &[60, 70, 85, 100, 120];

  /// The text a rule would be written from: raw prompt, leading whitespace
  /// trimmed, first `len` chars (char-boundary safe). Case and internal
  /// whitespace preserved so the result is a **literal prefix** of the prompt.
  pub fn sample(prompt: &str, len: usize) -> Option<String>;

  /// Grouping key input: `sample` lowercased with whitespace runs collapsed, so
  /// a spawner varying indentation still clusters.
  fn normalize(sample: &str) -> String;

  /// `sha256(salt || normalize(sample))`, first 16 bytes as 32 hex chars.
  pub fn fingerprint(salt: &[u8], sample: &str) -> String;
  ```

- **MIRROR**: CHAR_SAFE_TRUNCATION for `sample` (`.chars().take(len)`, no ellipsis here).
- **GOTCHA**: The split between `sample` (literal) and `normalize` (fuzzy) is deliberate and has a
  consequence to record: two prompts differing only in whitespace share a fingerprint but have
  different samples, so the rule eventually offered may hide **fewer** sessions than the cluster
  counted. That is the safe direction (under-hide), and Phase 5's card previews real live sessions,
  so the user sees the truth. Do not "fix" it by offering the normalized text as the rule — that
  string is not a prefix of the prompt and `prompt_hidden` would never match it.
- **GOTCHA**: 128-bit truncation is intentional (halves store size; the realistic attack is a
  dictionary of candidate prefixes, which full width would not prevent either). Say so in the doc
  comment.
- **VALIDATE**: tests — same prompt → same fp; same prompt with a newline where another has a space
  → same fp; different salt → different fp; a multi-byte prompt (`"日本語のプロンプト…"`) truncates
  without panicking; a prompt shorter than `len` yields `None` for that length (nothing to sample);
  `sample()` output is always a literal prefix of `prompt.trim_start()`.

### Task 6: `observe::Observations` — records, dedup, prune

- **ACTION**: The store itself.
- **IMPLEMENT**:

  ```rust
  #[derive(Serialize, Deserialize, Clone, Debug)]
  pub struct Observation {
      /// Which prefix length produced this fingerprint. Phase 4's
      /// shortest-set-wins de-duplication needs it; it reveals nothing.
      pub len: u16,
      pub n: u32,
      /// Unix seconds (wall clock — `Instant` can't survive a restart).
      pub first: u64,
      pub last: u64,
  }

  #[derive(Default)]
  pub struct Observations {
      /// PERSISTED: fingerprint → record. The only thing that reaches disk.
      records: HashMap<String, Observation>,
      /// NOT PERSISTED: fingerprint → sample text, this run only. Upholds the
      /// invariant that a proposal is only ever surfaced while its text is live
      /// in memory — a pattern you cannot read is one you must not be asked to
      /// accept.
      samples: HashMap<String, String>,
      /// NOT PERSISTED: session ids already counted this run. The first-prompt
      /// read retries every 5s until resolved (`FIRST_PROMPT_RETRY_SECS`), so
      /// without this one session would inflate its own count on every retry.
      seen: HashSet<String>,
      dirty: bool,
  }

  impl Observations {
      pub fn observe(&mut self, salt: &[u8], session_id: &str, prompt: &str) -> bool;
      pub fn prune(&mut self, retain_days: u64, now: u64) -> bool;
      pub fn clear(&mut self);            // Phase 4 command needs it; free here
      pub fn take_dirty(&mut self) -> bool;
      pub fn to_json(&self) -> serde_json::Value;      // records only
      pub fn from_json(v: serde_json::Value) -> Self;  // tolerant, like config load
  }
  ```

  `observe` returns early `false` if `seen` already contains the id; otherwise inserts the id and,
  for each `PREFIX_LENS` entry that yields a `sample`, upserts `{len, n+1, first, last=now}`.
- **GOTCHA**: `seen` is never pruned during a run. That is deliberate — dropping an id on
  `SessionEnd` would let a straggler or a resumed session re-count. Growth is one UUID per session
  seen (~50 KB per thousand sessions); say so in the doc comment so it doesn't read as a leak.
- **GOTCHA**: The accepted residual (PRD): a restart *mid-session* re-counts that session once.
  Tolerated — inflation only makes a proposal appear sooner, and the false-positive guard is the
  marker/allowlist check, never the count. Do not add session-id hashes to fight it.
- **GOTCHA**: `samples` must never be serialized. Enforce it with a test that round-trips through
  `to_json` and greps the string for the sample text.
- **IMPORTS**: `std::collections::{HashMap, HashSet}`, `std::time::{SystemTime, UNIX_EPOCH}`.
- **VALIDATE**: tests — one prompt observed five times in a row counts once; two distinct sessions
  with the same opening reach `n == 2`; `first` is preserved and `last` advances; `prune(30, now)`
  drops a record whose `last` is 31 days old and keeps one at 29; serialized JSON contains no prompt
  substring; `from_json` on garbage yields an empty store rather than panicking.

### Task 7: Config — four new fields (+ TS mirror)

- **ACTION**: Add to `Config`, `Default`, `sanitized()`, and `src/state/config.ts`.
- **IMPLEMENT**:

  ```rust
  #[serde(default, deserialize_with = "crate::ignore::deserialize_lenient")]
  pub never_hide: Vec<crate::ignore::Matcher>,   // default: vec![]
  #[serde(default)]
  pub markers: Vec<crate::markers::MarkerRule>,  // default: vec![] (additive only)
  #[serde(default = "default_true")]
  pub observe_enabled: bool,                     // default: true
  #[serde(default)]
  pub observe_retain_days: u64,                  // default: 30, 0 → default in sanitized()
  ```

  TS twin in `src/state/config.ts` + `DEFAULT_CONFIG`, reusing `IgnoreMatcher` for `never_hide`.
- **MIRROR**: CONFIG_FIELD_AND_CLAMP.
- **GOTCHA**: `observe_enabled` is a deliberate addition beyond the PRD's field list — without it,
  a user who wants observation off has no switch until Phase 5 ships a UI, and `retain_days` cannot
  express "never". One boolean, default on (the PRD's surfacing design presumes observation runs by
  default, else no proposal could ever appear).
- **GOTCHA**: `markers` uses plain `#[serde(default)]`, not the lenient deserializer — its shape is
  a struct, not a tagged enum, so an unparseable entry is a genuine config error. If `Vec<MarkerRule>`
  ever gains variants, revisit.
- **VALIDATE**: `cargo test config` — a config JSON written by today's build (no new keys) loads with
  `never_hide: []`, `observe_enabled: true`, `observe_retain_days: 30`; `observe_retain_days: 0`
  sanitizes to 30. Then `npm run typecheck`.

### Task 8: Engine — observation gate in `first_prompt_due`

- **ACTION**: Let the head-read happen when observation is on, even with zero prompt rules.
- **IMPLEMENT**: add `observe_enabled: bool` to `Engine` (default `false`, so existing tests are
  untouched) + `pub fn set_observe_enabled(&mut self, on: bool)`. Change the first line of
  `first_prompt_due` (`engine.rs:710`) to:

  ```rust
  if !self.observe_enabled && !self.ignore.has_prompt_rules() {
      return false;
  }
  ```

- **GOTCHA**: **This is the change without which Phase 2 is inert.** On a default install
  `has_prompt_rules()` is false, so today no transcript head is ever read and the Observer would
  see nothing at all.
- **GOTCHA**: Keep the existing `self.ignore.cwd_hidden(&s.cwd)` short-circuit below it. A session
  already hidden by a cwd rule has nothing to propose, and skipping it saves the I/O.
- **VALIDATE**: tests — with no rules and `observe_enabled(false)`, `first_prompt_due` is false
  (regression guard on today's behaviour); with `observe_enabled(true)` it is true; a cwd-hidden
  session stays false either way.

### Task 9: lib.rs wiring — state, startup, ingest, flush

- **ACTION**: Thread salt + observations through `AppState` and call the ingest step.
- **IMPLEMENT**:
  1. `AppState` gains `observe_salt: Mutex<String>` (empty until setup, mirroring `token`) and
     `observations: Mutex<Observations>`.
  2. In `setup`, after `token::load_or_create`: load the salt, load `observations` from the store,
     and `eng.set_observe_enabled(cfg.observe_enabled)` / `eng.set_never_hide(...)` /
     `eng.set_markers(...)` alongside the existing `set_ignore_rules` call (`lib.rs:676`).
  3. In `set_config` (`lib.rs:422`), mirror the `ignore_rules` block for the three new engine-facing
     fields, and `refresh(&app)` when `never_hide` changed (sessions may become visible).
  4. In `maybe_refresh_hidden`, insert the Task 1 pipeline between the read and `set_first_prompt`.
  5. In the sweep thread (`lib.rs:752`), after `eng.sweep()`: `prune(retain_days)` then flush the
     store if `take_dirty()`.
- **GOTCHA**: Do the observation work **off the engine lock**, like the read it follows. Take the
  `observations` lock only for the duration of `observe()` and never while holding `engine`, or the
  two mutexes can be acquired in opposing orders.
- **GOTCHA**: Flushing on every observation would write `beacon.json` several times per session.
  The 15s sweep tick is already the app's heartbeat — piggyback on it. A crash between ticks loses
  at most 15s of counts, which only delays a proposal.
- **GOTCHA**: `set_config` must apply `observe_enabled: false` immediately, and that path should
  **not** clear existing observations — clearing is `clear_observations` (Phase 4), an explicit act.
- **VALIDATE**: `cargo build`; then manual: run the app, let a session start, confirm `beacon.json`
  grows `observe_salt` + `observations`, and that
  `grep -io "<a phrase from your own prompt>" beacon.json` returns nothing.

### Task 10: The two guards, at ingest, before fingerprinting

- **ACTION**: Implement the drop conditions from Task 1 step 3-4.
- **IMPLEMENT**: In the ingest helper:

  ```rust
  // A human marker preceded this prompt (slash command, IDE injection): the
  // session's opening was a person at the keyboard. Never observed.
  if fp.human_marked || registry.is_human(&fp.text) { /* skip observe */ }
  // The user has declared this opening their own. Allowlisted openings never
  // touch disk at all — strictly better than filtering at proposal time, and no
  // hash/plaintext comparison is ever needed.
  else if never_hide.matches(&ev.cwd, Some(&fp.text)) { /* skip observe */ }
  else { observations.observe(&salt, &ev.session_id, &fp.text); }
  ```

- **GOTCHA**: `registry.is_human(&fp.text)` looks redundant given `human_marked` — keep both. The
  flag covers "a marker record preceded the prompt"; the registry covers "this prompt itself opens
  with a marker", which is the case for a *config-added* marker the transcript reader knows nothing
  about.
- **GOTCHA**: Unclassified (`polarity_of == None`) must fall through to observation. Unclassified
  means *eligible*, so the worst case is one proposal the user reviews (PRD decision 4).
- **VALIDATE**: covered by Task 13's integration tests.

### Task 11: `ignore.rs` — allowlist-shaped API

- **ACTION**: Add read-correct names without churning the deny path.
- **IMPLEMENT**: rename the bodies of `cwd_hidden` / `prompt_hidden` to `matches_cwd` /
  `matches_prompt`; keep `cwd_hidden` / `prompt_hidden` as one-line `#[inline]` delegates (the deny
  path keeps reading as "hidden"); add
  `pub fn matches(&self, cwd: &str, first_prompt: Option<&str>) -> bool`.
- **GOTCHA**: Do not change matching semantics. `matches_prompt` keeps `trim_start` on both sides
  and `starts_with_ci`; the anchoring test at `ignore.rs:201` must pass untouched.
- **VALIDATE**: `cargo test ignore` — all existing tests unchanged, plus `matches()` firing on cwd
  alone, prompt alone, and neither.

### Task 12: Engine — `never_hide` precedence

- **ACTION**: Allowlist outranks every ignore rule.
- **IMPLEMENT**: `Engine` gains `never_hide: IgnoreRules` + `pub fn set_never_hide(&mut self, ...)`.
  `session_hidden` takes it and short-circuits:

  ```rust
  fn session_hidden(ignore: &IgnoreRules, never: &IgnoreRules, s: &Session) -> bool {
      // Fail open. Extra noise is recoverable; a hidden session you needed is not.
      if never.matches(&s.cwd, s.first_prompt.as_deref()) { return false; }
      if s.revealed { return false; }                       // Task 13
      ignore.cwd_hidden(&s.cwd)
          || s.first_prompt.as_deref().is_some_and(|p| ignore.prompt_hidden(p))
  }
  ```

- **MIRROR**: SPLIT_BORROW_FOR_HIDDEN_CHECK — `set_first_prompt` now destructures three fields.
- **GOTCHA**: Every `session_hidden` call site must pass the new argument: `set_first_prompt`,
  `is_hidden`, `hidden_count`, `rollup`, `snapshot` (engine.rs:742/747, 757, 765, 823, 852).
- **VALIDATE**: tests — a session hidden by a cwd rule becomes visible when the same cwd substring
  is added to `never_hide`; likewise for a prompt prefix; an overlapping pair (same prefix in both
  lists) stays **visible**; `hidden_count` reflects it.

### Task 13: Reveal-on-block guard + counter

- **ACTION**: A hidden session that needs the user is un-hidden and tallied.
- **IMPLEMENT**: `Session` gains `revealed: bool`; `Engine` gains `reveal_count: u64` +
  `pub fn reveal_count(&self) -> u64`. At the end of `transition_to`, when
  `state == State::NeedsYou`, split-borrow and: if the session is currently hidden, set
  `revealed = true` and `reveal_count += 1`. Reset `revealed = false` in the `SessionStart` arm,
  next to `reset_subagents` (`engine.rs:294`) — a genuine restart is a new run.
- **GOTCHA**: `revealed` must be **sticky**, not "visible while red". Otherwise the row vanishes
  again the moment you answer and the session returns to Working — a flicker that reads as a bug.
- **GOTCHA**: There are **two** `Session` construction sites — `transition_to`'s vacant arm
  (`engine.rs:554`) and the `BeaconTerminal` arm (`engine.rs:416`). Both need the new field or the
  build breaks (which is the good outcome; just don't be surprised).
- **NOTE (emergent, desirable)**: `process_event` checks `eng.is_hidden(...)` *after* `apply`
  (`lib.rs:205-214`), so a revealed session's NeedsYou notification now fires too. That is the right
  behaviour — state it in the doc comment so nobody later "fixes" it.
- **GOTCHA**: `reveal_count` is in-memory only, not persisted. It is a diagnostic for Phase 5's
  audit view, and a counter that survived restarts would need pruning semantics nobody asked for.
- **VALIDATE**: tests — a cwd-hidden session driven by `Notification(permission_prompt)` reappears
  in `snapshot()`, colours the rollup red, and increments `reveal_count`; it stays visible after the
  following `PostToolUse`/`Stop`; a `SessionStart` for the same id re-hides it; `reveal_count` is 0
  for an ordinary hidden session's whole lifecycle (the premise-holds case).

### Task 14: Docs

- **ACTION**: Document what is now stored and how to allowlist, in the existing voice.
- **IMPLEMENT**:
  - `docs/IGNORE-RULES.md`: a `## Keeping your own openings out (never_hide)` section after "Rule
    kinds" — same matcher shapes, precedence stated plainly ("`never_hide` always wins"), and a
    short "What Session Signals records" note: salted hashes of the first ~120 characters, counts
    only, pruned after 30 days, nothing readable, `observe_enabled: false` to switch it off.
  - `CHANGELOG.md` under `## [Unreleased] / ### Added`: two entries (observation store; `never_hide`
    + reveal-on-block), matching the existing bullets' length and tone.
- **GOTCHA**: The honest limitation goes in the doc, not just the PRD: hashing a short, low-entropy
  prompt is not anonymity against someone who can hash candidate strings. What it defeats is a
  readable prompt log sitting in JSON that gets synced, backed up, or attached to a bug report.
- **VALIDATE**: `npm run format:check` (prettier covers markdown); reread for MD060 table style
  (`| --- |`, spaces around every pipe).

---

## Testing Strategy

### Unit Tests

| Test | Input | Expected | Edge? |
| --- | --- | --- | --- |
| `human_marker_before_prompt_is_flagged` | `<ide_selection>` record, then a typed prompt | `human_marked: true`, text = typed prompt | — |
| `meta_and_tool_results_do_not_flag_human` | `isMeta` + `tool_result`, then a prompt | `human_marked: false` | ✅ |
| `builtin_markers_cannot_be_overridden` | config `{prefix:"<ide_selection>", polarity:"machine"}` | entry dropped, verdict Human | ✅ |
| `unclassified_marker_is_eligible` | `<task-notification>…`, empty config | `polarity_of == None` → observed | ✅ |
| `salt_is_stable_and_not_the_auth_token` | two `load_or_create`, one `token::generate` | equal to itself, ≠ token, 64 hex | — |
| `fingerprint_survives_whitespace_variation` | same prompt, `\n` vs `  ` | same fp | ✅ |
| `fingerprint_changes_with_salt` | same prompt, two salts | different fp | — |
| `sample_is_a_literal_prefix` | any prompt, each `PREFIX_LENS` | `prompt.trim_start().starts_with(sample)` | — |
| `multibyte_prompt_truncates_safely` | CJK/emoji prompt | no panic, ≤ len chars | ✅ |
| `short_prompt_yields_no_sample_at_long_lengths` | 20-char prompt | `Some` at none of 60..120 | ✅ |
| `retry_reads_count_once` | same session observed 5× | `n == 1` | ✅ |
| `two_sessions_same_opening_reach_two` | two ids, one opening | `n == 2`, `first` preserved | — |
| `prune_drops_expired_keeps_fresh` | records at 31 and 29 days | one dropped, one kept | ✅ |
| `store_json_contains_no_prompt_text` | observe a distinctive phrase → `to_json` | phrase absent; `samples` absent | ✅ |
| `from_json_tolerates_garbage` | `"[]"`, `"null"`, truncated object | empty store, no panic | ✅ |
| `first_prompt_due_requires_rules_or_observation` | no rules; observe off/on | false / true | ✅ |
| `never_hide_outranks_ignore_rules` | same prefix in both lists | visible | ✅ |
| `allowlisted_opening_is_never_stored` | prompt matching `never_hide` | store empty **and** not hidden | ✅ |
| `hidden_session_that_blocks_is_revealed` | cwd-hidden → `permission_prompt` | in `snapshot`, rollup red, count 1 | ✅ |
| `reveal_is_sticky_until_session_start` | revealed → `Stop` → `SessionStart` | visible, then hidden again | ✅ |
| `existing_config_json_loads_with_new_defaults` | today's config, no new keys | defaults filled, nothing reset | ✅ |

### Edge Cases Checklist

- [ ] Empty prompt / whitespace-only prompt → no sample, no observation
- [ ] Prompt shorter than the shortest prefix length
- [ ] Multi-byte and emoji prompts at every truncation boundary
- [ ] Missing / unreadable transcript → fail open, session stays visible, nothing observed
- [ ] 32 MB transcript → still one bounded 64 KB head-read (unchanged path)
- [ ] Store file absent, unreadable, or containing `observations: null`
- [ ] `regenerate_token` → `observe_salt` untouched, history intact
- [ ] Concurrent access: engine lock and observations lock never held together
- [ ] Config with an unknown `never_hide` matcher kind → dropped, rest of config intact
- [ ] `observe_enabled: false` mid-run → reads stop, existing records untouched
- [ ] Permission denied writing `beacon.json` → counts lost, app keeps running (eprintln only)

---

## Validation Commands

### Static Analysis

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
npm run typecheck
npm run lint
npm run format:check
```

EXPECT: clean. (These are exactly CI's steps — `.github/workflows/ci.yml:60-106`.)

### Unit Tests

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

EXPECT: the existing **82** tests still pass, plus ~35 new ones. Any pre-existing test that changes
behaviour is a red flag — this plan is additive except for `first_prompt_due`'s gate and
`descriptor::first_prompt`'s return type.

### Build

```bash
npm run build
cargo build --manifest-path src-tauri/Cargo.toml
```

EXPECT: no warnings.

### Manual Validation

- [ ] `npm run tauri dev`, start a normal terminal session, then check the store:
      `observe_salt` present (64 hex), `observations` has entries with `len`/`n`/`first`/`last` only.
- [ ] `grep -io "<a distinctive phrase you typed>" beacon.json` → **no match**.
- [ ] Add a `never_hide` entry matching your own opening, restart a session → no new observation
      entry appears for it.
- [ ] Add a `cwd_contains` ignore rule for a running session, confirm it disappears, then trigger an
      `AskUserQuestion` in it → the row reappears, tray goes red, a notification fires.
- [ ] Remove `observations` from the store by hand while the app runs → it rebuilds without error.
- [ ] Leave the app running with `observe_enabled: false` → no store growth, no head-reads.

---

## Acceptance Criteria

- [ ] Every task's VALIDATE step passes
- [ ] Store contains hex + integers only; no prompt substring is greppable (PRD metric: 0 bytes
      plaintext on disk)
- [ ] A session read many times in one run counts exactly once
- [ ] A `never_hide` prefix produces **no store entry at all** — verified by inspecting the store,
      not just the absence of a proposal
- [ ] An unclassified marker neither forces nor blocks observation
- [ ] A hidden session driven to `NEEDS_YOU` reappears in `snapshot()` and increments the counter
- [ ] The same sessions are hidden as before this plan, ± allowlist and reveals
- [ ] `never_hide` and `ignore_rules` overlapping on one prefix leaves the session visible
- [ ] All five validation command groups green

## Completion Checklist

- [ ] Module docs (`//!`) explain the decision, not the mechanics
- [ ] Every test carries a doc comment naming the risk it covers
- [ ] Tolerated failures use `eprintln!("beacon: …")`; no prompt text in any log line
- [ ] No `any` in the TS mirror; `npm run typecheck` clean
- [ ] `CHANGELOG.md` + `docs/IGNORE-RULES.md` updated
- [ ] `docs/internal/implementation-plan.md` §B reconciled with this plan
- [ ] No proposal/UI code leaked in from Phase 4/5
- [ ] Nothing hand-edited in `tauri.conf.json` / `Cargo.toml` version fields

## Risks

| Risk | Likelihood | Impact | Mitigation |
| --- | --- | --- | --- |
| Marker flag not propagated → 0-FP figure doesn't transfer | **M** | High — the whole precision claim | Task 2 is first and load-bearing; two dedicated tests |
| Salt reused from `auth_token` | L | High — `regenerate_token` orphans all history | Separate store key + a test asserting independence |
| Store write amplification | M | Low | Debounced flush on the existing 15s sweep tick |
| Lock-order inversion (engine ↔ observations) | L | High — deadlock | Never hold both; observation runs off-lock, like the read |
| `first_prompt_due` change causes head-reads for every session | **H by design** | Low | Already bounded (64 KB), debounced (5s), off-lock; cwd-hidden sessions still skip |
| A `Session` field added at one of two construction sites | M | Low | Compiler catches it; both sites listed in Task 13 |
| Whitespace-normalized cluster vs. literal rule mismatch | M | Low | Under-hides, never over-hides; Phase 5 previews real sessions |
| Scope creep into Phase 4 | M | Medium | `clear()` is the only Phase-4-facing method allowed in; no clustering code |

## Notes

### Deltas from the PRD introduced by this plan

Each is a plan-level refinement, not a reopened decision. Flagged so they can be struck.

| # | Delta | Why |
| --- | --- | --- |
| 1 | Record is `{fp → len, n, first, last}`, not `{fp, n, first, last}` | Phase 4's shortest-set-wins de-dup needs to know which prefix length produced a fingerprint. Two bytes, no privacy cost |
| 2 | Fingerprint truncated to 128 bits (32 hex) | Halves store size; the realistic attack is a candidate-prefix dictionary, which full width doesn't prevent either |
| 3 | New `observe_enabled` config flag (default `true`) | Without it there's no off switch until Phase 5, and `retain_days` can't express "never" |
| 4 | **`descriptor::first_prompt` returns `human_marked`** | The in-app reader *skips* wrapper records and returns the next real prompt, so a marker check on the returned text can never fire. Without the flag, Phase 3's guard is decorative and the backtest's exclusion of wrapper-opened sessions has no in-app equivalent. This is the substantive finding of this planning pass |
| 5 | `sample` (literal, raw) split from `normalize` (fuzzy, for grouping) | The offered rule must be a literal prefix of the prompt or `prompt_hidden` will never match it. Consequence: a rule may hide fewer sessions than its cluster counted — the safe direction |
| 6 | Config `markers` are additive-only; built-in prefixes cannot be overridden | Overriding `<ide_selection>` → Machine is strictly worse than deleting it, and the PRD already rules out deletion |
| 7 | `revealed` is sticky per session, reset on `SessionStart` | A non-sticky reveal flickers the row out the instant the user answers |
| 8 | `matches_cwd` / `matches_prompt` aliases on `IgnoreRules` | An allowlist calling `cwd_hidden()` to mean "matches" is a comment away from a real bug |

### Sequencing

Task 1 → 2 → 3 must be serial (the ingest contract, then the flag, then the registry). After that,
Tasks 4-9 (Phase 2) and 10-13 (Phase 3) are independent apart from Task 10, which needs both.
Task 14 last. If splitting across sessions, commit at 3, 9, and 13.

### What this plan deliberately leaves observable

After it lands, `hidden_count()` and `reveal_count()` exist and are read only by tests — exactly as
`hidden_count` has been since Phase 1. Phase 5 wires both into the audit view. That is the shipped
guarantee the PRD chose over an uncollectable precision number: the user gets a mechanism to see
what's hidden and why, not a claim about how often we're right.

---

*Source PRD: [.claude/PRPs/prds/headless-session-filter.prd.md](.claude/PRPs/prds/headless-session-filter.prd.md) · Phases 2 + 3*
*Generated: 2026-07-30*
