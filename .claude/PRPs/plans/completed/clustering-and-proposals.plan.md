# Plan: Clustering + proposals (PRD Phase 4)

## Summary

Turn the observation store into an *offer*. Group the salted fingerprints already
being recorded, de-duplicate across the five prefix lengths (shortest prefix
wins), clamp the threshold at 3 in code, and expose five commands —
`list_proposals`, `accept_proposal`, `dismiss_proposal`,
`never_suggest_proposal`, `clear_observations` — that let a user turn a pattern
the app actually saw into an `ignore_rules` entry, or refuse it permanently.

This plan also carries the three outstanding review findings from Phases 2–3
(**H1**, **M1**, **M2**) as Tasks 1–4. They are not optional garnish: H1 blocks
merge, and **M2 changes the shape of `Observation.len`**, which Phase 4's
shortest-prefix de-duplication reads. Fixing them after building on top of them
would mean rewriting Phase 4's core.

## User Story

As a developer running agentic tooling alongside interactive Claude Code,
I want the app to offer me a filter built from a pattern it actually observed,
so that I can silence background noise without hand-writing a rule or trusting
a shipped guess.

## Problem → Solution

**Current state**: fingerprints accumulate in `beacon.json` and nothing reads
them. `Observations` has no query surface at all — `records` and `samples` are
private with no accessors. The user has no way to see what repeated, and the
only path to a filter is hand-editing `ignore_rules` in the store file.

**Desired state**: `list_proposals` returns the eligible clusters, highest count
first, each carrying its live sample text **and the currently-visible sessions
that would disappear on accept**. One click writes a plain
`first_prompt_prefix` rule; one click writes a `never_hide` entry and purges the
cluster.

## Metadata

- **Complexity**: Large
- **Source PRD**: `.claude/PRPs/prds/headless-session-filter.prd.md`
- **PRD Phase**: 4 — Clustering + proposals
- **Depends on**: Phases 2 + 3 (complete, uncommitted working tree)
- **Carried review findings**: H1 (HIGH), M1, M2 — from
  `.claude/PRPs/reviews/observation-store-and-allowlist-review.md`
- **Estimated Files**: 9 (1 created, 8 updated)
- **Tasks**: 12

---

## UX Design

Phase 4 ships **commands only** — no window renders a proposal yet (that is
Phase 5). The "UX" below is the command-level contract Phase 5 will render, and
the PRD's card mock is the target it must be able to satisfy without a second
round-trip.

### Before

```
┌──────────────────────────────────────────────────────────┐
│  beacon.json                                             │
│    "observations": { "a3f1…": {len:60,n:7,…}, … }        │
│                                                          │
│  ← nothing reads this. No command, no accessor, no UI.   │
│    A user who wants a filter hand-edits ignore_rules.    │
└──────────────────────────────────────────────────────────┘
```

### After

```
> invoke("list_proposals")
[
  {
    fingerprint: "a3f1…",           // opaque; the id the actions take
    sample: "IMPORTANT: You are running in non-interactive --print mode…",
    len: 60,                         // chars in `sample`
    count: 7,                        // sessions in this cluster
    first_seen: 1753…, last_seen: 1753…,
    matching: [                      // ← load-bearing: what vanishes on accept
      { session_id:…, label:"ecc-homunculus", state:"working", … },
      { session_id:…, label:"ecc-homunculus", state:"ready",   … }
    ]
  }
]

> invoke("accept_proposal", { fingerprint: "a3f1…" })
    → ignore_rules += { kind:"first_prompt_prefix", value:"IMPORTANT: You are…" }
    → config saved, engine re-judged, widget + tray refreshed
```

### Interaction Changes

| Touchpoint | Before | After | Notes |
|---|---|---|---|
| Observation store | write-only | queryable via `list_proposals` | Still hash-only on disk; `sample` is run-memory only |
| Writing a filter | hand-edit `beacon.json` | `accept_proposal` | Goes through `set_config`, so persistence + refresh + `config-updated` are free |
| Refusing a pattern | no mechanism | `dismiss_proposal` (this run) / `never_suggest_proposal` (durable + purge) | PRD's three-way card |
| Threshold | none | `config.propose_threshold`, floor 3 clamped in `sanitized()` | Hand-editing the store below 3 is clamped on load |
| Tray menu | unchanged | **unchanged** | The "Session filtering: 1 suggestion…" line is Phase 5 |

---

## Mandatory Reading

| Priority | File | Lines | Why |
|---|---|---|---|
| P0 | `src-tauri/src/observe.rs` | 1–238 | The store being queried. `sample`, `fingerprint`, `Observations`' three side tables, `to_json`/`from_json` |
| P0 | `.claude/PRPs/reviews/observation-store-and-allowlist-review.md` | 30–139 | H1/M1/M2 in full, including M2's four ordered steps and the prerequisite that makes them safe |
| P0 | `src-tauri/src/engine.rs` | 806–826 | `set_first_prompt` — H1's fix site |
| P0 | `src-tauri/src/engine.rs` | 606–625 | `transition_to`'s reveal guard — the pattern H1's fix mirrors |
| P0 | `src-tauri/src/engine.rs` | 977–994 | `session_hidden` — the single hidden-ness authority; the preview must route through it |
| P0 | `src-tauri/src/ignore.rs` | 92–141 | `matches_cwd` / `matches_prompt` / `matches` / `has_prompt_rules`; `IgnoreRules.matchers` is private |
| P0 | `src-tauri/src/lib.rs` | 467–550 | `set_config` — the whole apply path (port, engine swaps, save, `config-updated` emit). Accept/never-suggest reuse it wholesale |
| P1 | `src-tauri/src/engine.rs` | 926–960 | `snapshot()` — the `SessionView` construction Task 6 extracts |
| P1 | `src-tauri/src/lib.rs` | 353–387 | `observe_opening` — the established lock order (config → salt → observations, never nested under the engine lock) |
| P1 | `src-tauri/src/lib.rs` | 908–925 | The sweep tick's prune + dirty-flush; where `clear_observations` must also force a save |
| P1 | `src-tauri/src/config.rs` | 149–174 | `sanitized()` — where the threshold floor is clamped |
| P1 | `src-tauri/src/config.rs` | 85–123 | The four Phase-2/3 fields; the doc-comment density a new field must match |
| P1 | `src/state/config.ts` | 26–87 | The TS mirror. **A field missing here is silently reset on every save** — see Task 5's GOTCHA |
| P2 | `src-tauri/src/ignore.rs` | 143–160 | `contains_ci` / `starts_with_ci` — private; do not re-implement, route through `matches_prompt` |
| P2 | `.claude/PRPs/prds/headless-session-filter.prd.md` | 158–232 | The `never_hide` decisions, the in-memory-sample invariant, and the proposal card mock |
| P2 | `src/state/types.ts` | 8–34 | The existing TS `SessionView` — the proposal mirror reuses it, never redeclares it |

