# Implementation Report: Observation store + marker registry & `never_hide` allowlist

## Summary

Session Signals now reads each session's opening prompt once (when observation is on
or a prompt ignore rule exists) and records a **salted hash** of it — never
plaintext — so a later phase can offer the user a filter rule built from a
pattern it actually saw. Two guards run before anything is fingerprinted:
Claude Code's own human-interaction markers (built-in, plus additive
config), and a new `never_hide` allowlist that also outranks `ignore_rules`
entirely. A hidden session that ever hits a real block on the user
(permission prompt, plan approval) is un-hidden for the rest of its life —
a safety valve so a filter guess can never swallow a genuine request for
the user.

## Assessment vs Reality

| Metric | Predicted (Plan) | Actual |
| --- | --- | --- |
| Complexity | Large | Large — matched |
| Files Changed | 10 (4 created, 6 updated) | 11 (2 created, 9 updated — Cargo.lock auto-updated by the new dep) |
| New tests | ~35 | ~27 new + several existing tests extended with new assertions |

## Tasks Completed

| # | Task | Status | Notes |
| --- | --- | --- | --- |
| 1 | Ingest ordering contract | Done | Implemented as `maybe_refresh_hidden`'s doc comment + structure in `lib.rs`; no standalone code |
| 2 | `descriptor::first_prompt` returns `human_marked` | Done | New `FirstPrompt` struct; `human_prompt` refactored into `classify_user_prompt`/`UserPromptKind` so display-path behavior (`extract_from_str`) is unchanged |
| 3 | `markers.rs` registry | Done | `descriptor::is_wrapper` now sources `BUILTIN_HUMAN` from here |
| 4 | `sha2` dep + `observe::salt` | Done | Separate store key from `auth_token`, verified by test |
| 5 | `observe::sample`/`fingerprint` | Done | |
| 6 | `observe::Observations` store | Done | |
| 7 | Config fields + TS mirror | Done | `never_hide`, `markers`, `observe_enabled`, `observe_retain_days` |
| 8 | Engine observation gate | Done | `first_prompt_due` now also fires when `observe_enabled` |
| 9 | `lib.rs` wiring | Done | `AppState` gains `observe_salt`/`observations`; setup loads them; sweep thread prunes + flushes |
| 10 | Ingest guards | Done | `observe_opening` — marker guard, then `never_hide` guard, then `observe()` |
| 11 | `ignore.rs` allowlist API | Done | `matches_cwd`/`matches_prompt`/`matches`; `cwd_hidden`/`prompt_hidden` kept as inline aliases |
| 12 | `never_hide` precedence | Done | `session_hidden` fails open on a `never_hide` match, checked first |
| 13 | Reveal-on-block + counter | Done | `Session.revealed` (sticky, reset on `SessionStart`), `Engine.reveal_count` |
| 14 | Docs | Done | `docs/IGNORE-RULES.md` new section; `CHANGELOG.md` two entries |

## Validation Results

