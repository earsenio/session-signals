//! Derive a short, human-meaningful session descriptor from a Claude Code
//! transcript — **locally**, with no network or LLM call (the listener is
//! loopback-only and Session Signals never makes outbound calls; reading a file the user
//! already owns honors that).
//!
//! Source of truth, in priority order, all read from the session's transcript
//! JSONL (`transcript_path`, carried on every real hook event):
//!   1. `last-prompt` — Claude Code's record of the **most recent user prompt**,
//!      rewritten every turn. This tracks what the session is *currently* doing.
//!      We take the last (freshest) non-command-wrapper one. Preferred because
//!      `ai-title` (below) only regenerates occasionally and so lags real work.
//!   2. `ai-title` — Claude Code's generated session title (3–8 words). Used when
//!      there's no usable prompt yet. Take the last (freshest) one.
//!   3. The first human-typed `user` prompt (skipping tool-result and
//!      slash-command/hook-wrapper entries) — a brand-new-session fallback.
//!   4. Legacy `summary` (`.summary`) for older Claude Code schemas.
//!
//! We read only a bounded tail window — a multi-MB transcript is never scanned
//! end to end, keeping this cheap enough to run on the event worker. The
//! freshest `last-prompt`/`ai-title` are at the very end, so the tail captures
//! them; the first-prompt fallback only matters for tiny new-session files where
//! the tail covers the whole file anyway.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// Only ever read this many bytes from the tail of a transcript.
const MAX_TAIL_BYTES: u64 = 512 * 1024;
/// Only ever read this many bytes from the *head* of a transcript when resolving
/// the first prompt (the earliest records are at the start, so a small window is
/// plenty). Used by the first-prompt ignore rule (`ignore::Matcher`).
const MAX_HEAD_BYTES: usize = 64 * 1024;
/// Cap the descriptor length so a long fallback prompt can't blow out the row.
const MAX_LEN: usize = 80;

/// Read a transcript's tail and derive its descriptor, or `None` if the file is
/// missing/unreadable or yields nothing usable.
pub fn extract(transcript_path: &str) -> Option<String> {
    let mut file = File::open(transcript_path).ok()?;
    let len = file.metadata().ok()?.len();
    // Seek to the last MAX_TAIL_BYTES so we catch the freshest `ai-title`. The
    // first (partial) line after the seek just fails to parse and is skipped.
    let start = len.saturating_sub(MAX_TAIL_BYTES);
    if start > 0 {
        file.seek(SeekFrom::Start(start)).ok()?;
    }
    let mut buf = Vec::new();
    file.take(MAX_TAIL_BYTES).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    extract_from_str(&text)
}

/// Read a transcript's **head** and return the session's *first* prompt — the
/// earliest queued/typed instruction — or `None` if unresolved. Used by the
/// first-prompt ignore rule to recognize headless `--print` sessions by their
/// injected opening note. Deliberately anchored to the first record so an
/// ordinary session that merely quotes the phrase later is never matched.
pub fn first_prompt(transcript_path: &str) -> Option<String> {
    let mut file = File::open(transcript_path).ok()?;
    let mut buf = vec![0u8; MAX_HEAD_BYTES];
    let n = file.read(&mut buf).ok()?;
    buf.truncate(n);
    let text = String::from_utf8_lossy(&buf);
    first_prompt_from_str(&text)
}

/// Pure core of [`first_prompt`]: return the content of the earliest record that
/// is a queued instruction (`queue-operation`/`enqueue`) or a genuine human
/// `user` prompt, skipping slash-command/hook wrappers. File-free for testing.
fn first_prompt_from_str(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match v.get("type").and_then(|t| t.as_str()) {
            // The queued initial instruction (how headless runs are seeded).
            Some("queue-operation") => {
                if v.get("operation").and_then(|o| o.as_str()) == Some("enqueue") {
                    if let Some(c) = v.get("content").and_then(|c| c.as_str()) {
                        if !is_wrapper(c) {
                            return Some(c.to_string());
                        }
                    }
                }
            }
            // Or the first genuinely human-typed prompt.
            Some("user") => {
                if let Some(p) = human_prompt(&v) {
                    return Some(p);
                }
            }
            _ => {}
        }
    }
    None
}