## External Documentation

No external research needed — this phase uses only established internal
patterns (`tauri::command`, `tauri_plugin_store`, serde) plus `sha2`, already
in the tree and already wrapped by `observe::fingerprint`. No new dependency.

---

## Patterns to Mirror

### SPLIT_BORROW_FOR_HIDDEN_CHECK
```rust
// SOURCE: src-tauri/src/engine.rs:606-625
if state == State::NeedsYou {
    let Engine {
        sessions,
        ignore,
        never_hide,
        reveal_count,
        ..
    } = self;
    if let Some(s) = sessions.get_mut(&ev.session_id) {
        if !s.revealed && session_hidden(ignore, never_hide, s) {
            s.revealed = true;
            *reveal_count += 1;
        }
    }
}
```
`session_hidden` is a free function precisely so the rules can be borrowed
immutably while the session map is borrowed mutably. Task 1 needs the same
destructure in `set_first_prompt`, extended with `reveal_count`.

### LENIENT_PER_ENTRY_PARSE
```rust
// SOURCE: src-tauri/src/ignore.rs:53-66
let raw = Vec::<serde_json::Value>::deserialize(d)?;
let mut out = Vec::with_capacity(raw.len());
for v in raw {
    match serde_json::from_value::<Matcher>(v.clone()) {
        Ok(m) => out.push(m),
        Err(_) => eprintln!("beacon: dropping unrecognized ignore rule: {v}"),
    }
}
Ok(out)
```
One bad entry drops one entry, never the whole collection. Task 2 applies this
shape to `Observations::from_json` (a map, not a list).

### CONFIG_FIELD_AND_CLAMP
```rust
// SOURCE: src-tauri/src/config.rs:110-118, 168-170
/// Whether Session Signals reads session openings to look for repeating
/// patterns (salted-hash counts only — see `observe.rs`). On by default:
/// the eventual filter-proposal surface presumes observation runs.
#[serde(default = "default_observe_enabled")]
pub observe_enabled: bool,

// …in sanitized():
if self.observe_retain_days == 0 {
    self.observe_retain_days = DEFAULT_OBSERVE_RETAIN_DAYS;
}
```
Every field carries a `#[serde(default…)]` and a doc comment explaining *why*
the default is what it is; every bound is enforced in `sanitized()`, not at the
call site.

### COMMAND_REUSES_SET_CONFIG
```rust
// SOURCE: src-tauri/src/lib.rs:517-533
if new.ignore_rules != old.ignore_rules {
    let state = app.state::<AppState>();
    let mut eng = state.engine.lock_safe();
    eng.set_ignore_rules(IgnoreRules::new(new.ignore_rules.clone()));
    drop(eng);
    // Re-judged sessions may drop out of (or back into) the widget/tray.
    refresh(&app);
}
```
`set_config` already owns persistence, the engine swap, the refresh, and the
`config-updated` broadcast. Tasks 9's accept/never-suggest **call it** rather
than re-implementing any of that.

### OFF_LOCK_THEN_LOCK
```rust
// SOURCE: src-tauri/src/lib.rs:338-350
{
    let eng = state.engine.lock_safe();
    if !eng.first_prompt_due(&ev.session_id, Duration::from_secs(FIRST_PROMPT_RETRY_SECS)) {
        return false;
    }
}
// Bounded head read — lock intentionally NOT held here.
let fp = descriptor::first_prompt(path);
if let Some(fp) = &fp {
    observe_opening(app, &ev.session_id, &ev.cwd, fp);
}
let mut eng = state.engine.lock_safe();
eng.set_first_prompt(&ev.session_id, fp.map(|fp| fp.text))
```
A scoped block releases one lock before the next is taken. `list_proposals`
must do the same: build under the observations lock, drop it, *then* take the
engine lock for previews.

### IMMUTABLE_RULE_DERIVATION
```rust
// SOURCE: .claude/rules/common/coding-style.md — "ALWAYS create new objects,
// NEVER mutate existing ones"
pub fn with(&self, extra: Matcher) -> IgnoreRules {
    let mut matchers = self.matchers.clone();
    matchers.push(extra);
    IgnoreRules { matchers }
}
```
Task 6's `with` returns a new set; the engine's own `ignore` is never touched
by a preview.

### TEST_STRUCTURE
```rust
// SOURCE: src-tauri/src/observe.rs:345-360
/// Force an artificial `first` so a later upsert overwriting it would
/// be caught — the entry API must only set `first` on insert.
#[test]
fn two_sessions_same_opening_reach_two() {
    let mut o = Observations::default();
    let salt = b"salt";
    let p = "please review the listener implementation end to end thoroughly";
    assert!(o.observe(salt, "session-1", p));
    let fp = fingerprint(salt, &sample(p, PREFIX_LENS[0]).unwrap());
    o.records.get_mut(&fp).unwrap().first = 1_000;
    assert!(o.observe(salt, "session-2", p));
    let rec = o.records.get(&fp).unwrap();
    assert_eq!(rec.n, 2);
    assert_eq!(rec.first, 1_000, "first must not be overwritten");
}
```
Doc comment states the failure mode being pinned; a sentinel value catches the
regression; assertions carry messages. In-module tests reach private fields
directly — do the same rather than widening visibility for tests.

---

## Files to Change

