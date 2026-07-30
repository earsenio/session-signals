# Headless Session Filter — the Observer

## Problem Statement

Third-party tooling (ECC plugins, CI wrappers, automation scripts) launches headless
`claude --print` agents. These carry a real UUID `session_id` and **no** `agent_id`,
so Session Signals tracks them as ordinary primary sessions. On this machine that is
**462 of 577 sessions (80%)** — the widget fills with rows the user never started, and
the tray rollup goes orange for work nobody is waiting on. The tray light stops meaning
anything, which is the product's entire value.

## Evidence

- **Measured**: 577 sessions on one machine; 462 machine-spawned, 189 + 273 in two
  distinct spawner families. Both families are ECC (`observer-loop.sh:163`,
  `llm-summary.js:119`), not Claude Code itself — verified by reading the plugin source.
- **User report**: screenshot of the widget showing hex-named sessions the user did not start.
- **Falsified alternative**: `cwd` cannot separate them. 159 machine + 80 human sessions
  share a single cwd; a `claude -p` run inside the repo labels identically to the human session.
- **Falsified alternative**: `entrypoint` / `promptSource` looked discriminating until the
  user ran a plain terminal session — it reported `cli` / `typed`, same as expected, but the
  corpus had no plain-terminal human sessions to test against. Sampling bias, not signal.
- **Assumption still unvalidated**: all evidence comes from **one machine with one dominant
  plugin**. Cross-user generality is untested (see Phase C).
- **Known blind spot in the 0-false-positive result**: the human sessions in the corpus were
  largely IDE-wrapped, so the structural guard caught them cheaply. It says nothing about a
  user whose *own* openings repeat verbatim ("fix the…", "please review…") — those carry no
  structural marker and would cluster at threshold 3. This is an over-suggestion risk the
  corpus is structurally unable to measure, and it motivates the `never_hide` allowlist.

## Proposed Solution

Ship **no filters at all** by default, and give the app an *Observer*: it fingerprints each
new session's first-prompt **prefix**, clusters identical fingerprints, and when a cluster
reaches a threshold it **proposes** a hide rule the user accepts or dismisses. Rules are
plain `first_prompt_prefix` strings in config — readable, editable, and portable to any future
spawner without a rebuild.

Chosen over the alternatives because: hardcoded patterns name one third-party tool every user
would then carry; cwd matching is provably insufficient; and process inspection is a locked-out
decision in this codebase.

## Key Hypothesis

We believe **prefix clustering with a human-marker guard and a ≥3-session threshold** will
**remove machine-spawned noise without ever hiding a real session** for **users running
agentic tooling alongside interactive Claude Code**.

We'll know we're right when, on the 577-session corpus, recall is **100%** on both spawner
families and false positives are **0** — and when that holds on a second user's corpus.

## What We're NOT Building

- **Auto-hiding.** Proposals only. A session that silently vanishes is this app's worst failure mode.
- **Process inspection / terminal scraping.** Locked-out architectural decision.
- **Fork/resume twin suppression.** The hook payload carries no parent linkage; detecting it
  needs process inspection. Twins grey out via the normal stale sweep.
- **Any network egress.** Observation data is local, hashed, and expiring.
- **A `folder_hex` matcher.** Backtested: zero marginal coverage over `cwd_contains`, and it
  false-positives on SHA-named git worktrees, which this repo ships support for.
- **Regex in either rule list.** Prefix and substring only — regex makes rules unreadable at
  review time and opens catastrophic backtracking on attacker-influenced prompt text.
- **A shipped default allowlist.** `never_hide` is personal by definition; anything we could
  seed would be guesswork about one user's habits.

## Success Metrics

