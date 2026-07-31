# Fixture corpus

Fixtures for `corpus_replay.rs` (PRD Phase 6). Each file is a miniature Claude
Code transcript — one JSONL record per line — read by exactly one function:
[`descriptor::first_prompt`](../../src/descriptor.rs), which only ever reads
the **head** of the file (`MAX_HEAD_BYTES`) looking for the session's
*earliest* queued instruction or human-typed prompt. Nothing else in these
files matters — no `ai-title`, no `assistant` turns, no tail content — because
nothing else in this codebase's ignore/observation pipeline reads them.

## Standing rule: authored, never harvested

**No fixture may contain a real prompt, path, repo name, or session id copied
from anyone's machine.** Every fixture here is written to *reproduce the
shape* a real transcript exhibits — a paraphrase, not a copy. This is the one
place this phase deliberately departs from the PRD's "real captured sessions
as fixtures" wording; see the plan's Notes for why. If you're adding a case,
write it from scratch in the same voice as the existing files.

## The two record shapes `first_prompt` resolves

`first_prompt_from_str` (`descriptor.rs:85`) scans lines top to bottom and
returns the **first** record that is either:

1. A queued instruction — how a headless/spawned run is seeded:
   ```json
   {"type":"queue-operation","operation":"enqueue","content":"IMPORTANT: You are running in a non-interactive automated review session. …"}
   ```
2. A genuine human-typed `user` prompt. `message.content` is **either a plain
   string or an array of content blocks** — both are common, and the array
   form was a real blind spot (Phase 1 fixed it after ~17% of prompts were
   invisible to this exact reader):
   ```json
   {"type":"user","message":{"role":"user","content":"a typed prompt"}}
   {"type":"user","message":{"role":"user","content":[{"type":"text","text":"a typed prompt"}]}}
   ```

Anything else on the way — `isSidechain`/`isMeta` entries, `tool_result`
blocks, malformed JSON — is skipped, not fatal.

## Wrapper records — evidence of a human, not a prompt

A slash command, local command, or IDE context injection
(`markers::BUILTIN_HUMAN`: `<command-`, `<local-command-`, `<ide_opened_file>`,
`<ide_selection>`) is never itself returned as the prompt, but if one precedes
the real prompt it sets `FirstPrompt.human_marked = true` — evidence a person
was at the keyboard even though the *returned text* is the next real prompt:

```json
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"<ide_selection>The user selected lines 1 to 2"}]}}
{"type":"user","message":{"role":"user","content":"the actual typed prompt"}}
```

`first_prompt("fixtures/human_ide_marked.jsonl")` on that pair returns
`FirstPrompt { text: "the actual typed prompt", human_marked: true }`.

## Fixture index

| File | Shape | Represents |
|---|---|---|
| `machine_ecc_observer.jsonl` | `queue-operation` | Spawner family A opening (>120 chars, all five `PREFIX_LENS` fingerprint) |
| `machine_ecc_summary.jsonl` | `queue-operation` | Spawner family B opening, distinct text |
| `human_quotes_machine_phrase.jsonl` | `user` string | The spawner phrase appears mid-prompt, not at the start — must stay visible |
| `human_ide_marked.jsonl` | wrapper + `user` string | `<ide_selection>` precedes the real prompt — never clustered |
| `human_repeated_opening.jsonl` | `user` string | A short human opening — the known blind spot: it *will* cluster like a machine family |
| `human_array_content.jsonl` | `user` array | Prompt as content blocks, not a string |
| `machine_sha_worktree.jsonl` | `queue-operation` | A machine opening; replayed under a SHA-named cwd to prove verdict follows the prompt, not the folder shape |
| `shared_cwd_machine.jsonl` / `shared_cwd_human.jsonl` | mixed | One cwd, two sessions, opposite verdicts |

Every fixture resolves via a real `first_prompt(path)` call in
`corpus_replay.rs` — none of these shapes are asserted by inspection alone.