/// The pure parsing core (transcript text in, descriptor out) — file-free so it
/// can be unit-tested directly.
fn extract_from_str(text: &str) -> Option<String> {
    let mut last_prompt: Option<String> = None;
    let mut last_ai_title: Option<String> = None;
    let mut first_prompt: Option<String> = None;
    let mut first_summary: Option<String> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // partial trailing line or non-JSON noise
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("last-prompt") => {
                // Freshest real prompt wins; skip command/hook wrappers (e.g. a
                // trailing `/compact`) so the row keeps showing the last typed task.
                if let Some(p) = v.get("lastPrompt").and_then(|p| p.as_str()) {
                    if !is_wrapper(p) {
                        last_prompt = Some(p.to_string());
                    }
                }
            }
            Some("ai-title") => {
                if let Some(t) = v.get("aiTitle").and_then(|t| t.as_str()) {
                    last_ai_title = Some(t.to_string());
                }
            }
            Some("summary") => {
                if first_summary.is_none() {
                    if let Some(s) = v.get("summary").and_then(|s| s.as_str()) {
                        first_summary = Some(s.to_string());
                    }
                }
            }
            Some("user") if first_prompt.is_none() => {
                if let Some(p) = human_prompt(&v) {
                    first_prompt = Some(p);
                }
            }
            _ => {}
        }
    }

    // Latest prompt (current work) → freshest title → first prompt → legacy summary.
    last_prompt
        .or(last_ai_title)
        .or(first_prompt)
        .or(first_summary)
        .and_then(|s| clean(&s))
}

/// True for a slash-command / local-command wrapper string (not a typed task).
fn is_wrapper(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with("<command-") || t.starts_with("<local-command-")
}

/// Extract a genuinely human-typed prompt string from a `type=="user"` entry, or
/// `None` if it's a tool result, a sidechain/meta entry, or a slash-command/hook
/// wrapper rather than something the user actually typed.
fn human_prompt(v: &serde_json::Value) -> Option<String> {
    if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
        return None;
    }
    if v.get("isMeta").and_then(|b| b.as_bool()) == Some(true) {
        return None;
    }
    // Real typed prompts have a *string* content; tool results are arrays.
    let content = v.get("message")?.get("content")?.as_str()?;
    if is_wrapper(content) {
        return None;
    }
    Some(content.to_string())
}

