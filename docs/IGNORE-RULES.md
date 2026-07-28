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