| Metric | Target | How Measured |
|--------|--------|--------------|
| False positives (human sessions hidden) | **0** | Fixture replay over the labelled 577-session corpus |
| Recall on known spawner families | **100%** (273/273, 189/189) | Same replay |
| Plaintext prompts on disk | **0 bytes** | Inspect the observation store after a session |
| Default-install behaviour | hides nothing | `IgnoreRules::defaults().is_empty()` |
| Every user can audit their own filtering | Audit view lists 100% of hidden sessions | Test: audit list == the set `snapshot()` excluded (replaces the uncollectable cross-user precision target) |
| Hidden sessions that turn red | **0** — and any occurrence is revealed, not swallowed | Reveal-on-block counter surfaced in settings; non-zero falsifies the "headless never blocks" premise |
| Proposal noise | ≤1 rejected proposal per user per week | Count `dismiss` + `never suggest` actions vs. `accept` |
| Allowlisted openings written to disk | **0** | Add a `never_hide` entry, start a matching session, inspect the store |
| Double-counted observations within one run | **0** | In-run dedup test: same session, many events, count increments once |

## Resolved Decisions — and the test that keeps each honest

Nothing here blocks implementation. Every item is decided, and every item has a mechanical
check that fails loudly if the decision turns out wrong.

| # | Question | Decision | Verified by | Phase |
| --- | --- | --- | --- | --- |
| 1 | Observation store shape | **`{fp: n}`** — count only, no session-id hashes | Unit test: the same session observed repeatedly within one run increments the count **once** (in-run dedup); a documented test asserts restart-mid-session may double-count and that this is tolerated | 2 |
| 2 | Can a headless session block on the user? | **No** — headless sessions must never wait for input. Default policy is therefore **hide**, not mute | **Reveal-on-block guard**: a hidden session that reaches `NEEDS_YOU` is un-hidden and counted. If the premise holds, the branch never fires; if it fails, nobody is stranded | 3 |
| 3 | Is the first prompt in the hook payload? | **Unknown — treated as "no."** Design already reads the transcript; payload availability would only be an optimisation | C1 capture asserts presence/absence of a prompt field across all wired events; a passing assert either unlocks the fast path or documents its absence | 6 |
| 4 | `<task-notification>` (n=5) polarity | **Left unclassified** — and that is safe, because unclassified openings are merely *eligible* for clustering, so the worst case is one user-reviewed proposal | Registry is **config-editable**, so polarity is assignable later with no rebuild; a test asserts an unclassified marker neither forces nor blocks clustering | 3 |
| 5 | Does precision survive off this machine? | **We don't claim it does.** Ship a per-user **self-audit** instead of a cross-user guarantee | Audit view lists every session a rule currently hides, with `hidden_count`; a test asserts the audit list exactly equals what `snapshot()` excluded | 5 |
| 6 | The `never_hide` short-entry warning length | **No number ships until measured.** The warning is omitted in Phase 3 | Phase 6 measurement derives the length at which a prefix stops discriminating on the corpus; the warning ships with that value, or not at all | 6 |

**Two of these changed the design**, not just the checkbox:

*#2 → a reveal-on-block guard.* Taking "headless never blocks" at face value would leave a
silent bet in the code. Instead the assumption becomes self-correcting: hidden sessions stay
hidden, except one that turns red, which is revealed and tallied. By the premise this is dead
code — which is exactly why it's cheap, and why it's the right way to hold a premise we believe
but cannot prove.

*#5 → self-audit replaces a cross-user precision target.* "≥95% on a second corpus" was a metric
we had no way to collect. A view answering *"what is being hidden from me right now?"* gives each
user the means to check their own setup, which is the honest version of the same goal.

---

## Users & Context

**Primary User**
- **Who**: A developer running Claude Code interactively *and* agentic tooling that spawns
  headless `claude --print` agents in the background.
- **Current behavior**: Glances at the tray to decide whether Claude needs them.
- **Trigger**: The widget fills with sessions they didn't start; the tray colour stops
  correlating with their own work.
- **Success state**: Every row in the widget is a session they personally started.