| File | Action | Justification |
|---|---|---|
| `src-tauri/src/proposals.rs` | CREATE | Pure clustering/eligibility/dedup logic, unit-testable with no `AppHandle` (the harness gap the Phase 2/3 report documented) |
| `src-tauri/src/observe.rs` | UPDATE | M1 + M2 fixes; query accessors; `dismissed` side table; `purge_family` |
| `src-tauri/src/engine.rs` | UPDATE | H1 fix; `view_of` extraction; `preview_hidden_by` |
| `src-tauri/src/ignore.rs` | UPDATE | `IgnoreRules::with` |
| `src-tauri/src/config.rs` | UPDATE | `propose_threshold` + floor-3 clamp |
| `src-tauri/src/lib.rs` | UPDATE | Five commands + handler registration + `mod proposals` |
| `src/state/config.ts` | UPDATE | Mirror `propose_threshold` (mandatory — see Task 5 GOTCHA) |
| `src/state/proposals.ts` | CREATE | TS mirror of `Proposal`, reusing the existing `SessionView` |
| `docs/IGNORE-RULES.md`, `CHANGELOG.md` | UPDATE | Proposal flow, the count-vs-preview caveat, and review finding L4's "until it restarts" clause |

## NOT Building

- **The tray menu line.** `Session filtering: N suggestions…` is Phase 5's
  success signal, and adding it here would ship a UI affordance with no card
  behind it.
- **Any settings UI.** No `Settings.tsx` change at all. Phase 5.
- **The audit view / `hidden_count` surfacing / reveal-count display.** Phase 5.
- **Real corpus fixtures.** Phase 4's replay test builds its corpus in-test.
  Captured-session fixtures are Phase 6's whole purpose; asserting "exactly 2
  proposals on the 577-session corpus" here would mean inventing the corpus.
- **A minimum sample length for proposal eligibility.** See Risks — PRD decision
  6 forbids shipping an unmeasured number, and Phase 6 derives it.
- **Persisting dismissals.** Run-only, matching the sample table they gate.
- **Retroactive purge on a hand-written `never_hide` entry.** Hashes are
  one-way; the PRD documents this. Task 8 adds a *proposal-time* `never_hide`
  check, which covers the user-visible symptom without claiming a purge.
- **Machine-polarity auto-classification.** The registry exists; no marker is
  confirmed Machine. `Could` in the PRD's MoSCoW.
- **Fixing M3, L2, L3.** M3 is Phase 5's prerequisite (the UI that makes it
  reachable ships there); L2/L3 are noted-only.
- **Changing `PREFIX_LENS`.** Five lengths stay. M2 changes only what happens
  *below* the shortest of them.

---

## Step-by-Step Tasks

### Task 1: H1 — reveal a session blocked *before* it was classified

- **ACTION**: Close the reveal-on-block hole in `engine.rs::set_first_prompt`.
- **IMPLEMENT**: Extend the existing destructure with `reveal_count`, and after
  updating `first_prompt` / `first_prompt_checked_at`:
  ```rust
  let now_hidden = session_hidden(ignore, never_hide, s);
  // A session that is *already* blocked must not be hidden by a late
  // classification. `transition_to` guards the other direction (hidden, then
  // blocked); this is the same valve at the other point where hidden-ness can
  // flip — and it is the *normal* ordering for a first-prompt rule, since
  // `SessionStart` fires before any prompt exists so classification is always
  // deferred to a retry ≥5s later.
  if now_hidden && !was && s.state == State::NeedsYou && !s.revealed {
      s.revealed = true;
      *reveal_count += 1;
      return false; // hidden-ness did not change: it stayed visible
  }
  session_hidden(ignore, never_hide, s) != was
  ```
- **MIRROR**: SPLIT_BORROW_FOR_HIDDEN_CHECK.
- **IMPORTS**: none new (`State` is already in scope in `engine.rs`).
- **GOTCHA**: Compute `now_hidden` *before* setting `s.revealed`, exactly as
  `transition_to` does — `session_hidden` short-circuits on `revealed`, so
  setting it first makes the guard test the flag it is about to write. The
  early `return false` is correct: `was` was `false` and the session stays
  visible, so hidden-ness genuinely did not change.
- **VALIDATE**: New test `session_blocked_before_classification_is_not_hidden`:
  drive a session to `NeedsYou` with a `first_prompt_prefix` rule installed but
  no prompt yet, then `set_first_prompt(Some(matching_prompt))`. Assert
  `is_hidden == false`, `snapshot().len() == 1`, `rollup() == Rollup::Red`,
  `reveal_count() == 1`. The pre-fix tree produces
  `hidden=true, snapshot_len=0, rollup=Grey, reveal_count=0` — run the test
  before the fix to see it fail.

### Task 2: M1 — per-entry lenient load for observations

- **ACTION**: Stop one malformed record from discarding the entire history.
- **IMPLEMENT**: In `observe.rs::from_json`, parse to
  `HashMap<String, serde_json::Value>` first, then `from_value::<Observation>`
  per entry, dropping failures with
  `eprintln!("beacon: dropping unreadable observation record {k}")` — the key
  only, **never the value** (a value is hash + counts, but the habit of never
  logging store contents is the one that keeps prompt text out of logs). Add
  `#[serde(default)]` at the container level of `Observation` and `Default` to
  its derive list.
- **MIRROR**: LENIENT_PER_ENTRY_PARSE.
- **IMPORTS**: none new.
- **GOTCHA**: Container-level `#[serde(default)]` requires `Observation:
  Default`. A record that loses `last` then defaults to `0` and is pruned on
  the next sweep — that is the right direction (a record we cannot date should
  not accumulate), but say so in a comment so it does not read as an oversight.
- **VALIDATE**: Rewrite `from_json_tolerates_garbage` — it currently
  *demonstrates* the all-or-nothing behaviour rather than guarding against it.
  New assertions: a map with one good and one malformed record keeps the good
  one; a record missing `len` still loads with `len: 0`; a top-level non-object
  (`[]`, `null`) still yields an empty store.

### Task 3: M2 step 1 — intra-call fingerprint de-duplication

- **ACTION**: Make `observe()` count each *distinct fingerprint* at most once
  per call. **Do this before Task 4** — the clamp without it turns every short
  opening into `n = 5` on first sighting, straight through the threshold-3
  floor, breaking the PRD's "0 double-counted observations within one run".
- **IMPLEMENT**: In `observe()`, a local `HashSet<String>` of fingerprints
  already counted this call; `if !counted.insert(fp.clone()) { continue; }`
  before the `records.entry(fp)` upsert.
- **MIRROR**: the existing `seen` set — same idea one level down (session-level
  dedup across calls; fingerprint-level dedup within one call).