/// Collapse internal whitespace/newlines to single spaces, trim, and truncate to
/// `MAX_LEN` chars (char-boundary safe) with an ellipsis. `None` if empty.
fn clean(s: &str) -> Option<String> {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    if collapsed.chars().count() > MAX_LEN {
        let mut out: String = collapsed.chars().take(MAX_LEN - 1).collect();
        out.push('…');
        Some(out)
    } else {
        Some(collapsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_prompt_wins_over_title() {
        // The whole point of the fix: a fresh prompt beats the (stale) title.
        let t = r#"
{"type":"ai-title","aiTitle":"Debug background opacity blinking issue"}
{"type":"last-prompt","lastPrompt":"please reconcile the section and commit"}
"#;
        assert_eq!(
            extract_from_str(t).as_deref(),
            Some("please reconcile the section and commit")
        );
    }

    #[test]
    fn latest_prompt_takes_freshest_non_wrapper() {
        // Multiple last-prompt records; the freshest real one wins, and a trailing
        // slash-command wrapper is skipped (we keep showing the last typed task).
        let t = r#"
{"type":"last-prompt","lastPrompt":"first task"}
{"type":"last-prompt","lastPrompt":"the current task"}
{"type":"last-prompt","lastPrompt":"<command-name>/compact</command-name>"}
"#;
        assert_eq!(extract_from_str(t).as_deref(), Some("the current task"));
    }

    #[test]
    fn prefers_last_ai_title_when_no_prompt() {
        // No last-prompt yet → fall back to the freshest ai-title.
        let t = r#"
{"type":"mode","mode":"default"}
{"type":"ai-title","aiTitle":"Initial title"}
{"type":"ai-title","aiTitle":"Refined session title"}
"#;
        assert_eq!(
            extract_from_str(t).as_deref(),
            Some("Refined session title")
        );
    }

    #[test]
    fn falls_back_to_first_human_prompt() {
        // No ai-title yet (brand-new session). Skip tool-result arrays and the
        // slash-command wrapper; take the first real typed prompt.
        let t = r#"
{"type":"user","isSidechain":true,"message":{"role":"user","content":"subagent noise"}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"x"}]}}
{"type":"user","message":{"role":"user","content":"<command-name>/compact</command-name>"}}
{"type":"user","message":{"role":"user","content":"add a session descriptor"}}
{"type":"user","message":{"role":"user","content":"a later prompt"}}
"#;
        assert_eq!(
            extract_from_str(t).as_deref(),
            Some("add a session descriptor")
        );
    }

    #[test]
    fn legacy_summary_fallback() {
        let t = r#"
{"type":"summary","summary":"Old-schema session summary"}
{"type":"assistant","message":{"role":"assistant","content":"hi"}}
"#;
        assert_eq!(
            extract_from_str(t).as_deref(),
            Some("Old-schema session summary")
        );
    }

    #[test]
    fn ai_title_wins_over_prompt_and_summary() {
        let t = r#"
{"type":"summary","summary":"sum"}
{"type":"user","message":{"role":"user","content":"first prompt"}}
{"type":"ai-title","aiTitle":"The Title"}
"#;
        assert_eq!(extract_from_str(t).as_deref(), Some("The Title"));
    }

    #[test]
    fn collapses_and_truncates() {
        let long = "word ".repeat(40); // 200 chars, many spaces
        let line = format!(
            "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"{}\"}}}}",
            long.trim()
        );
        let out = extract_from_str(&line).unwrap();
        assert!(out.chars().count() <= MAX_LEN, "truncated to cap");
        assert!(out.ends_with('…'));
        assert!(!out.contains("  "), "internal whitespace collapsed");
    }

    #[test]
    fn extract_reads_a_real_file() {
        // Exercises the file-read/seek path end to end (small file → tail is the
        // whole file, seek start = 0).
        let path =
            std::env::temp_dir().join(format!("beacon_desc_test_{}.jsonl", std::process::id()));
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n\
             {\"type\":\"ai-title\",\"aiTitle\":\"Real File Title\"}\n",
        )
        .unwrap();
        assert_eq!(
            extract(path.to_str().unwrap()).as_deref(),
            Some("Real File Title")
        );
        let _ = std::fs::remove_file(&path);
        // A missing path is a clean None, never a panic.
        assert_eq!(extract("/no/such/transcript.jsonl"), None);
    }

    #[test]
    fn first_prompt_takes_earliest_enqueue() {
        // A headless run: the earliest record is the queued non-interactive note.
        let t = r#"
{"type":"queue-operation","operation":"enqueue","content":"IMPORTANT: You are running in non-interactive --print mode. Do the thing."}
{"type":"user","message":{"role":"user","content":"a later message"}}
"#;
        assert_eq!(
            first_prompt_from_str(t).as_deref(),
            Some("IMPORTANT: You are running in non-interactive --print mode. Do the thing.")
        );
    }

    #[test]
    fn first_prompt_falls_back_to_first_human_user() {
        // No enqueue record: take the first real human prompt, skipping sidechain
        // noise, tool-result arrays, and slash-command wrappers.
        let t = r#"
{"type":"user","isSidechain":true,"message":{"role":"user","content":"subagent noise"}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"x"}]}}
{"type":"user","message":{"role":"user","content":"<command-name>/compact</command-name>"}}
{"type":"user","message":{"role":"user","content":"the real first prompt"}}
"#;
        assert_eq!(
            first_prompt_from_str(t).as_deref(),
            Some("the real first prompt")
        );
    }

    #[test]
    fn first_prompt_none_when_no_prompt() {
        assert_eq!(first_prompt_from_str(""), None);
        assert_eq!(
            first_prompt_from_str("{\"type\":\"assistant\"}\nnot json"),
            None
        );
    }

    #[test]
    fn empty_or_garbage_yields_none() {
        assert_eq!(extract_from_str(""), None);
        assert_eq!(extract_from_str("not json\n{partial"), None);
        // A user entry that's only a command wrapper is not a descriptor.
        let t = r#"{"type":"user","message":{"role":"user","content":"<command-name>/x</command-name>"}}"#;
        assert_eq!(extract_from_str(t), None);
    }
}