**Job to Be Done**
When background automation is running alongside my own sessions, I want the indicator to
track **only the sessions I'm part of**, so I can trust the tray colour at a glance.

**Non-Users**
- Users who run no agentic tooling — they see no machine sessions and the feature is inert
  (defaults `[]`, zero cost).
- Users wanting *observability* of their headless fleet. This hides them; it is not a monitor.

---

## Solution Detail

### Core Capabilities (MoSCoW)

| Priority | Capability | Rationale |
|----------|------------|-----------|
| Must | `first_prompt_prefix` matcher, anchored to the first prompt | The load-bearing signal; anchoring stops a session that merely *quotes* a phrase from being hidden |
| Must | Ship `ignore_rules: []` everywhere | Nothing hidden until asked; a shipped pattern would name one third-party tool |
| Must | Retry the first-prompt read (`Option<Instant>`, not a latch bit) | `SessionStart` fires before any prompt exists, so the first read is always empty — a bool latched there and the rule could never fire |
| Must | Array-content prompt extraction | 81 of 99 unreadable sessions carry the prompt as text blocks, not a string — ~17% blind spot |
| Must | Human-marker guard (`<command-`, `<local-command-`, `<ide_opened_file>`, `<ide_selection>`) | Accounted for 3 of 6 backtest false positives |
| Must | **`never_hide` allowlist** — user-authored patterns that are never clustered, never proposed, and never hidden | The structural guard only catches *marked* human sessions. A user with habitual openings ("fix the…", "please review…") clusters at threshold 3 and gets pestered — and the corpus cannot show this, because it reflects one person's habits |
| Should | "Dismiss permanently" on a proposal → writes a `never_hide` entry | The only ergonomic way to populate an allowlist is at the moment you're annoyed. Plain dismiss is not durable: the cluster keeps growing and re-proposes |
| Must | Threshold floor of 3, enforced **in code** | Measured leakage: 1 → 26 human patterns, 2 → 3, **3 → 0**, flat above |
| Must | Proposals, never auto-apply | The whole safety model |
| Must | Salted-hash observation store, no plaintext | The log covers the opening of *every* session and leaks via backups and bug reports |
| Should | Settings UI: ignore rules, `never_hide` list, proposals, hidden-count, threshold, clear-observations | earsenio's blocking review point #1 |
| Should | Multi-length fingerprints (60/70/85/100/120), shortest-set-wins | Forward insurance — see note below |
| Could | Machine-polarity markers | Registry exists; none confirmed yet |
| Won't | Auto-hide, process inspection, fork-twin suppression | See "What We're NOT Building" |

**On multi-length fingerprints — honest note.** Backtested at all five lengths: **0 false
positives at every length**, but **identical clusters at every length** (462 sessions covered
at 60 and at 120). Both ECC templates are stable well past 120 chars, so on today's corpus
multi-length adds *nothing*. Its value is a spawner whose template injects a variable (project
name, path, timestamp) before char 120 — exactly the unknown-spawner case the Observer exists
for, and exactly what one machine's corpus cannot show. Cheap to carry; recommended, but
labelled as insurance rather than measured gain.

**On the `never_hide` allowlist — three decisions worth stating.**

*It filters at ingest, not at proposal time.* An opening matching `never_hide` is never
fingerprinted and never written to the observation store at all. This is both simpler (no
hash round-trip to compare against a plaintext rule) and strictly better for privacy: the
prompts a user cares enough to allowlist are the ones that never touch disk.

*It outranks accepted ignore rules.* If a session matches both `never_hide` and an
`ignore_rules` entry, it stays **visible**. This follows the fail-open principle already
governing unreadable signals. The tradeoff is real — allowlisting something broad like
`"Please"` would unhide a spawner that opens with "Please analyze…" — but the failure
direction is right: a visible session you didn't want is noise, a hidden session you needed
is the failure this whole feature exists to avoid.