- **IMPORTS**: `HashSet` already imported.
- **GOTCHA**: The collision is reachable *today*, not just under the clamp:
  `normalize` collapses whitespace, so a prompt whose chars 61–70 are all
  whitespace (a newline plus indentation — common in injected machine openings)
  makes `sample(60)` and `sample(70)` fingerprint identically. Do not treat
  this task as merely preparatory.
- **VALIDATE**: New test `whitespace_tail_collision_counts_once` — a prompt of
  60 visible chars followed by 15 whitespace chars then more text; assert the
  colliding fingerprint's `n == 1` after a single `observe`.

### Task 4: M2 steps 2–4 — clamp `sample`, record the real length

- **ACTION**: Hash whatever characters exist instead of refusing short prompts
  (author's call, recorded in the review).
- **IMPLEMENT**:
  1. `sample` returns `Some(trimmed.chars().take(len).collect())` with no length
     floor, **but** returns `None` when the trimmed prompt is empty.
  2. In `observe()`, store the sample's actual char count in `Observation.len`,
     not the nominal `PREFIX_LENS` entry.
  3. Update `sample_is_a_literal_prefix`, which asserts
     `s.chars().count() == len`, to `<= len` plus the still-load-bearing
     `p.trim_start().starts_with(&s)`.
  4. Replace `short_prompt_yields_no_sample_at_long_lengths` with
     `short_prompt_is_sampled_at_its_own_length`.
- **MIRROR**: CHAR_SAFE_TRUNCATION — `chars().take(n)`, never byte slicing.
- **IMPORTS**: none new.
- **GOTCHA**: The empty guard is not cosmetic. Without it a whitespace-only
  opening fingerprints the empty string, five identical times, and a cluster of
  three would propose a rule with an empty `value` —
  `ignore::starts_with_ci` returns `false` on an empty prefix, so the rule would
  hide nothing while looking to the user like an accepted filter. Refuse it at
  the source.
- **VALIDATE**: `sample("", 60).is_none()`, `sample("   \n\t ", 60).is_none()`.
  A 40-char prompt observed once produces **exactly one** record (Task 3's dedup
  collapses all five lengths) with `len == 40`. Multibyte: a 5-grapheme CJK
  prompt records `len == 5` and does not panic.

### Task 5: `propose_threshold` config field, floor 3 in code

- **ACTION**: Add the threshold, clamped where every other bound is clamped.
- **IMPLEMENT**: In `config.rs`:
  ```rust
  /// Minimum cluster size before an observed opening is offered as a filter.
  /// **Floored at [`MIN_PROPOSE_THRESHOLD`] in `sanitized()`, not in the UI** —
  /// measured leakage on the research corpus was 26 human patterns at 1, 3 at
  /// 2, and 0 at 3, and a UI-only default is bypassable by hand-editing this
  /// file.
  #[serde(default = "default_propose_threshold")]
  pub propose_threshold: u32,
  ```
  plus `pub const DEFAULT_PROPOSE_THRESHOLD: u32 = 3;`,
  `pub const MIN_PROPOSE_THRESHOLD: u32 = 3;`,
  `fn default_propose_threshold() -> u32 { DEFAULT_PROPOSE_THRESHOLD }`, the
  `Default for Config` entry, and in `sanitized()`:
  `if self.propose_threshold < MIN_PROPOSE_THRESHOLD { self.propose_threshold = MIN_PROPOSE_THRESHOLD; }`
- **MIRROR**: CONFIG_FIELD_AND_CLAMP.
- **IMPORTS**: none new.
- **GOTCHA**: **The TS mirror is mandatory, not cosmetic.** `Settings.tsx`
  sends the whole `Config` object on every save; a field absent from the TS
  interface is absent from the JSON, `#[serde(default…)]` fills it, and the
  user's threshold silently resets on the next unrelated toggle. Add
  `propose_threshold: number` to `Config` and `propose_threshold: 3` to
  `DEFAULT_CONFIG` in `src/state/config.ts` **in this task**, not later.
- **VALIDATE**: Extend the existing `existing_config_json_loads_with_new_defaults`
  (a config JSON predating this field must load with `propose_threshold == 3`).
  New test `propose_threshold_below_floor_is_clamped`: `1` and `0` both
  sanitize to `3`; `5` is preserved. `npm run typecheck` clean.

### Task 6: `IgnoreRules::with` + `Engine::preview_hidden_by`

- **ACTION**: Give the proposal card a preview that cannot drift from the
  behaviour it predicts.
- **IMPLEMENT**:
  1. `ignore.rs`: `pub fn with(&self, extra: Matcher) -> IgnoreRules` — clone
     the vec, push, return a new set. Doc-comment it as the "what would happen
     if…" constructor.
  2. `engine.rs`: extract `snapshot()`'s per-session `SessionView` construction
     into `fn view_of(&self, id: &str, s: &Session, now: Instant) -> SessionView`
     and call it from `snapshot()` (behaviour identical — this is a pure
     extraction).
  3. `engine.rs`:
     ```rust
     /// Which currently-visible sessions would disappear if `matcher` were
     /// appended to the ignore rules. Computed by re-running the real
     /// `session_hidden` against a candidate rule set, so the preview can never
     /// drift from the behaviour it predicts — including `never_hide`
     /// precedence and the sticky `revealed` flag, both of which mean a session
     /// matching the pattern may nonetheless *not* disappear.
     pub fn preview_hidden_by(&self, matcher: crate::ignore::Matcher) -> Vec<SessionView>
     ```
     Filter to `!session_hidden(&self.ignore, nh, s) && session_hidden(&candidate, nh, s)`,
     map through `view_of`, sort by label for a stable card.
- **MIRROR**: IMMUTABLE_RULE_DERIVATION; `snapshot()` for the sort-stability
  habit.
- **IMPORTS**: `crate::ignore::Matcher` in `engine.rs` (`IgnoreRules` is
  already imported).
- **GOTCHA**: Do **not** hand-roll the prefix test with `starts_with`.
  `matches_prompt` lowercases both sides and trims leading whitespace; a
  hand-rolled preview would disagree with the rule on exactly the cases users
  notice. Routing through a candidate `IgnoreRules` makes agreement structural.
- **VALIDATE**: New tests — `preview_lists_only_sessions_that_would_vanish`
  (three sessions, one matching, one already hidden by an existing rule, one
  unrelated → preview has exactly one entry); `preview_excludes_never_hide_and_revealed`
  (a session matching the pattern but allowlisted, and one with `revealed`
  set, both absent from the preview); `preview_does_not_mutate_engine_rules`
  (`hidden_count()` unchanged after the call).

### Task 7: `Observations` query surface + dismissals + family purge

- **ACTION**: Open a read path without making `records`/`samples` public.
- **IMPLEMENT** in `observe.rs`:
  - `pub fn iter_with_samples(&self) -> impl Iterator<Item = (&str, &Observation, &str)>`
    — only fingerprints whose sample is live this run. This *is* the PRD's
    in-memory invariant, expressed as the only way to enumerate.
  - `pub fn sample_for(&self, fp: &str) -> Option<&str>`
  - a third run-only side table `dismissed: HashMap<String, u32>` (fingerprint →
    count at dismissal), with `pub fn dismiss(&mut self, fp: &str, at_count: u32)`
    and `pub fn dismissed_at(&self, fp: &str) -> Option<u32>`.
  - `pub fn purge_family(&mut self, fp: &str) -> usize` — remove `fp`, plus
    every other fingerprint whose live sample is prefix-related to `fp`'s (in
    either direction, case-insensitively: the 60-char parent and the 120-char
    child are one family). Drops the matching `samples` and `dismissed` entries
    too, sets `dirty`, returns how many records went.
