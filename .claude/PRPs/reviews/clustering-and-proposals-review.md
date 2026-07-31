# Code Review: Clustering + proposals (PRD Phase 4)

**Reviewed**: 2026-07-30
**Branch**: `feat/headless-session-filter` (uncommitted working tree, 10 files; Phases 2–3 now committed as `c17e57e`)
**Source**: [.claude/PRPs/reports/clustering-and-proposals-report.md](.claude/PRPs/reports/clustering-and-proposals-report.md)
**Decision**: **REQUEST CHANGES** — one HIGH finding, reproduced

## Summary

The three carried review findings (H1, M1, M2) are all genuinely fixed, and the
Phase 4 feature is faithful to the plan — including the two things most likely
to have been cut: the preview really does re-run `session_hidden` against a
candidate rule set rather than hand-rolling a prefix test, and the pure/wiring
split really does keep the whole eligibility pipeline testable without an
`AppHandle`. Validation is green and the report's numbers check out exactly:
133 lib tests + 1 integration, fmt/clippy/typecheck/lint/scoped-prettier clean.

One HIGH defect: **"Dismiss" doesn't dismiss.** For any opening longer than 60
characters — which is every machine family the PRD is built around — dismissing
a proposal immediately resurfaces the same opening at the next prefix length.
I reproduced it. Everything else is MEDIUM or below.

## Findings

### CRITICAL

