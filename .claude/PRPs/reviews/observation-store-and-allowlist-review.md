# Code Review: Observation store + marker registry & `never_hide` allowlist

**Reviewed**: 2026-07-30
**Branch**: `feat/headless-session-filter` (uncommitted working tree, 12 files)
**Source**: [.claude/PRPs/reports/observation-store-and-allowlist-report.md](.claude/PRPs/reports/observation-store-and-allowlist-report.md)
**Decision**: **REQUEST CHANGES** — one HIGH finding, reproduced

## Summary

The implementation is faithful to the plan and unusually well-documented — every non-obvious
choice carries the reasoning that produced it, and the report's self-assessment (including its two
documented deviations and its scoped format-check claim) checks out against the tree. Validation is
green: 109 lib tests + 1 integration test, clippy `-D warnings` clean, fmt/typecheck/lint clean.

One HIGH defect: the reveal-on-block safety valve has a hole on the path where a session becomes
hidden *after* it is already blocked. I reproduced it — a session sits at `NeedsYou` while
`snapshot()` excludes it and the tray reads Grey, with `reveal_count` at 0. Everything else is
MEDIUM or below.

## Findings

### CRITICAL

None. No secrets, no network egress, no injection surface, no unsafe code. The privacy posture
holds: `store_json_contains_no_prompt_text` is a real test, `samples` is genuinely excluded from
`to_json`, the salt lives under its own store key, and no log line carries prompt text.

### HIGH

#### H1 — Reveal-on-block misses the "blocked before classified" ordering