- **MIRROR**: the existing `seen` table's "NOT PERSISTED" doc-comment style;
  `prune`'s `dirty`-setting and `HashSet` orphan cleanup.
- **IMPORTS**: none new.
- **GOTCHA**: `purge_family` can only relate fingerprints that have live
  samples — a record carried over from a previous run has no sample and cannot
  be matched to a family. The PRD already documents this ("hashes are one-way");
  put it in the doc comment rather than letting a reader assume the purge is
  total. `clear_observations` is the escape hatch.
- **VALIDATE**: `purge_family_drops_the_whole_prefix_chain` — observe a long
  prompt (records at several lengths), purge via the 100-length fingerprint,
  assert every prefix-related record is gone and an unrelated cluster survives.
  `dismissed_survives_only_the_run` — `Observations::from_json(o.to_json())`
  has an empty `dismissed` (it never reaches disk).

### Task 8: `proposals.rs` — eligibility + shortest-prefix de-duplication

- **ACTION**: The pure core. No `AppHandle`, no `Engine` — fully unit-testable.
- **IMPLEMENT**:
  ```rust
  #[derive(Serialize, Clone, Debug)]
  pub struct Proposal {
      /// Opaque cluster id — what `accept_proposal` / `dismiss_proposal` /
      /// `never_suggest_proposal` take. Never shown to the user.
      pub fingerprint: String,
      /// The literal opening a rule would be written from. Live in memory
      /// only — a pattern you cannot read is one you must not be asked to
      /// accept, so a cluster with no live sample is not a proposal.
      pub sample: String,
      pub len: u16,
      pub count: u32,
      pub first_seen: u64,
      pub last_seen: u64,
      /// Currently-visible sessions this rule would hide. Left **empty** by
      /// `build` — the command layer fills it from the engine, which this
      /// module deliberately does not depend on. May be shorter than `count`:
      /// `count` groups on the whitespace-normalized form while a rule is a
      /// literal prefix, and only sessions live *right now* can appear.
      pub matching: Vec<crate::engine::SessionView>,
  }

  pub fn build(
      obs: &Observations,
      threshold: u32,
      ignore: &IgnoreRules,
      never_hide: &IgnoreRules,
  ) -> Vec<Proposal>
  ```
  Pipeline, in order:
  1. `obs.iter_with_samples()` — no live sample, no proposal.
  2. `rec.n >= threshold.max(MIN_PROPOSE_THRESHOLD)` — clamp again here, so the
     floor holds even if a caller passes an unsanitized value.
  3. Drop when `ignore.matches_prompt(sample)` — the user already accepted a
     rule covering this (review finding **L1**, folded in as planned).
  4. Drop when `never_hide.matches_prompt(sample)` — covers records stored
     *before* a hand-written allowlist entry existed, which ingest filtering
     cannot reach.
  5. Drop when `obs.dismissed_at(fp)` is `Some(n)` and `rec.n <= n` — dismissal
     lapses as the cluster grows, per the PRD.
  6. **Shortest-prefix-wins**: sort surviving candidates by `sample` char count
     ascending (ties broken by fingerprint for determinism), then keep each only
     if no already-kept sample is a prefix of it — tested via a throwaway
     `IgnoreRules::new(vec![FirstPromptPrefix { value: kept.sample.clone() }])`
     and `matches_prompt(candidate.sample)`, so the dedup agrees with the rule
     semantics by construction.
  7. Sort the result by `count` desc, then `last_seen` desc, then `fingerprint`
     asc.
- **MIRROR**: COMMAND_REUSES_SET_CONFIG is *not* used here (no side effects);
  mirror `ignore.rs`'s module-doc density and TEST_STRUCTURE for the tests.
- **IMPORTS**: `crate::engine::SessionView`, `crate::ignore::{IgnoreRules, Matcher}`,
  `crate::observe::Observations`, `crate::config::MIN_PROPOSE_THRESHOLD`,
  `serde::Serialize`. Register `pub mod proposals;` in `lib.rs`.
- **GOTCHA**: `never_hide` is checked **prompt-only**. A `CwdContains`
  allowlist entry cannot be evaluated against a fingerprint — there is no cwd
  in the store, by design. Say so in a comment; it is a real gap, in the
  fail-open direction (the proposal still surfaces, the user still decides).
- **VALIDATE**: New in-module tests — `below_threshold_is_not_proposed`;
  `cluster_without_live_sample_is_not_proposed`;
  `shortest_prefix_wins_across_lengths` (one long prompt observed 3×, expect
  exactly **one** proposal, the shortest sample); `two_families_yield_two_proposals`;
  `already_covered_by_ignore_rule_is_skipped`; `never_hide_prefix_suppresses_the_proposal`;
  `dismissed_returns_when_the_cluster_grows`; `ordering_is_highest_count_first`;
  `threshold_below_floor_is_clamped_in_build`.

### Task 9: The five commands