None. No secrets, no network egress, no injection surface, no unsafe code, no
new dependency. The privacy posture holds: `to_json` still excludes `samples`,
`store_json_contains_no_prompt_text` still passes untouched, and the one new
log line (`from_json`'s drop notice) prints the fingerprint key only, never the
value.

### HIGH

#### H1 — "Dismiss" is defeated by the next prefix length

**Files**: [src-tauri/src/proposals.rs:71](src-tauri/src/proposals.rs#L71) (dismissal filter),
[src-tauri/src/proposals.rs:93-104](src-tauri/src/proposals.rs#L93-L104) (dedup)

The dismissal filter runs **inside the candidate chain**, before
shortest-prefix-wins de-duplication. One opening ≥120 chars produces five
records (one per `PREFIX_LENS` entry). Dedup normally collapses them to the
60-char one. But dismissing removes that 60-char candidate *before* dedup runs
— so the 70-char record is no longer shadowed, becomes the new shortest
survivor, and is returned as a fresh proposal carrying near-identical text.

The user clicks "Not now" and the same card comes straight back. Clicking it
five times exhausts the lengths; a sixth click finally sticks. The docs
committed in this same change say the proposal "reappears once the cluster
grows past its count at dismissal" — which is not what happens.

Reproduced with a temporary probe (since removed), one opening observed 3×:

```
PROBE before dismiss: 1 proposal(s)
PROBE kept sample len = 60
PROBE after dismiss: 1 proposal(s)
PROBE   resurfaced sample len = 70
```

`accept_proposal` and `never_suggest_proposal` are **not** affected — both write
a rule from the 60-char sample, and steps 3/4 filter every longer sample because
the short one is a literal prefix of it. Only dismissal, which is keyed on a
single fingerprint, has this hole.

**Why the test suite missed it**: `dismissed_returns_when_the_cluster_grows`
([proposals.rs:226-229](src-tauri/src/proposals.rs#L226-L229)) uses a
deliberately sub-60-char prompt, and its comment explains why — *"a longer
prompt would fingerprint at several lengths, and dismissing just one wouldn't
clear the others."* The multi-record case was identified and then designed
around rather than covered. That comment is the bug report.

**Fix** — move the dismissal filter *after* de-duplication, so it acts on the
proposal the user was actually shown:

```rust
// …candidate chain WITHOUT the dismissed filter…
// (dedup loop unchanged)
kept.retain(|p| obs.dismissed_at(&p.fingerprint).is_none_or(|n| p.count > n));
kept.sort_by(/* unchanged */);
```

This is correct in both directions: the 70-char record is consumed by dedup
before dismissal is consulted, so it can never be promoted; and when the cluster
grows, `count` rises on the kept fingerprint and it reappears as documented.
Dismissing a whole family via `Observations` would also work, but this is one
line and needs no new API.

**Regression test**: rewrite `dismissed_returns_when_the_cluster_grows` to use a
prompt **longer than 120 chars** (so all five lengths fingerprint distinctly),
assert `build` is empty after dismissing the single returned proposal, then
observe a 4th session and assert exactly one proposal returns. Delete the
"kept under 60 chars" comment — it documents a constraint that should no longer
exist.

### MEDIUM

#### M1 — A very short sample can now become a very broad rule

**Files**: [src-tauri/src/observe.rs:91-101](src-tauri/src/observe.rs#L91-L101) (clamp),
[src-tauri/src/proposals.rs:58-81](src-tauri/src/proposals.rs#L58-L81) (no length floor)

This is the accepted consequence of the M2 resolution, flagged as the top Risk
in the plan and re-stated here only because it is now reachable code rather than
a design note. With the length floor gone, three sessions opening with
`"continue"` cluster at `n = 3` and propose a `first_prompt_prefix` rule with
`value: "continue"` — which hides every future session whose opening starts with
that word.

The committed mitigations do hold: nothing auto-applies, and the card shows the
sample text plus a live preview. But the preview only covers sessions live *at
that moment*; the rule's real blast radius is future sessions, which nothing
displays.

**No fix requested here** — PRD decision 6 explicitly forbids shipping an
unmeasured length threshold, and this was the author's call. Two follow-throughs
that should be treated as load-bearing rather than nice-to-have:

1. Phase 5's card must render the sample **with its length visible** (`len` is
   already on the wire and mirrored in TS), so a 8-char pattern reads as
   alarming rather than terse.
2. Phase 6's prefix-discrimination sweep must report separately on samples below
   60 chars, and that number should gate proposal eligibility, not just the
   `never_hide` warning it was originally scoped for.

### LOW

- **L1 — `Observation.len`'s doc comment is now wrong.**
  [observe.rs:140-142](src-tauri/src/observe.rs#L140-L142) still says *"Which
  prefix length produced this fingerprint. A later de-duplication pass needs
  it."* Neither clause survives Task 4: it is now the sample's **actual** char
  count, and the dedup pass uses `sample.chars().count()`, not this field. Worth
  correcting while the change is fresh — it is the one place a reader would go
  to understand the field.
- **L2 — `purge_family` and the dedup use different "same family" relations.**
  [observe.rs:315-326](src-tauri/src/observe.rs#L315-L326) relates samples via
  `normalize` (lowercased, whitespace-collapsed); `build`'s dedup relates them
  via `matches_prompt` (raw literal, case-insensitive). They disagree when
  interior whitespace differs, so the purge is the broader of the two. Broader
  is the safe direction for an explicit "never suggest", but the divergence
  deserves a one-line comment so a future reader doesn't assume they match.
- **L3 — dedup allocates a throwaway `IgnoreRules` per (kept, candidate) pair.**
  [proposals.rs:95-100](src-tauri/src/proposals.rs#L95-L100). Deliberate, and
  the right trade — it buys semantics parity with the rule the proposal writes —
  but it is O(n²) allocations. Fine at realistic cluster counts; noted only so
  it is a known cost rather than an oversight.
- **L5 — `dismiss_proposal` alone returns `()` rather than `Result`.**
  <!-- Downgraded from MEDIUM 2026-07-30. Originally filed as a silent failure;
       that framing was wrong — see the paragraph below. -->
  [lib.rs:606-620](src-tauri/src/lib.rs#L606-L620) no-ops when the fingerprint
  has no live sample, while `accept_proposal` and `never_suggest_proposal`
  return `Err("proposal is no longer available")` in the same situation.

  **This is not a silent failure.** The lookup goes through
  `iter_with_samples()`, and `proposals::build` enumerates through that *same*
  iterator ([proposals.rs:67](src-tauri/src/proposals.rs#L67)) — so a
  fingerprint the lookup cannot find is one `build` cannot return either. The
  only ways to get there are `clear_observations`, `never_suggest_proposal`
  (which purges the family *and* installs a `never_hide` rule that filters it
  independently at step 4), and an app restart (samples are run-only, and the
  webview restarts with the backend). `prune` is not a fourth path: it drops
  records past the `last` cutoff, and anything with a live sample was observed
  this run. In every case the proposal is already gone by a stronger mechanism,
  so the no-op produces exactly the outcome a successful dismissal would have.

  What remains is API consistency: three sibling commands, two returning
  `Result`, one not, forcing Phase 5 to special-case one call site for no
  behavioural reason.
- **L4 — `prune` doesn't drop `dismissed` entries for pruned fingerprints.**
  `clear` and `purge_family` both do. The map is bounded by user clicks within
  one run, so this is housekeeping, not a leak.

## What's notably right

- **The preview cannot lie.** `preview_hidden_by`
  ([engine.rs:983-1002](src-tauri/src/engine.rs#L983-L1002)) computes
  `!hidden(current) && hidden(candidate)` through the real `session_hidden`, so
  `never_hide` precedence and the sticky `revealed` flag are honoured for free —
  and `preview_excludes_never_hide_and_revealed` pins both. This is the single
  most load-bearing thing in the phase and it was built the careful way.
- **The H1 (Phase 2/3) regression test is a real reproduction**, not a
  restatement: it establishes the premise (`!is_hidden` before classification),
  then asserts all four post-conditions the review specified, including
  `rollup() == Red` — the symptom a user would actually notice.
- **`iter_with_samples` makes the in-memory invariant structural.** It is the
  only way to enumerate, so a record with no live sample cannot be proposed by
  construction rather than by a check someone might forget.
  `cluster_without_live_sample_is_not_proposed` round-trips through `to_json`
  to prove it.
- **The synthetic corpus test refuses to overclaim.** It asserts the repeated
  *human* opening **does** get proposed, with a message saying it is
  "documenting the over-suggestion this corpus cannot rule out, not asserting
  it's fine". A test that records an uncomfortable truth is worth more than one
  that hides it.
- **Both deviations in the report are real and correctly diagnosed** — the
  `seen`-set session-id collision and the too-long `never_hide` rule value are
  both consequences of real semantics, and the fixes address the cause.

## Validation Results

| Check | Result | Notes |
| --- | --- | --- |
| `cargo fmt --check` | Pass | |
| `cargo clippy --all-targets -- -D warnings` | Pass | |
| `cargo test` | Pass | 133 lib + 1 integration (baseline 109) — matches the report exactly |
| `npm run typecheck` | Pass | |
| `npm run lint` | Pass | |
| `npx prettier --check src/state/{config,proposals}.ts` | Pass | Scoped, per the no-sweep convention |

## Files Reviewed

| File | Change |
| --- | --- |
| `src-tauri/src/proposals.rs` | Added (+380) |
| `src/state/proposals.ts` | Added (+27) |
| `src-tauri/src/observe.rs` | Modified (+286/-…) |
| `src-tauri/src/engine.rs` | Modified (+201) |
| `src-tauri/src/lib.rs` | Modified (+110) |
| `src-tauri/src/config.rs` | Modified (+35) |
| `src-tauri/src/ignore.rs` | Modified (+20) |
| `src/state/config.ts` | Modified (+5) |
| `docs/IGNORE-RULES.md` / `CHANGELOG.md` | Modified (+61) |

## Recommended Order

1. Fix **H1** — move the dismissal filter after dedup; rewrite the test with a
   >120-char prompt (blocks merge)
2. **L1** doc correction, **L2** clarifying comment, **L5** signature
   (all one-liners, cheap now)
3. Carry **M1**'s two follow-throughs into the Phase 5 and Phase 6 plans