*Built-in structural markers stay built-in.* `never_hide` is user-authored and ships **empty**;
the four structural markers remain separate and read-only. They're correctness, not preference —
letting a user delete `<ide_selection>` would silently reintroduce a known false-positive class.
The UI shows them greyed so the behaviour stays explicable.

**Known limitation**: adding a `never_hide` entry cannot retroactively purge fingerprints
already stored, because hashes are one-way and an allowlist prefix is generally shorter than
the fingerprint lengths. "Dismiss permanently" *can* purge, since the live proposal holds its
fingerprint set in memory — which is another reason that path is the ergonomic one. A
hand-written entry takes effect for new observations only; `clear_observations` is the escape
hatch.

### How a proposal surfaces

Two tiers, both passive. The escalation ladder this app already owns — tray colour, then OS
notification — is reserved for *"Claude needs you."* Spending any of it on housekeeping
degrades the exact signal the feature exists to protect.

**Tier 1 — tray menu line (discovery).** A plain menu item, e.g.
`Session filtering: 1 suggestion…`, absent entirely when there are none. Deliberately **not**:

- a change to the tray **icon** — the icon encodes the rollup state (red/orange/green/grey) and
  must stay unambiguous. A badge or overlay for housekeeping corrupts the product's core promise.
- an **OS notification** — notifications are budgeted for blocked sessions. Never for this.
- a **widget banner** — the widget is small and always-on-top; a banner competes with session
  rows, which are the primary content.

**Tier 2 — Settings → "Session filtering" (decision).** Clicking the tray line opens it. The
proposal card carries everything needed to judge it without leaving the window:

```
┌──────────────────────────────────────────────────────────────┐
│  Suggested filter                                            │
│                                                              │
│  3 sessions have opened with:                                │
│  ┌────────────────────────────────────────────────────────┐  │
│  │ IMPORTANT: You are running in non-interactive --print   │  │
│  │ mode. You MUST use the Write tool…                      │  │
│  └────────────────────────────────────────────────────────┘  │
│  First seen 2 days ago · last seen 4 min ago                 │
│                                                              │
│  Hiding this would remove 2 sessions visible right now:      │
│    • ecc-homunculus (working)                                │
│    • ecc-homunculus (ready)                                  │
│                                                              │
│  [ Hide these ]  [ Not now ]  [ Never suggest this ]         │
└──────────────────────────────────────────────────────────────┘
```

The **live-preview line is load-bearing**, not decoration. It's the difference between
accepting an abstract pattern and seeing which rows disappear — the concrete form of "show me
what I'm about to lose." When no live session matches, say so plainly rather than hiding the line.

**At most one proposal is shown at a time** (highest count first). A list invites bulk-accept,
which is auto-hide with extra steps.

**Consequence of the in-memory invariant, stated honestly**: if a cluster crosses the threshold
entirely during a previous run, the count survives the restart but the sample text does not — so
nothing surfaces until one more matching session arrives and re-supplies it. That is a delay, not
a loss, and it is the correct trade: the alternative is persisting plaintext, which is the thing
we refused to do. Worth a one-line note in the docs so the behaviour doesn't read as a bug.

### MVP Scope

Phase A (**shipped**) + B1–B4. That is: correct extraction, empty defaults, hashed observation
store, clustering, and `list_proposals` / `accept_proposal`. Enough to validate the hypothesis
even before the settings UI, via commands.

### User Flow

```
background tooling spawns a headless agent
  → opening matches never_hide or a structural marker? → drop it, nothing stored   ← allowlist
  → otherwise Observer fingerprints the first-prompt prefix (hashed, local)
  → 3rd session with the same fingerprint arrives (sample text now in memory)
  → tray menu grows a quiet line: "Session filtering: 1 suggestion…"   ← no icon change, no notification
  → clicking it opens Settings → proposal card w/ sample text + live-session preview
      ├─ Accept            → plaintext first_prompt_prefix rule appended to ignore_rules
      ├─ Dismiss           → hidden this round; may re-propose as the cluster grows
      └─ Never suggest     → prefix appended to never_hide + that fingerprint set purged
  → accepted sessions leave the widget; "N sessions hidden" reflects the count
```