- **ACTION**: Wire the pure core to the app.
- **IMPLEMENT** in `lib.rs`:
  ```rust
  /// Eligible filter proposals, highest count first, each with the live
  /// sessions it would hide. Returns the full list (Phase 5's card renders
  /// only the head — a list on screen invites bulk-accept, which is auto-hide
  /// with extra steps — but the count drives the tray line).
  #[tauri::command]
  fn list_proposals(app: AppHandle) -> Vec<proposals::Proposal>
  ```
  - `list_proposals`: read `threshold` + rules from config; build under the
    observations lock; **drop it**; take the engine lock and fill each
    `matching` via `preview_hidden_by(Matcher::FirstPromptPrefix { value: p.sample.clone() })`.
  - `accept_proposal(app, fingerprint) -> Result<(), String>`: resolve the
    sample (`Err("proposal is no longer available")` if the sample is gone —
    stale UI, restart, or a purge); build the matcher; if `ignore_rules`
    already contains it, return `Ok(())` (idempotent); else push and call
    `set_config`.
  - `dismiss_proposal(app, fingerprint)`: `obs.dismiss(&fp, current_count)`.
  - `never_suggest_proposal(app, fingerprint) -> Result<(), String>`: same
    resolve, push onto `never_hide`, `set_config`, **then** `purge_family`, then
    `save_observations`.
  - `clear_observations(app)`: `obs.clear()`, then `save_observations` — do not
    wait for the sweep tick; a user who clicks "clear" expects the file to
    change now.
  - Register all five in `tauri::generate_handler![…]`.
- **MIRROR**: COMMAND_REUSES_SET_CONFIG, OFF_LOCK_THEN_LOCK.
- **GOTCHA**: **Never hold the observations lock while taking the engine lock.**
  `observe_opening` establishes config → salt → observations with the engine
  lock released; `list_proposals` inverting that would be the one place a
  deadlock could exist. Scope the build in its own block. Second gotcha:
  `set_config` internally re-reads config and takes the engine lock, so it must
  be called with **no** guard held.
- **VALIDATE**: `cargo build` + `cargo clippy --all-targets -- -D warnings`.
  These commands need a live `AppHandle`, and this codebase has no `tauri::test`
  harness (confirmed in the Phase 2/3 report against `token.rs` and
  `tests/hooks_install.rs`) — so their logic lives in Task 8's pure `build`,
  which is fully covered, and the wiring is verified by the manual checklist
  below. Do not invent a harness for this phase.

### Task 10: TS mirror

- **ACTION**: Type the command surface for Phase 5.
- **IMPLEMENT**: `src/state/proposals.ts` — a `Proposal` interface mirroring the
  Rust struct field-for-field, importing `SessionView` from `./types`. No new
  `SessionView` declaration. No React, no `invoke` wrapper (Phase 5 owns the
  call sites).
- **MIRROR**: `src/state/config.ts`'s comment style — `///` doc comments
  explaining *why* a field exists, and the explicit "Mirrors the Rust …
  (src-tauri/src/…)" header line.
- **GOTCHA**: TypeScript strict, no `any`. `first_seen`/`last_seen` are Unix
  **seconds**, not milliseconds — say so, or Phase 5 renders 1970.
- **VALIDATE**: `npm run typecheck`, `npm run lint`, and
  `npx prettier --check src/state/proposals.ts src/state/config.ts` (scoped —
  the repo-wide `format:check` has 35 pre-existing CRLF failures; do **not**
  sweep them).

### Task 11: Synthetic corpus replay

