# Ignoring machine-spawned sessions

Some tooling launches Claude Code in headless mode (`claude --print`) in the
background. Those runs are real sessions — they carry a normal `session_id` and
no `agent_id`, so Session Signals cannot tell them apart from your own work and
shows them as ordinary rows. If you run such tooling, the widget fills with
sessions you never started and the tray colours for work you aren't doing.

`ignore_rules` lets you hide them.

> **Session Signals ships with no ignore rules at all.** Nothing is hidden until
> you add a rule. A session that silently disappears is the worst thing this app
> can do — it exists to make sure you don't miss one — so filtering is always
> opt-in, and the patterns below name specific third-party tools that most users
> don't run.

## Where the rules live

In the Session Signals store (`beacon.json` in the app config dir), under
`config.ignore_rules`.

## Rule kinds

### `first_prompt_prefix` — match the session's opening prompt

```jsonc
{ "kind": "first_prompt_prefix", "value": "IMPORTANT: You are running in non-interactive" }
```

Hides a session whose **first** prompt starts with this text (case-insensitive,
leading whitespace ignored). This is the reliable one: a spawner injects a fixed
instruction, and that opening identifies it.

It is deliberately **anchored to the first prompt**, so a session where *you*
merely mention the phrase later is never hidden.

Sessions that open with one of Claude Code's own interaction markers —
`<command-…>`, `<local-command-…>`, `<ide_opened_file>`, `<ide_selection>` — are
never matched. Those mean a human typed a slash command or opened a file, so a
long autonomous run you started with `/some-command` stays visible.

### `cwd_contains` — match the working directory

```jsonc
{ "kind": "cwd_contains", "value": "ecc-homunculus" }
```

Hides a session whose working directory contains this substring
(case-insensitive). Convenient when a spawner uses its own scratch directory.

**Use with care.** A directory rule cannot separate machine sessions from your
own when both run in the same folder — which is common, since background tooling
often analyses the repo you're working in. Prefer `first_prompt_prefix`.

## Keeping your own openings out (`never_hide`)

Session Signals also **observes** which session openings repeat, so it can
eventually offer you a ready-made `first_prompt_prefix`/`cwd_contains` rule
built from a pattern it actually saw — instead of you having to write one by
hand from the recipe below. `never_hide` is how you keep specific openings of
your own out of that observation entirely, and — **`never_hide` always
wins** — out of `ignore_rules` too, even if a rule would otherwise hide them.

```jsonc
"never_hide": [
  { "kind": "first_prompt_prefix", "value": "please review the listener" },
  { "kind": "cwd_contains", "value": "my-personal-scratch-repo" }
]
```

Same two matcher shapes as `ignore_rules`, same precedence logic — but
inverted: a `never_hide` match makes a session visible (or keeps it visible)
regardless of what `ignore_rules` says about the same cwd or prompt. If a
prefix appears in both lists, the session stays **visible**. This fails open
on purpose: extra noise in the widget is recoverable, a session you needed
that silently vanished isn't.

### What Session Signals records

With observation on (`observe_enabled`, on by default), Session Signals reads
each session's first prompt once and stores a **salted hash** of its opening —
never the prompt text itself. Concretely, per distinct opening: a few
128-bit fingerprints (one per tracked prefix length), a count, and first/last-seen
timestamps. Records older than `observe_retain_days` (30 by default) are
pruned automatically. `grep`-ing `beacon.json` for anything you typed will
never find it.

Honest limitation: hashing a short, low-entropy prompt is not anonymity
against someone who can hash a dictionary of candidate strings. What it
defeats is a readable prompt log sitting in JSON that could get synced,
backed up, or attached to a bug report.

An opening a human marker precedes — a slash command, `<ide_opened_file>`,
`<ide_selection>` — is never observed either, on the same reasoning as the
`ignore_rules` anchoring above: those are evidence of you at the keyboard, not
a repeatable machine pattern.