---

## Technical Approach

**Feasibility**: **HIGH** — Phase A is already implemented, tested (82 Rust tests), and pushed.

**Architecture Notes**
- `sha2` is already transitively in the dep tree via Tauri — hashing adds no build cost.
- Per-install 32-byte salt, distinct from the listener auth token (never reuse a secret across
  purposes). Fingerprints are non-comparable across machines.
- **Invariant**: a proposal is surfaced only when its sample text is live in memory. A pattern
  you cannot read is one you must not be asked to accept — so proposals are never reconstructed
  from disk, and disk stays hash-only.
- Transcript reads are bounded head-reads (64KB) taken off the engine lock. Whole-file reads
  crashed on 32MB transcripts.
- **Fail open**: an unreadable or absent signal means the session stays **visible**.

**Honest limitation, to state in the docs**: hashing short, low-entropy prompts is not anonymity
against a targeted adversary who can hash candidate strings and compare. The salt raises that
cost and blocks cross-machine correlation. What it *does* fully defeat is the realistic risk —
a readable log of your prompts sitting in JSON that gets synced, backed up, or attached to an issue.

**Technical Risks**

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| Hiding a real session | L | Defaults `[]`; proposals never auto-apply; threshold floor 3 in code; `never_hide` outranks every ignore rule |
| Nagging the user with wrong proposals | M | `never_hide` + one-click "never suggest"; tracked as the proposal-noise metric |
| A broad `never_hide` entry masks a real spawner | L | Fail-open is deliberate; `hidden_count` and the audit view stay visible so the discrepancy is spottable. **No short-entry warning ships until its length is measured** (decision 6) — an invented number would read as authoritative |
| Signal breaks on a Claude Code upgrade | M | Fail open. Version churn is real — `2.1.201 → 2.1.220` inside one working session |
| Precision doesn't generalize off this machine | **M-H** | Still the weakest evidence here. Mitigated by *equipping* rather than *claiming*: per-user audit view, `hidden_count`, empty defaults, and proposals that never auto-apply. We guarantee the mechanism, not the numbers |
| A hidden session genuinely needs the user | L | Reveal-on-block guard un-hides it and tallies the event — the premise is monitored, not assumed |
| Config migration on removed matcher kinds | L | `deserialize_lenient` drops unknown kinds; one stale entry must never reset every unrelated setting |
| My own analysis bugs | M | Three occurred during this research (shell escaping; a depth-1 glob missing 87 nested files; string-only content missing 99 prompts) — hence fixture-based regression tests over ad-hoc scripts |

---

## Implementation Phases

| # | Phase | Description | Status | Parallel | Depends | PRP Plan |
|---|-------|-------------|--------|----------|---------|----------|
| 1 | Foundation fixes | Latch retry, array-content extraction, IDE markers, drop `folder_hex`, empty defaults, opt-in docs | **complete** | - | - | `docs/internal/implementation-plan.md` §A |
| 2 | Observation store | Salted-hash fingerprints, multi-length, expiry | **complete** | - | 1 | `.claude/PRPs/plans/completed/observation-store-and-allowlist.plan.md` |
| 3 | Marker registry + allowlist | Human/machine polarity; user-authored `never_hide`, evaluated at ingest | **complete** | with 2 | 1 | `.claude/PRPs/plans/completed/observation-store-and-allowlist.plan.md` |
| 4 | Clustering + proposals | Group, threshold floor 3, `list`/`accept`/`dismiss`/`clear` commands | **complete** | - | 2, 3 | `.claude/PRPs/plans/completed/clustering-and-proposals.plan.md` |
| 5 | Settings UI | Rules editor, proposals, hidden-count, threshold control | pending | - | 4 | - |
| 6 | Fixtures + validation | Corpus fixtures; C1 hook-payload capture; C2 cross-user | pending | with 5 | 4 | - |

