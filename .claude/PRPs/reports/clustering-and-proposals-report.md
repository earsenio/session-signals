# Implementation Report: Clustering + proposals (PRD Phase 4)

## Summary

Turned the observation store into an offer. Grouped the salted fingerprints
already being recorded, de-duplicated across the five prefix lengths
(shortest prefix wins), clamped the propose threshold at 3 in code, and
exposed five commands — `list_proposals`, `accept_proposal`,
`dismiss_proposal`, `never_suggest_proposal`, `clear_observations` — that let
a user turn a pattern the app actually saw into an `ignore_rules` entry, or
refuse it permanently. Also carried and fixed the three outstanding review
findings from Phases 2–3 (H1, M1, M2), per the plan's Tasks 1–4.

## Assessment vs Reality

| Metric | Predicted (Plan) | Actual |
|---|---|---|
| Complexity | Large | Large — matched |
| Files Changed | 9 (1 created, 8 updated) | 10 (2 created, 8 updated) — plan under-counted `src/state/proposals.ts` as a create alongside `proposals.rs` |
| Tasks | 12 | 12, all complete |

## Tasks Completed

| # | Task | Status | Notes |
|---|---|---|---|
| 1 | H1 — reveal a session blocked before classification | Done | `engine.rs::set_first_prompt` now guards the reveal-on-block case symmetrically with `transition_to` |
| 2 | M1 — per-entry lenient load for observations | Done | `Observations::from_json` now drops one bad record instead of the whole store |
| 3 | M2 step 1 — intra-call fingerprint de-duplication | Done | `observe()` now dedupes fingerprints within one call before counting |
| 4 | M2 steps 2–4 — clamp `sample`, record real length | Done | `sample()` has no length floor (only an empty-prompt guard); `Observation.len` is the actual sample char count |
| 5 | `propose_threshold` config field, floor 3 | Done | Clamped in `sanitized()` and re-clamped in `proposals::build`; TS mirror added in the same task |
| 6 | `IgnoreRules::with` + `Engine::preview_hidden_by` | Done | Preview re-runs real `session_hidden` against a candidate rule set — never a hand-rolled prefix test |
| 7 | `Observations` query surface + dismissals + purge | Done | `iter_with_samples`, `sample_for`, `dismiss`/`dismissed_at`, `purge_family` |
| 8 | `proposals.rs` — eligibility + shortest-prefix dedup | Done | Pure module, no `AppHandle`/`Engine` dependency |
| 9 | Five commands | Done | Registered in `generate_handler!`; observations lock released before the engine lock is taken (`list_proposals`) |
| 10 | TS mirror | Done | `src/state/proposals.ts`, reusing the existing `SessionView` |
| 11 | Synthetic corpus replay | Done | Two machine families + a repeated-human blind-spot case, documented and removable via `never_hide` |
| 12 | Docs | Done | New "Suggested filters" section in `docs/IGNORE-RULES.md`; L4's "rest of its life" → "until it restarts" in both `docs/IGNORE-RULES.md` and `CHANGELOG.md`; `CHANGELOG.md` Unreleased entries added |

## Validation Results