- **ACTION**: One integration-flavoured test over the whole pure pipeline.
- **IMPLEMENT**: In `proposals.rs` tests, build an in-test corpus: two machine
  families (a long stable opening each, ≥3 sessions apiece), plus human
  openings — one repeated verbatim 3× (the PRD's "known blind spot"), one
  wrapper-marked, one one-off. Feed them through `Observations::observe`, then
  `build`. Assert: the two machine families both appear; the wrapper-marked
  opening is absent **only because it was never observed** (assert the guard at
  the `observe_opening` level is out of scope here — instead just don't feed
  it, and comment why); and the repeated human opening **does** appear —
  documenting the over-suggestion the corpus cannot rule out. Then add it to
  `never_hide`, rebuild, and assert it is gone.
- **MIRROR**: TEST_STRUCTURE — the doc comment states what failure the test
  pins.
- **GOTCHA**: Do not name this test after the PRD's "exactly 2 proposals, 0
  false positives" corpus result. That claim is about 577 real sessions and
  belongs to Phase 6. Name it `synthetic_corpus_yields_one_proposal_per_family`
  and say in the doc comment that the real replay is Phase 6's.
- **VALIDATE**: `cargo test --manifest-path src-tauri/Cargo.toml`.

### Task 12: Docs

- **ACTION**: Document the proposal flow and close review finding **L4**.
- **IMPLEMENT**:
  - `docs/IGNORE-RULES.md`: a "Suggested filters" section — where proposals come
    from, the three actions, that nothing auto-applies, the threshold floor of
    3, and **the two honest caveats**: (a) a cluster that crossed the threshold
    entirely during a previous run surfaces only after one more matching session
    re-supplies the sample text (a delay, not a loss — the alternative was
    persisting plaintext); (b) the preview list can be shorter than the count,
    because the count groups on normalized whitespace while the rule is a
    literal prefix.
  - **L4**: change "for the rest of its life" to "until it restarts" in
    `docs/IGNORE-RULES.md` **and** `CHANGELOG.md` — a genuine `SessionStart`
    clears `revealed` (`engine.rs:319-325`).
  - `CHANGELOG.md` `[Unreleased] → Added`: one entry for suggested filters +
    the five commands + `propose_threshold`. One `Fixed` entry for H1, written
    from the user's point of view ("a session that hit a permission prompt
    before its opening was classified could be hidden while genuinely waiting
    on you").
- **MIRROR**: the existing Unreleased entries' voice — user-facing effect first,
  mechanism second, no internal type names.
- **GOTCHA**: `package.json` is the single source of truth for the version.
  Do not touch `tauri.conf.json` or `Cargo.toml` versions; do not add a release
  heading.
- **VALIDATE**: Read both files end to end; confirm no "rest of its life"
  remains (`grep -rn "rest of its life" docs/ CHANGELOG.md` → empty).

---

## Testing Strategy

### Unit Tests

| # | Test | Input | Expected | Edge? |
|---|---|---|---|---|
| 1 | `session_blocked_before_classification_is_not_hidden` | NeedsYou, then a matching first prompt resolves | visible, `rollup()==Red`, `reveal_count()==1` | **H1** |
| 2 | `from_json_keeps_good_records_beside_bad` | `{"a":{valid},"b":{"bad":1}}` | 1 record survives | **M1** |
| 3 | `observation_missing_field_defaults` | record without `len` | loads, `len == 0` | M1 |
| 4 | `from_json_tolerates_non_object` | `[]`, `null` | empty store, no panic | M1 |
| 5 | `whitespace_tail_collision_counts_once` | 60 visible chars + 15 whitespace + more | colliding fp `n == 1` | **M2 prereq** |
| 6 | `short_prompt_is_sampled_at_its_own_length` | 40-char prompt | one record, `len == 40` | **M2** |
| 7 | `empty_and_whitespace_prompts_are_not_sampled` | `""`, `"  \n\t"` | `sample` → `None`, no record | M2 |
| 8 | `multibyte_short_prompt_records_char_count` | 5-grapheme CJK | `len == 5`, no panic | M2 |
| 9 | `sample_is_a_literal_prefix` (updated) | any prompt, all lengths | `starts_with` holds, `count() <= len` | M2 |
| 10 | `propose_threshold_below_floor_is_clamped` | 0, 1, 5 | 3, 3, 5 | floor |
| 11 | `existing_config_json_loads_with_new_defaults` (extended) | pre-Phase-4 JSON | `propose_threshold == 3` | back-compat |
| 12 | `preview_lists_only_sessions_that_would_vanish` | 3 sessions, 1 matches | preview len 1 | preview |
| 13 | `preview_excludes_never_hide_and_revealed` | allowlisted + revealed matchers | both absent | fail-open |
| 14 | `preview_does_not_mutate_engine_rules` | any preview call | `hidden_count()` unchanged | immutability |
| 15 | `purge_family_drops_the_whole_prefix_chain` | long prompt, purge via 100-len fp | family gone, unrelated survives | purge |
| 16 | `dismissed_survives_only_the_run` | dismiss, round-trip JSON | `dismissed` empty | run-only |
| 17 | `below_threshold_is_not_proposed` | `n == 2`, threshold 3 | no proposal | floor |
| 18 | `cluster_without_live_sample_is_not_proposed` | record loaded from JSON, no sample | no proposal | **invariant** |
| 19 | `shortest_prefix_wins_across_lengths` | one prompt × 3 sessions | exactly 1 proposal, shortest sample | dedup |
| 20 | `two_families_yield_two_proposals` | two distinct openings × 3 | 2 proposals | dedup |
| 21 | `already_covered_by_ignore_rule_is_skipped` | cluster matching an existing rule | no proposal | **L1** |
| 22 | `never_hide_prefix_suppresses_the_proposal` | allowlisted opening in the store | no proposal | pre-existing records |
| 23 | `dismissed_returns_when_the_cluster_grows` | dismiss at 3, observe a 4th | reappears | PRD |
| 24 | `ordering_is_highest_count_first` | clusters n=3,7,5 | 7,5,3 | ordering |
| 25 | `threshold_below_floor_is_clamped_in_build` | `build(.., 1, ..)` | behaves as 3 | defence in depth |
| 26 | `synthetic_corpus_yields_one_proposal_per_family` | in-test corpus | 2 machine + 1 human proposal; human gone after `never_hide` | replay |

### Edge Cases Checklist
- [ ] Empty / whitespace-only opening → never sampled, never proposed (Task 4)
- [ ] Prompt shorter than every `PREFIX_LENS` entry → one record at its own length
- [ ] Multibyte prompt truncated on a char boundary, never a byte boundary
- [ ] A cluster whose sample was lost to a restart → counted, never proposed
- [ ] `accept_proposal` on a fingerprint whose sample vanished → `Err`, not a panic
- [ ] `accept_proposal` twice → idempotent, one rule
- [ ] `never_suggest_proposal` → rule added **and** family purged **and** saved
- [ ] `clear_observations` → store key emptied immediately, not on the next tick
- [ ] Preview when nothing live matches → empty vec (Phase 5 must say so plainly, not hide the line)
- [ ] Threshold hand-edited to 1 in `beacon.json` → clamped on load *and* in `build`
- [ ] Config JSON predating `propose_threshold` → loads, defaults to 3
- [ ] Lock order: observations released before the engine lock is taken

---

## Validation Commands

### Static Analysis
```bash
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
npm run typecheck
npm run lint
```
EXPECT: all clean, zero warnings.

### Tests
```bash
cargo test --manifest-path src-tauri/Cargo.toml
```
EXPECT: all green. Baseline is **109 lib + 1 integration**; this plan adds ~26
and rewrites 3, so expect ~132 lib tests.

### Build
```bash
cargo build --manifest-path src-tauri/Cargo.toml
npm run build
```
EXPECT: clean.

### Format (scoped — do NOT sweep)
```bash
npx prettier --check src/state/config.ts src/state/proposals.ts
```
EXPECT: pass. `npm run format:check` fails on ~35 pre-existing files (Windows
CRLF checkout artifact, verified unrelated in the Phase 2/3 review, finding L5).
Fixing them repo-wide is a forbidden sweep under the project's parallel-session
convention.

### Manual Validation (requires the live desktop app)
- [ ] `npm run tauri dev`; run three `claude -p "<same long opening>"` sessions
- [ ] `invoke("list_proposals")` from the settings devtools console returns one
      proposal with `count: 3` and readable `sample` text
- [ ] `matching` lists the live rows that are actually on screen
- [ ] `accept_proposal` → those rows disappear from the widget, the tray
      recolours, and `beacon.json`'s `config.ignore_rules` gains the entry
- [ ] `never_suggest_proposal` on a second cluster → `config.never_hide` gains
      the entry and the cluster's fingerprints are gone from `observations`
- [ ] `clear_observations` → the `observations` key is empty in `beacon.json`
      immediately
- [ ] Restart the app: the accepted rule still hides its sessions; no proposal
      re-surfaces for it
- [ ] `grep` `beacon.json` for a phrase you typed → no match (privacy holds)

---

## Acceptance Criteria

- [ ] H1 fixed with a regression test that fails on the pre-fix tree
- [ ] M1 fixed; one bad record no longer discards the history
- [ ] M2 fixed in the review's order — dedup **before** clamp
- [ ] `Observation.len` holds the actual sample length, not the nominal one
- [ ] `propose_threshold` exists, defaults to 3, is clamped at 3 in
      `sanitized()` **and** in `build`, and is mirrored in TS
- [ ] Five commands registered and callable
- [ ] Proposals surface only with a live sample (the PRD's in-memory invariant)
- [ ] Shortest-prefix-wins collapses one prompt's five lengths to one proposal
- [ ] A cluster already covered by an `ignore_rules` entry is not re-proposed
- [ ] `matching` is derived by re-running `session_hidden`, never a hand-rolled
      prefix test
- [ ] No plaintext reaches disk — `store_json_contains_no_prompt_text` still
      passes unchanged
- [ ] All validation commands pass; no repo-wide format sweep

## Completion Checklist

- [ ] Code follows the discovered patterns (split-borrow, lenient parse,
      config clamp, `set_config` reuse, off-lock-then-lock, immutable rule
      derivation)
- [ ] Every new public item carries a doc comment saying *why*, in the
      surrounding density
- [ ] No `any` in TypeScript; no `unwrap` on user-supplied data in Rust
- [ ] No hardcoded thresholds outside `config.rs` constants
- [ ] `docs/IGNORE-RULES.md` + `CHANGELOG.md` updated; L4's "until it restarts"
      applied in both
- [ ] PRD Phase 4 row moved to `complete` with a report path (at report time,
      not now)
- [ ] Write `.claude/PRPs/reports/clustering-and-proposals-report.md`
- [ ] No scope additions — nothing from the NOT Building list crept in

## Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| **A very short sample becomes a very broad rule.** M2's clamp makes a repeated 4-char opening proposal-eligible; accepting it writes a 4-char `first_prompt_prefix` that could hide unrelated sessions | M | **High** | Recommended course: propose anyway. PRD decision 6 forbids shipping an unmeasured length threshold ("a number in a UI reads as authoritative"), and the safety model is preview + never-auto-apply, both of which hold here. Phase 6's prefix-discrimination sweep derives the number; the review already flags that the sweep should report separately on sub-60 samples. **Flagged to the author as a consequence of the M2 resolution that the review's "Accepted consequence" paragraph understated** |
| Preview drifts from actual behaviour, so accepting hides rows the card never showed | L | High | `preview_hidden_by` re-runs the real `session_hidden` against a candidate `IgnoreRules`; agreement is structural, not maintained by hand. Test 12–14 |
| Deadlock between the observations and engine mutexes | L | High | One established order, one scoped block, one GOTCHA on Task 9. No nesting anywhere |
| Count and preview disagree (count 7, preview 2) and reads as a bug | M | Low | Documented in Task 12's caveat (b): the count groups on normalized whitespace, the rule is a literal prefix, and only live sessions can appear. The direction is under-hide, which is the safe one |
| Phase 5 renders the whole list and invites bulk-accept | M | Medium | `list_proposals`' doc comment states the one-at-a-time contract at the point a Phase 5 implementer will read it |
| `propose_threshold` missing from the TS mirror silently resets the user's value | M | Medium | Made a same-task requirement with an explicit GOTCHA, not a follow-up |
| Fixing H1/M1/M2 inside this phase makes the diff large and hard to review | H | Low | Tasks 1–4 are independent, each with its own test — commit them separately, before Task 5, so the Phase 4 feature diff stands alone |

---

## Deltas from the PRD

| # | PRD says | This plan does | Why |
|---|---|---|---|
| 1 | Phase 4 scope lists clustering + five commands | Also carries H1, M1, M2 (Tasks 1–4) | M2 changes `Observation.len`, which shortest-prefix dedup reads. Building on the unfixed shape means rewriting Phase 4's core later. H1 blocks merge regardless |
| 2 | "Corpus replay yields exactly 2 proposals, 0 false positives" | Synthetic in-test corpus only | The 577-session fixtures are Phase 6's deliverable. Asserting the number here would mean inventing the corpus |
| 3 | `never_hide` "filters at ingest, **not** at proposal time" | Also filters at proposal time | Ingest filtering still owns the privacy win (allowlisted openings never touch disk). The proposal-time check covers records stored *before* a hand-written entry existed — which the PRD's own "Known limitation" says ingest cannot reach. Free: sample and rule are both plaintext in memory |
| 4 | Dismiss "hidden this round" | Run-only `HashMap<fp, n_at_dismissal>` | Matches the sample table's lifetime — a dismissal cannot outlive the sample that justified it |
| 5 | Proposal card shows one at a time | `list_proposals` returns all, sorted | The tray line needs a count, and replay tests need the set. The one-at-a-time rule is a UI contract, documented at the command |
| 6 | Threshold floor "enforced in code" | Clamped in `sanitized()` **and** re-clamped in `build` | `sanitized()` covers the file; the second clamp covers any future caller that skips it |
| 7 | "Dismiss permanently … + that fingerprint set purged" | `purge_family` relates fingerprints via **live samples only** | Hashes are one-way. Stated in the doc comment so nobody reads the purge as total; `clear_observations` is the escape hatch |
| 8 | Phase 4 lists `never_suggest_proposal (→ never_hide + purge)` | Purge runs **after** `set_config` succeeds | A failed config save must not leave the cluster purged and the rule unwritten |

## Notes

- **Commit Tasks 1–4 separately from 5–12.** They are review fixes against
  Phases 2–3, and the Phase 2/3 work is still uncommitted in the working tree
  (12 modified files, `markers.rs` and `observe.rs` untracked). Folding the
  fixes into the same commit as the phase they fix is fine; folding Phase 4's
  feature work in with them is not.
- **The working tree is uncommitted.** `git status` shows Phases 2–3 unstaged.
  Confirm with the author whether to commit those first — this plan assumes the
  tree as it stands is the baseline either way.
- `sha2` is already a direct dependency (added in Phase 2); no `Cargo.toml`
  change in this phase.
- No `AppHandle` test harness exists in this codebase (verified against
  `token.rs` and `tests/hooks_install.rs`). This is why Task 8 carries the
  logic and Task 9 carries only wiring — the same split the Phase 2/3 report
  documented as a deviation. Do not introduce `tauri::test` here.