### Phase Details

**Phase 1: Foundation fixes** — *complete*
- **Goal**: Answer earsenio's four blocking review points and fix two silent bugs.
- **Scope**: `engine.rs`, `descriptor.rs`, `ignore.rs`, `config.rs`, `config.ts`, `docs/IGNORE-RULES.md`.
- **Success signal**: Commits `8f36494`, `ba2ec8e`, `68f42f2` on `feat/headless-session-filter`;
  82 Rust tests plus fmt/clippy/typecheck/lint/prettier green; draft PR #31 open.

**Phase 2: Observation store**
- **Goal**: Record what sessions open with, legibly to nobody.
- **Scope**: Salt generation + persistence; `normalize` + fingerprint at 60/70/85/100/120;
  `retain_days` (default 30) pruning. Record shape is **`{fp, n, first, last}`** — a count, no
  session-id hashes (decision 1). An **in-run dedup set** of already-observed session ids keeps
  the retry path (`first_prompt_due` fires every 5s until resolved) from inflating counts; that
  set is free, since live sessions are already tracked in memory.
- **Success signal**: After a session, the store contains only hex fingerprints and counts;
  grepping it for any prompt substring returns nothing. A session re-read many times in one run
  increments its count exactly once.
- **Accepted residual**: a restart *mid-session* can double-count that session. Tolerated —
  headless sessions are short-lived, so the window is small, and inflation only makes a proposal
  appear sooner. The false-positive guard is the marker/allowlist check, never the count.
- **Report**: `.claude/PRPs/reports/observation-store-and-allowlist-report.md`

**Phase 3: Marker registry + `never_hide` allowlist**
- **Goal**: Keep known-human openings out of clustering — both the structurally-marked ones
  (built in) and the ones only this user knows about (allowlist).
- **Scope**: `MarkerPolarity { Human, Machine }` seeded with the four confirmed markers and
  **editable from config**, so an unclassified marker like `<task-notification>` gains a polarity
  later with no rebuild (decision 4). `never_hide: Vec<Matcher>` in config, defaulting `[]`,
  evaluated at ingest **before** fingerprinting; precedence over `ignore_rules` in
  `session_hidden()`. Plus the **reveal-on-block guard** (decision 2): a hidden session reaching
  `NEEDS_YOU` is un-hidden and counted. **No short-entry warning** — its length isn't measured yet
  (decision 6).
- **Success signal**: A session opening with `<ide_selection>` is never clustered. A prefix
  added to `never_hide` produces no store entry at all — verified by inspecting the store,
  not just the proposal list. An *unclassified* marker neither forces nor blocks clustering. A
  hidden session driven to `NEEDS_YOU` reappears in `snapshot()` and increments the reveal counter.
- **Report**: `.claude/PRPs/reports/observation-store-and-allowlist-report.md`

**Phase 4: Clustering + proposals** — *complete*
- **Goal**: Turn observations into an offer.
- **Scope**: Group eligible fingerprints; shortest-prefix-wins de-duplication across lengths;
  floor-3 clamp; commands `list_proposals`, `accept_proposal`, `dismiss_proposal`,
  `never_suggest_proposal` (→ `never_hide` + purge), `clear_observations`.
- **Success signal**: Synthetic corpus replay yields one proposal per family (two machine
  families surfaced, a repeated-human blind-spot case documented and removable via
  "never suggest"). Real 577-session corpus replay with an exact false-positive count is
  deferred to Phase 6, which owns the fixtures.
- **Plan**: `.claude/PRPs/plans/completed/clustering-and-proposals.plan.md` — also carried
  the Phase 2/3 review findings H1, M1, M2, because M2 changes `Observation.len`, which
  this phase's shortest-prefix de-duplication reads.
