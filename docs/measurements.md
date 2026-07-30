# Measurements (PRD Phase 6)

Closes the two open measurements from `.claude/PRPs/prds/headless-session-filter.prd.md`
("Evidence", decisions 3 and 6). Both are produced by committed test harnesses
(`src-tauri/tests/hook_payload_capture.rs`, `src-tauri/tests/prefix_sweep.rs`)
rather than one-off scripts, so either can be reproduced or re-run later.

This file contains **aggregate numbers only** — cluster counts, lengths,
booleans. No prompt text, file path, session id, or username from the
measured corpus appears below or in the committed harnesses. The corpus
itself is never committed (see `src-tauri/tests/fixtures/README.md`'s
"authored, never harvested" rule, which governs the *committed test fixtures*
and, by the same reasoning, the raw research corpus too).

---

## Decision 3 — does any wired hook payload field carry the first prompt?

**Method**: `src-tauri/tests/hook_payload_capture.rs` commits a keys-only,
sanitized reconstruction of the eight state-driving hook bodies
(`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`,
`Notification`, `Stop`, `SubagentStop`, `SessionEnd`), sourced from CLAUDE.md's
two empirically-verified blocks (the only live capture this repo has ever
done against its own listener) plus `engine::HookEvent`'s own field docs. A
test cross-checks a recorded verdict table against those bodies' actual keys,
so the answer can't silently drift from the committed data.

**Result — answered, with an explicit caveat, not a flat "no"**: within the
currently empirically-verified schema, **no** wired event carries the
session's first prompt directly. `descriptor::first_prompt`'s transcript-head
read stays load-bearing; there is no fast path to retire it today.

**Caveat on record**: Claude Code's public hooks documentation describes a
`prompt` field on `UserPromptSubmit` carrying the *current* turn's typed
text. This repo has never captured a live `UserPromptSubmit` body against its
own listener to confirm that field's presence (only the subagent `agent_id`
finding was captured that way), so it is not asserted as fact here. Even if
confirmed, it would answer "carries *a* prompt", not necessarily "carries the
*first* prompt" — whether a headless `--print` spawn's injected opening fires
`UserPromptSubmit` at all is a separate, unverified question. A dedicated
live capture is the natural follow-up if this fast path is ever wanted.

---

## Decision 6 — at what prefix length does a prefix stop discriminating?

**Method**: `src-tauri/tests/prefix_sweep.rs` (`#[ignore]`d; reads
`BEACON_CORPUS`, a local `~/.claude/projects`-shaped tree, never committed).
For every `.jsonl` file in the tree (recursive walk — a depth-1 glob missed 87
nested files during the PRD's original research), resolves the first prompt
via the real `descriptor::first_prompt` (a string-only parser missed 99
array-content prompts in that same original research; this reuses the fixed
parser). For each hypothetical prefix length 4–120, groups resolved prompts by
`observe::fingerprint` at that length and counts:

- **clusters** — groups of 2+ prompts sharing a fingerprint at this length.
- **mixed** — of those, how many contain *both* a human-marked opening
  (`FirstPrompt.human_marked`, or a built-in `markers::BUILTIN_HUMAN` match)
  and an unmarked one. This is the discrimination failure: a length short
  enough that an unrelated human and machine-shaped opening collide.
- The same two numbers **restricted to samples under 60 chars** (M1
  follow-through #2), plus how many resolved prompts that restriction covers
  (`sub60_n`) — most rows this small only capture naturally-short prompts,
  since `observe::sample` truncates *to* the swept length, not down to 60.

**Run**: 2026-07-30, against this developer's full local `~/.claude/projects`
tree.

- **756** `.jsonl` files walked.
- **568** resolved a first prompt (21 human-marked, 547 unmarked).
- **9** of those 568 are naturally shorter than 60 characters (~1.6%).

Results (`len` = hypothetical prefix length; `mixed`/`sub60_mix` = the
discrimination-failure count at that length):

| len | clusters | mixed | sub60_clusters | sub60_mixed | sub60_n |
|----:|---------:|------:|---------------:|------------:|--------:|
| 4–6 | 7 | 1 | 7 | 1 | 568 |
| 7 | 8 | 2 | 8 | 2 | 568 |
| 8 | 13 | **5 (peak)** | 13 | 5 | 568 |
| 9–13 | 11–12 | 4 | 11–12 | 4 | 568 |
| 14–17 | 10 | 4 | 10 | 4 | 568 |
| 18–20 | 9 | 3 | 9 | 3 | 568 |
| 21–25 | 10 | 2 | 10 | 2 | 568 |
| 26 | 9 | 1 | 9 | 1 | 568 |
| 27–56 | 9–10 | 1 | 9–10 | 1 | 568 |
| **57–59** | 8 | **0** | 8 | 0 | 568 |
| 60–120 | 8 | 0 | 1 | 0 | 9 |

(The full 4–120 per-length table is reproducible with
`BEACON_CORPUS=<path> cargo test --test prefix_sweep -- --ignored --nocapture`;
condensed here into runs where the values didn't change.)

**Verdict — a clean knee exists at 57 characters.** Mixed-polarity clusters
occur at every hypothetical length from 4 through 56 (peaking at 8 chars,
where a fifth of all clusters at that length mixed polarities), then drop to
**zero at 57 and stay zero for the entire remaining swept range (57–120)** —
64 consecutive lengths, over the full 568-prompt corpus. This is a single-
developer local corpus, not a cross-user sample — the same evidentiary
standard the PRD already accepted for decision 5 ("guarantee the mechanism,
not a figure we can't collect").

The existing `observe::PREFIX_LENS` floor (60) already sits past this knee.
The one real gap was naturally-short prompts (< 60 chars): `observe::sample`
has no length floor, so such a prompt is sampled at its own (short) length
with nothing to stop a resulting cluster from becoming a proposal. Only 9 of
568 resolved prompts (1.6%) fall into this case in the measured corpus, and
none formed a mixed cluster — small sample, but it costs nothing to close the
gap given the strong 57-char knee from the much larger 568-prompt sweep.

**Shipped**: `config::MIN_PROPOSE_SAMPLE_LEN = 60`, enforced in
`proposals::build` (`sample.chars().count() >= MIN_PROPOSE_SAMPLE_LEN`).
Chosen to match the existing `PREFIX_LENS` floor exactly — already validated
by this measurement — rather than introduce a second, unrelated constant.
Tests: `sample_below_measured_floor_is_not_proposed`,
`sample_at_the_floor_is_proposed` (`src-tauri/src/proposals.rs`).

**Not shipped this phase**: the `never_hide` short-entry UI warning from the
PRD's decision 6 UX sketch. The eligibility floor above is the load-bearing
half (M1 follow-through #2 asked for both); the warning is presentation-only
and depends on Phase 5's settings UI existing to host it — tracked there, not
blocked on more measurement.