| Level | Status | Notes |
|---|---|---|
| Static Analysis | Pass | `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `npm run typecheck`, `npm run lint` all clean |
| Unit Tests | Pass | 133 Rust lib tests (baseline 109 + 1 integration; plan predicted ~132) |
| Build | Pass | `cargo build` and `npm run build` both clean |
| Integration | Pass | `cargo test` — `hooks_install.rs` (1 test) green |
| Edge Cases | Pass | See checklist below |
| Scoped format | Pass | `npx prettier --check src/state/config.ts src/state/proposals.ts` |

## Files Changed

| File | Action | Notes |
|---|---|---|
| `src-tauri/src/proposals.rs` | CREATED | Pure clustering/eligibility/dedup core, 10 in-module tests |
| `src/state/proposals.ts` | CREATED | TS mirror of `Proposal` |
| `src-tauri/src/observe.rs` | UPDATED | M1/M2 fixes, query surface, `dismissed` table, `purge_family` |
| `src-tauri/src/engine.rs` | UPDATED | H1 fix, `view_of` extraction, `preview_hidden_by` |
| `src-tauri/src/ignore.rs` | UPDATED | `IgnoreRules::with` |
| `src-tauri/src/config.rs` | UPDATED | `propose_threshold` + floor-3 clamp |
| `src-tauri/src/lib.rs` | UPDATED | Five commands, handler registration, `mod proposals` |
| `src/state/config.ts` | UPDATED | Mirrors `propose_threshold` |
| `docs/IGNORE-RULES.md` | UPDATED | "Suggested filters" section, L4 phrasing fix |
| `CHANGELOG.md` | UPDATED | Unreleased `Added`/`Fixed` entries, L4 phrasing fix |

## Deviations from Plan

- **Test-helper session-id collisions.** The plan's own test names implied
  multiple `observe_n_times`-style calls sharing one `Observations` instance.
  Because `Observations::observe`'s intra-run dedup set (`seen`) is keyed by
  session id across the *whole store*, not per-prompt, an early draft of
  `two_families_yield_two_proposals` / `ordering_is_highest_count_first` /
  `dismissed_returns_when_the_cluster_grows` reused overlapping session ids
  across calls and silently under-counted later clusters. Fixed by tagging
  each call with a unique session-id prefix. Caught immediately by the
  validation loop (tests failed, root cause diagnosed, fixed) — no production
  code was wrong.
- **`never_hide` proposal-time test used a too-long rule value.** The
  synthetic-corpus test initially tried to allowlist the *full* repeated
  human prompt, but `never_hide.matches_prompt(sample)` compares against the
  proposal's (possibly truncated) `sample`, and a rule longer than the
  sample it's tested against can never match via a prefix test. Fixed by
  using the proposal's own `sample` as the rule value — which is also what
  `never_suggest_proposal` actually writes in production.
- Otherwise implemented exactly as planned, including the Task 1–4 review
  fixes carried alongside the Phase 4 feature work.

## Issues Encountered

None beyond the test-helper bugs above, all resolved before moving to the
next task per the "fix before moving on" validation-loop rule.

## Tests Written

| Test File | Tests | Coverage |
|---|---|---|
| `src-tauri/src/engine.rs` | 4 new (`session_blocked_before_classification_is_not_hidden`, `preview_lists_only_sessions_that_would_vanish`, `preview_excludes_never_hide_and_revealed`, `preview_does_not_mutate_engine_rules`) | H1 fix + `preview_hidden_by` |
| `src-tauri/src/observe.rs` | 11 new/rewritten (M1 lenient-load trio, M2 sample/length trio + collision test, purge_family, dismissed-run-only, short-prompt-single-record, multibyte-char-count) | M1/M2 fixes + query surface |
| `src-tauri/src/ignore.rs` | 1 new (`with_appends_without_mutating_self`) | `IgnoreRules::with` |
| `src-tauri/src/config.rs` | 1 new (`propose_threshold_below_floor_is_clamped`) + 1 extended | `propose_threshold` clamp |
| `src-tauri/src/proposals.rs` | 10 new (eligibility, ordering, dedup, dismissal-lapse, never_hide/ignore suppression, synthetic corpus replay) | The pure clustering core |

Total: 133 Rust lib tests (up from a 109-test baseline).

## Edge Cases Checklist

- [x] Empty / whitespace-only opening → never sampled, never proposed
- [x] Prompt shorter than every `PREFIX_LENS` entry → one record at its own length
- [x] Multibyte prompt truncated on a char boundary, never a byte boundary
- [x] A cluster whose sample was lost to a restart → counted, never proposed
- [x] `accept_proposal` on a fingerprint whose sample vanished → `Err`, not a panic
- [x] `accept_proposal` twice → idempotent, one rule
- [x] `never_suggest_proposal` → rule added and family purged and saved (purge runs after `set_config` succeeds)
- [x] `clear_observations` → store key emptied immediately, not on the next tick
- [x] Preview when nothing live matches → empty vec
- [x] Threshold hand-edited to 1 in `beacon.json` → clamped on load and in `build`
- [x] Config JSON predating `propose_threshold` → loads, defaults to 3
- [x] Lock order: observations released before the engine lock is taken (`list_proposals`)

## Next Steps
- [ ] Code review via `/code-review`
- [ ] Create PR via `/prp-pr`
- [ ] Manual validation checklist from the plan (requires the live desktop app — not run this session)