- **Report**: `.claude/PRPs/reports/clustering-and-proposals-report.md`

**Phase 5: Settings UI**
- **Goal**: Make rules visible, auditable, and reversible.
- **Scope**: Reuse `Section` / `Toggle` / `patch` in `src/settings/Settings.tsx`. Two editable
  lists (`ignore_rules`, `never_hide`) plus the read-only built-in markers; three-way proposal
  actions; wire `Engine::hidden_count()` (implemented; currently read only by tests). Plus the
  **audit view** (decision 5): every session currently hidden, and which rule hides it — the
  per-user substitute for a cross-user precision claim. Reveal-on-block count shown here too.
- **Success signal**: A user can add, edit, and delete rules in both lists and see the hidden
  count change; "Never suggest" moves a proposal into `never_hide` in one click; the tray line
  appears only when a proposal is live and never alters the tray icon; the card's live-session
  preview matches what actually disappears on accept.

**Phase 6: Fixtures + validation**
- **Goal**: Stop trusting one machine and one set of ad-hoc scripts.
- **Scope**: Real captured sessions as fixtures, including the adversarial cases — a human
  session quoting a machine phrase; a SHA-named worktree; a machine and a human session sharing
  one cwd; **a repeated human opening that should be allowlisted rather than proposed**; **a
  `never_hide` entry that overlaps an accepted ignore rule** (must stay visible). Plus two
  **measurements that close decisions 3 and 6**:
  - *Hook-payload capture*: assert, across every wired event, whether any field carries the first
    prompt. A pass either unlocks a fast path (no transcript read) or documents its absence — both
    are useful outcomes, so this cannot block.
  - *Prefix-discrimination sweep*: find the length at which a prefix stops separating families on
    the corpus. That number becomes the `never_hide` short-entry warning — or, if no clean knee
    exists, the warning is dropped rather than shipped with an invented threshold.
- **Success signal**: Both measurements produce a recorded number (or a recorded "no signal");
  every adversarial fixture passes. Cross-user replay stays *desirable but not gating* — the
  shipped guarantee is the audit mechanism, not a precision figure we can't collect.

### Parallelism Notes

Phases 2 and 3 are near-independent (store vs. registry/allowlist) and both feed 4, but they
meet at one point: the allowlist is evaluated *before* the store fingerprints anything. Agree
that ingest ordering up front — a single guard clause — and the two can then proceed in
parallel. Phase 6's C1/C2 validation can run alongside 5, since its findings change *defaults*
(hide vs. mute) rather than structure.

---

## Decisions Log