**Files**: [src-tauri/src/engine.rs:604-624](src-tauri/src/engine.rs#L604-L624) (guard),
[src-tauri/src/engine.rs:806-825](src-tauri/src/engine.rs#L806-L825) (`set_first_prompt`)

`revealed` is set in exactly one place — `transition_to`, when `state == State::NeedsYou`. But a
session's hidden-ness also flips inside `set_first_prompt`, and that path never consults the
session's state. A session that is *already* `NeedsYou` when its first prompt resolves therefore
becomes hidden with the valve untouched.

This ordering is not exotic — it is the **normal** one for a first-prompt rule. The first head-read
happens at `SessionStart`, before any prompt exists, so classification is always deferred to a retry
≥5s later ([lib.rs:229](src-tauri/src/lib.rs#L229)). Any permission prompt inside that window lands
before classification.

Reproduced with a temporary probe (since removed):

```
PROBE: state=NeedsYou hidden=true reveal_count=0 snapshot_len=0 rollup=Grey
```

A session genuinely blocked on the user, invisible in the widget, tray reading Grey, and the
counter that is supposed to falsify the "headless never blocks" premise still at zero. Partial
mitigation: the transition's notification did fire, because `process_event` checks `is_hidden`
before the prompt resolves — so the user is told once, then the row vanishes.

**Fix** — apply the same guard where hidden-ness actually changes:

```rust
// in set_first_prompt, after updating first_prompt/checked_at:
let now_hidden = session_hidden(ignore, never_hide, s);
// A session that is *already* blocked must not be hidden by a late
// classification — same safety valve as `transition_to`, at the other
// point where hidden-ness can flip.
if now_hidden && !was && s.state == State::NeedsYou && !s.revealed {
    s.revealed = true;
    *reveal_count += 1;
    return false; // hidden-ness did not change: it stayed visible
}
```

(`reveal_count` needs adding to the existing destructure.) Suggested test:
`session_blocked_before_classification_is_not_hidden` — assert `is_hidden == false`,
`snapshot().len() == 1`, `rollup() == Red`, `reveal_count() == 1`.

### MEDIUM

#### M1 — `Observations::from_json` is all-or-nothing, against this codebase's own precedent

**File**: [src-tauri/src/observe.rs:222-230](src-tauri/src/observe.rs#L222-L230)

`serde_json::from_value::<HashMap<String, Observation>>(v).unwrap_or_default()` discards the
**entire** history if any single record is malformed — which the test
`from_json_tolerates_garbage` actually demonstrates rather than guards against. `Observation`'s
fields also carry no `#[serde(default)]`, so adding a field in Phase 4 wipes every existing record
on upgrade.

This is the exact hazard [ignore.rs:48-52](src-tauri/src/ignore.rs#L48-L52) exists to prevent
("one stale entry would abort deserialization and silently reset *every* unrelated setting"). The
blast radius here is smaller — lost counts only delay a proposal — but the asymmetry is worth
closing while it's cheap.

**Fix**: parse to `HashMap<String, serde_json::Value>`, then `from_value` per entry, dropping bad
ones with the established `eprintln!("beacon: …")`. Add `#[serde(default)]` to `Observation`.

#### M2 — Openings shorter than 60 characters can never be observed

**File**: [src-tauri/src/observe.rs:91-97](src-tauri/src/observe.rs#L91-L97)

`sample` returns `None` when the trimmed prompt is shorter than `len`, and `PREFIX_LENS` starts at
60. A spawner whose injected opening is a single short line ("Summarize the staged diff.") is
permanently invisible to the Observer — no fingerprint at any length, so no proposal, ever.

Both ECC families have long openings so today's corpus can't see this, which is precisely why it
deserves a written decision. There is a defensible case for the floor (short prefixes discriminate
poorly — the thing Phase 6's prefix-discrimination sweep is meant to measure), but right now it is
an emergent property of `<`, not a recorded choice.

**Resolution (author's call, 2026-07-30)**: hash whatever characters exist — `sample` clamps to
`min(len, actual)` instead of returning `None`. Identical fingerprints across all five lengths for
one short prompt are acceptable; Phase 4's shortest-set-wins de-duplication already collapses them.

**This fix has one prerequisite**, or it turns a coverage gap into a false-positive generator:

> `observe()` has no intra-call fingerprint de-duplication
> ([observe.rs:165-180](src-tauri/src/observe.rs#L165-L180)). It loops over `PREFIX_LENS` and does
> `rec.n += 1` per iteration, so **two lengths yielding the same fingerprint double-count a single
> session**. Under the clamp, every prompt under 60 chars produces five identical fingerprints, so
> one session would land at `n = 5` — straight through the threshold-3 floor on its first sighting,
> which is exactly the "0 double-counted observations within one run" metric the PRD commits to.

The collision is already reachable today, just rarely: `normalize` drops trailing whitespace, so a
prompt whose characters 61-70 are all whitespace (a newline plus indentation — common in injected
machine openings) makes `sample(60)` and `sample(70)` normalize identically. The clamp makes the
systematic case out of a latent one.

**Fix, in order**:

1. Add a per-call `HashSet<String>` of fingerprints already counted in this `observe()` and skip
   repeats — the same belt-and-braces idea as the existing `seen` set, one level down.
2. Then clamp `sample` to `min(len, actual)`.
3. Store the **actual sample length** in `Observation.len`, not the nominal `PREFIX_LENS` entry —
   otherwise a 40-char prompt records `len: 60` and Phase 4's shortest-set-wins compares a fiction.
4. Update `sample_is_a_literal_prefix`, which currently asserts `s.chars().count() == len`.

**Accepted consequence, worth stating**: short openings are weak discriminators, so a repeated
human one-liner ("fix the tests") now becomes proposal-eligible. That is the over-suggestion risk
the corpus is structurally unable to measure (PRD "Known blind spot"), and what absorbs it is
`never_hide` plus the fact that proposals never auto-apply. Phase 6's prefix-discrimination sweep
should report separately on samples below the floor.

#### M3 — New TS defaults point the unsafe way when `cfg` is stale
<!-- Severity revised 2026-07-30 → LOW today, MEDIUM once Phase 5 ships the UI. The original
     "pre-load race" justification below is the weak path; the reachable one is a swallowed
     rejection. -->

**File**: [src/state/config.ts:83-86](src/state/config.ts#L83-L86)

[Settings.tsx:24](src/settings/Settings.tsx#L24) seeds `cfg` from `DEFAULT_CONFIG` and `persist()`
sends the whole object, so a save issued while `cfg` is still `DEFAULT_CONFIG` writes those values
for every field the user didn't touch. The existing
[config.ts:58-62](src/state/config.ts#L58-L62) comment documents exactly this hazard and picks the
safe direction for `ignore_rules` (`[]` can only fail toward *not* resurrecting deleted rules).

The four new fields fail the other way: `observe_enabled: true` re-enables observation a user
disabled, and `never_hide: []` wipes a hand-written allowlist (re-hiding sessions).

**The startup race is not the reachable path.** The settings webview is created once at launch and
only hidden/shown after ([lib.rs:713-717](src-tauri/src/lib.rs#L713-L717)); re-navigation on open
— the only thing that remounts React — is `#[cfg(debug_assertions)]`
([tray.rs:195-200](src-tauri/src/tray.rs#L195-L200)). Every write is user-initiated, `patch` uses
the functional `setCfg((c) => …)` form, and `get_config` never holds a lock across I/O. Clicking
inside that window is not humanly possible.

**The reachable path is the silent catch** at [Settings.tsx:59](src/settings/Settings.tsx#L59):
`invoke("get_config").then(…).catch(() => {})`. If that invoke ever rejects — a dev-mode
`navigate()` racing webview teardown, or any IPC hiccup — `cfg` stays `DEFAULT_CONFIG` for the
**lifetime of the webview**, with no retry and no error surfaced. The panel then renders defaults
as though they were the user's settings, and the next unrelated toggle persists them: allowlist
gone, observation re-enabled, toast says "Saved".

**Fix**: a `loaded` flag gating `persist` until the initial fetch resolves, and surface the
rejection instead of swallowing it. Worth doing before Phase 5 adds controls for these fields —
after that, the panel will actively display an empty list *as* the user's allowlist.

**Fix**: gate `persist()` behind a `loaded` flag, or have `set_config` merge rather than replace.
Worth resolving before Phase 5 rather than after.

### LOW

- **L1 — Sessions hidden by the user's own prompt rules are still observed.**
  `observe_opening` runs before `set_first_prompt`
  ([lib.rs:345-349](src-tauri/src/lib.rs#L345-L349)), so a session about to be hidden by an
  existing `first_prompt_prefix` rule still increments its cluster. Harmless now; Phase 4 must skip
  proposals that match a rule already in `ignore_rules`, or it will re-propose what the user
  already accepted.
- **L2 — `take_dirty()` is consumed before the save can fail.**
  [lib.rs:919-922](src-tauri/src/lib.rs#L919-L922): if `save_observations` fails, the flag is
  already cleared, so nothing retries until a new observation arrives. Matches the documented
  tolerance, but the flag would be better consumed only on success.
- **L3 — `observe_opening` clones the whole `Config` and rebuilds `markers::Registry` per call.**
  Debounced to roughly once per session, so negligible — noted only because it sits on an event
  path.
- **L4 — Docs slightly overstate reveal stickiness.** `docs/IGNORE-RULES.md` and `CHANGELOG.md` say
  a revealed session stays visible "for the rest of its life"; a genuine `SessionStart` clears it
  (correctly — engine.rs:319-325). Worth one clause: "until it restarts".
- **L5 — `npm run format:check` fails on 35 files locally.** Verified pre-existing and unrelated:
  untouched files (`src/settings/Settings.tsx`) fail while the edited `src/state/config.ts` passes.
  A Windows CRLF-checkout artifact — CI runs on Linux with LF, so it won't block. The report's
  handling of this (scoped fix, no repo-wide sweep) was the right call per the project's
  parallel-session convention.

## What's notably right

- **H1's sibling case is handled correctly.** The guard reads hidden-ness *before* setting
  `revealed`, so it evaluates the deny rules rather than short-circuiting on the flag it's about to
  set, and the test deliberately builds a raw `HookEvent` because the `notif` helper would have
  overwritten the hidden cwd. That comment prevents a future silent test regression.
- **The `human_marked` propagation is implemented exactly as reasoned**, and
  `meta_and_tool_results_do_not_flag_human` pins the distinction that makes it correct — only
  `is_wrapper` rejections count as human evidence.
- **`classify_user_prompt` refactor preserves display-path behaviour**; `human_prompt` is now a
  thin adapter, and the existing descriptor tests pass untouched.
- **Deviation 2 (no `Engine::set_markers`) is the right call** — the registry has exactly one
  consumer, and threading a duplicate through `Engine` would have created a field with no reader.
- **`two_sessions_same_opening_reach_two` plants a sentinel `first` value** to catch an upsert
  overwriting it. That is testing the failure mode, not the happy path.

## Validation Results

| Check | Result | Notes |
| --- | --- | --- |
| `cargo fmt --check` | Pass | |
| `cargo clippy --all-targets -- -D warnings` | Pass | |
| `cargo test` | Pass | 109 lib + 1 integration (baseline was 82) |
| `npm run typecheck` | Pass | |
| `npm run lint` | Pass | |
| `npm run build` | Pass | reported; re-verified via typecheck |
| `npm run format:check` | Fail (pre-existing) | 35 files, CRLF artifact — see L5 |

## Files Reviewed

| File | Change |
| --- | --- |
| `src-tauri/src/observe.rs` | Added (+346) |
| `src-tauri/src/markers.rs` | Added (+134) |
| `src-tauri/src/engine.rs` | Modified (+257/-18) |
| `src-tauri/src/lib.rs` | Modified (+165) |
| `src-tauri/src/descriptor.rs` | Modified (+192) |
| `src-tauri/src/config.rs` | Modified (+80) |
| `src-tauri/src/ignore.rs` | Modified (+52) |
| `src-tauri/Cargo.toml` / `Cargo.lock` | Modified (`sha2 = "0.10"`) |
| `src/state/config.ts` | Modified (+24) |
| `docs/IGNORE-RULES.md` / `CHANGELOG.md` | Modified (+68) |

## Recommended Order

1. Fix **H1** + regression test (blocks merge)
2. Fix **M1** (cheap, and gets harder once records exist in the wild)
3. **M2** — intra-call fingerprint de-dup **first**, then the `min(len, actual)` clamp (order
   matters: the clamp without the de-dup counts a short opening five times per session)
4. Fix **M3** before Phase 5 adds the UI that makes it reachable
5. **L4** doc clause; fold **L1** into the Phase 4 plan