| Level | Status | Notes |
| --- | --- | --- |
| Static Analysis | Pass | `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `npm run typecheck`, `npm run lint` all clean |
| Unit Tests | Pass | 109 lib tests (was 82 baseline), 1 integration test, all green |
| Build | Pass | `cargo build`, `npm run build` (tsc + vite) both clean |
| Integration | N/A | No new integration test added for `observe_opening`/`maybe_refresh_hidden` — see Deviations |
| Format | Pass (scoped) | `npm run format:check`: the one file I edited that was failing (`src/state/config.ts`) now passes; ~35 pre-existing unrelated files remain non-compliant (CRLF line-ending artifacts across the whole checkout) and were deliberately left untouched per the repo's no-repo-wide-sweeps convention |

## Files Changed

| File | Action | Lines |
| --- | --- | --- |
| `src-tauri/src/observe.rs` | CREATED | +346 |
| `src-tauri/src/markers.rs` | CREATED | +134 |
| `src-tauri/src/engine.rs` | UPDATED | +275/-? (net +275 per diffstat, includes deletions) |
| `src-tauri/src/lib.rs` | UPDATED | +165 |
| `src-tauri/src/descriptor.rs` | UPDATED | +192 (incl. refactor of `human_prompt`) |
| `src-tauri/src/config.rs` | UPDATED | +80 |
| `src-tauri/src/ignore.rs` | UPDATED | +52 |
| `src-tauri/Cargo.toml` | UPDATED | +4 (`sha2` dep) |
| `src-tauri/Cargo.lock` | AUTO-UPDATED | via `cargo build` after the new dep |
| `src/state/config.ts` | UPDATED | +24 |
| `docs/IGNORE-RULES.md` | UPDATED | +54 |
| `CHANGELOG.md` | UPDATED | +14 |

## Deviations from Plan

- **No AppHandle-level integration test for `observe_opening`/`maybe_refresh_hidden`.**
  The plan's testing-strategy table lists `allowlisted_opening_is_never_stored`
  and similar as if they were unit tests, but both functions require a live
  `tauri::AppHandle` (for `app.store(...)`), and this codebase has no
  `tauri::test` harness anywhere — confirmed by checking `token.rs` (tests
  only `generate()`, never `load_or_create`) and the one existing integration
  test (`tests/hooks_install.rs`, which only covers the pure-filesystem
  `hooks::install`/`uninstall`, not anything needing an `AppHandle`). Every
  guard's *individual* logic is unit-tested in its own module
  (`markers::Registry::is_human`, `ignore::IgnoreRules::matches`,
  `observe::Observations::observe`); the wiring itself is a short, direct
  function verified by `cargo build` + full test suite + manual-validation
  checklist (see Next Steps). This mirrors existing project precedent rather
  than a new gap.
- **`Engine` does not hold a `markers::Registry`.** The plan's Task 9 prose
  mentions `eng.set_markers(...)` "alongside the existing `set_ignore_rules`
  call," but no task ever defines `Engine::set_markers`, and the marker
  registry is only consulted by the ingest guard (Task 10), never by
  visibility/hiding logic. `observe_opening` builds a `markers::Registry`
  fresh from the live config on each call instead (cheap — a small `Vec`
  filter) rather than threading a duplicate copy through `Engine`. This
  avoids an unused field and an extra setter with no consumer.
- Everything else matches the plan's task list, patterns, and gotchas as
  written.

## Issues Encountered

- Two of my own test constructions were briefly wrong (not implementation
  bugs): `fingerprint_survives_whitespace_variation` originally truncated two
  differently-shaped strings to different content before comparing; fixed by
  comparing untruncated strings differing only in one whitespace character.
  `store_json_contains_no_prompt_text`'s char-whitelist assertion mistook
  `Observation`'s own field names (`len`, `first`, …) for leaked plaintext;
  replaced with a per-word substring check. Both caught immediately by the
  test run and fixed before moving on.
- One real (but expected) build break: changing `descriptor::first_prompt`'s
  return type in Task 2 broke its one caller in `lib.rs`; fixed with a
  documented interim shim, later replaced by the full Task 9/10 rewrite.

## Tests Written

| Test File | Tests | Coverage |
| --- | --- | --- |
| `src-tauri/src/descriptor.rs` | +2 new, 3 extended | `human_marked` propagation through wrapper-skip and meta/tool-result paths |
| `src-tauri/src/markers.rs` | 4 (new file) | built-in classification, additive config, override rejection, unclassified |
| `src-tauri/src/observe.rs` | 14 (new file) | salt, sample/fingerprint (whitespace/multibyte/truncation), dedup, prune, JSON no-leak, garbage tolerance |
| `src-tauri/src/config.rs` | 2 (new test module) | backward-compat load, retain-days sanitization |
| `src-tauri/src/ignore.rs` | +1 | `matches()` cwd/prompt/neither |
| `src-tauri/src/engine.rs` | +5 | observation gate, `never_hide` precedence (cwd + prompt), reveal-on-block lifecycle, zero-reveal premise-holds case |

## Next Steps

- [ ] Code review via `/code-review`
- [ ] Manual validation (from the plan, not automatable here — requires
      running the live desktop app): `npm run tauri dev`, confirm
      `observe_salt`/`observations` appear in `beacon.json`, `grep` for a
      typed phrase finds nothing, a `never_hide` entry suppresses a new
      observation, and a hidden session driven to `NeedsYou` reappears with
      a notification.
- [ ] Create PR via `/prp-pr`