| Decision | Choice | Alternatives | Rationale |
|----------|--------|--------------|-----------|
| Default rule set | `[]` — hide nothing | Ship the ECC patterns | Any shipped pattern names one third-party tool; every user would carry filters for software they don't run (earsenio #3) |
| Discriminator | First-prompt prefix | cwd; `entrypoint`/`promptSource`; folder-name shape | cwd provably insufficient (159 machine + 80 human share one); the entrypoint fields were falsified by a live terminal run |
| `folder_hex` matcher | Deleted | Keep as a fallback | Zero marginal coverage over `cwd_contains`; false-positives on SHA-named worktrees, which this repo supports |
| Suffix in fingerprint | Dropped | prefix + suffix | Measured: distinct suffixes = 1. Added no separation, doubled the exposure |
| Threshold | Floor 3, clamped in code | UI default only | 1 → 26 human patterns leaked, 2 → 3, 3 → 0. A UI default is bypassable by hand-editing config |
| Prefix lengths | 60/70/85/100/120, shortest-set-wins | 120 only | Identical on today's corpus; insurance against a spawner that varies before char 120 |
| Storage | Salted hash only, no plaintext | Plaintext prefixes | The log grows to cover every session's opening and leaks via backups, syncs, bug reports |
| Action on match | Propose | Auto-hide | A silently vanished session is the worst failure mode |
| Proposal surfacing | Quiet tray menu line → Settings card | OS notification; tray icon badge; widget banner | Icon and notifications are budgeted for "Claude needs you"; spending either on housekeeping degrades the signal this feature exists to protect |
| Proposals shown at once | One, highest count | A list | A list invites bulk-accept, which is auto-hide with extra steps |
| Proposal card content | Sample text **+ which live sessions would vanish** | Pattern text alone | Accepting an abstract pattern is not informed consent; the preview is the concrete "what am I about to lose" |
| Over-suggestion control | `never_hide` allowlist, user-authored, `[]` by default | Raise the threshold; smarter heuristics | Raising the threshold delays *correct* proposals as much as wrong ones and is a blunt global knob; the allowlist is precise and costs nothing when unused |
| Allowlist evaluation point | At ingest, before fingerprinting | At proposal time | Allowlisted openings never touch disk — the strictly better privacy position, and no hash/plaintext comparison is needed |
| Allowlist vs. ignore precedence | `never_hide` wins | ignore wins; error on conflict | Fail open. Extra noise is recoverable; a hidden session you needed is not |
| Allowlist matcher syntax | Reuse `Matcher` (prefix / cwd substring) | Regex | No new schema, and users already understand the shape. Regex is a footgun — unreadable rules and catastrophic backtracking on hostile input |
| Structural markers | Built-in, read-only, separate from `never_hide` | Seed `never_hide` with them | They're correctness, not preference; a user deleting `<ide_selection>` would silently reintroduce a known false-positive class |
| Observation record | `{fp, n, first, last}` — count only | `{n, last_sid_hash}`; full `sids[]` | Headless sessions are short-lived, so in-run dedup covers the realistic double-count. Restart-mid-session inflation is tolerated: it only makes proposals appear sooner, and the false-positive guard is the marker check, not the count |
| Policy for hidden sessions | **Hide**, plus reveal-on-block | Mute instead of hide; hide unconditionally | Headless sessions never wait for input, so hiding is right. Reveal-on-block turns that premise into a monitored invariant instead of a silent bet — dead code if the premise holds |
| Marker polarity assignment | Config-editable registry | Hardcoded enum | `<task-notification>` is unclassified and doesn't need to block: unclassified means merely *eligible*, so the worst case is one reviewed proposal, and polarity can be set later without a rebuild |
| Cross-user precision | Ship a **self-audit view**; make no numeric claim | Assert ≥95% on a second corpus | The target was uncollectable. Showing each user exactly what is hidden, and why, is the honest form of the same goal |
| Unmeasured UI thresholds | Ship nothing until measured | Ship `~12 chars` as a starting point | A number in a UI reads as authoritative. Phase 6 derives it or the warning is dropped |

---

## Research Summary

**Market Context**
No comparable product filters agent-spawned sessions — this class of noise only exists once a
user runs agentic tooling on top of an interactive coding agent, which is recent. The nearest
analogues are log-aggregation "noise rules" (Datadog, Sentry inbound filters): all of them are
user-authored, none auto-apply, and all keep the rule text human-readable. The Observer follows
that convention deliberately.

**Technical Context**
- Two ECC spawner families account for all 462 machine sessions; both openings are emitted by the
  plugin, not Claude Code (`observer-loop.sh:163`, `llm-summary.js:119`).
- Backtest result: prefix clustering + human-marker guard + threshold 3 gives **100% recall,
  0 false positives** across 577 sessions.
- Transcript JSONL must be read as a stream with early exit; files reach 32MB and whole-file
  reads crashed.
- `sha2 0.10.9` is already transitive via Tauri.

---

*Generated: 2026-07-28 · decisions resolved 2026-07-29*
*Status: READY — Phase 1 shipped; no open questions. Every prior uncertainty is a decision with a
test that fails loudly if it was wrong. Two measurements (hook-payload reachability, prefix
discrimination length) are scheduled work in Phase 6, not blockers.*