Set `observe_enabled: false` to turn observation off entirely; existing
records are left alone until you clear them.

### A hidden session that blocks on you always reappears

If a session hidden by `ignore_rules` ever needs you — a permission prompt, a
plan to approve — it is un-hidden **until it restarts**, notifies like any
other session, and colours the tray red. This is a safety valve, not a bug:
a filter is a guess about what's machine-spawned, and a guess should never be
allowed to swallow a request for you specifically. A genuine restart
(`SessionStart`) clears the reveal, so the rule applies normally again from
the next run.

## Suggested filters

Instead of writing a rule by hand, Session Signals can offer you one built from
a pattern it actually observed. `list_proposals` returns every eligible
cluster — an opening seen at least `propose_threshold` times (3 by default,
floored at 3 in code) — highest count first, each with its readable sample
text and the currently-visible sessions that would disappear on accept.

Three actions, none of them automatic:

- **Accept** (`accept_proposal`) — writes the sample as a `first_prompt_prefix`
  entry in `ignore_rules`. Idempotent: accepting twice writes one rule.
- **Dismiss** (`dismiss_proposal`) — "not now," this run only. The proposal
  reappears once the cluster grows past its count at dismissal.
- **Never suggest** (`never_suggest_proposal`) — adds the sample to
  `never_hide` *and* purges every fingerprint whose live sample is
  prefix-related to it from the observation store, so it stops surfacing
  again this run. A record with no live sample (carried over from a previous
  run) can't be matched to that family and may still exist on disk — hashes
  are one-way, so a purge can never be total across a restart.
  `clear_observations` is the full reset if you want one.

Nothing is applied without one of these three actions — a proposal sitting
unaddressed changes nothing.

Two honest caveats:

- **A cluster that crossed the threshold entirely in a previous run
  surfaces only after one more matching session re-supplies the sample
  text.** The count is persisted; the readable sample is not (see "What
  Session Signals records" above). This is a delay, not a loss — the
  alternative was persisting plaintext.
- **The preview list (`matching`) can be shorter than `count`.** `count`
  groups on the whitespace-normalized opening; the rule a proposal writes is
  a literal prefix, and only sessions live *right now* can appear in the
  preview. Seeing fewer rows than the count is expected, not a bug.

## Recipe: ECC (`continuous-learning` / homunculus observer)

The [ECC plugin](https://github.com/affaan-m/ECC) spawns `claude -p` for two
background jobs. Both use fixed openings, so one rule each is enough:

```jsonc
"ignore_rules": [
  { "kind": "first_prompt_prefix", "value": "IMPORTANT: You are running in non-interactive" },
  { "kind": "first_prompt_prefix", "value": "Below is a conversation log from a Claude Code" }
]
```

Source of those strings, if you want to verify them against your installed
version:

- `skills/continuous-learning-v2/agents/observer-loop.sh` — the instinct observer
- `scripts/lib/llm-summary.js` — the session summariser (calls `claude -p` directly)

Optionally add the scratch directory, which also catches observer runs before
their first prompt is written:

```jsonc
{ "kind": "cwd_contains", "value": "ecc-homunculus" }
```

## Writing a rule for other tooling

1. Find the session in `~/.claude/projects/<project>/<session-id>.jsonl`.
2. Read its first `user` (or `queue-operation`) record — that's the opening prompt.
3. Take a distinctive **leading** fragment, long enough not to collide with
   anything you'd type yourself.
4. Add it as a `first_prompt_prefix` rule.

Prefer a longer prefix over a shorter one. `"IMPORTANT:"` alone would hide any
session that happens to begin that way, including yours.

## Behaviour of hidden sessions

Hidden sessions are still tracked — they simply never reach the widget list, never
colour the tray, and never raise a notification. Remove the rule and they reappear
immediately; no state is lost.

## Turning it off

Set `ignore_rules` to `[]`. That is also the shipped default.
