//! Derive a short, human-meaningful session descriptor from a Claude Code
//! transcript — **locally**, with no network or LLM call (the listener is
//! loopback-only and Beacon never makes outbound calls; reading a file the user
//! already owns honors that).
//!
//! Source of truth, in priority order, all read from the session's transcript
//! JSONL (`transcript_path`, carried on every real hook event):
//!   1. `ai-title` — Claude Code's own generated session title (3–8 words). The
//!      file carries many as it regenerates over the session's life; we take the
//!      **last** one (freshest), which is why we read the file's *tail*.
//!   2. The first human-typed `user` prompt (skipping tool-result and
//!      slash-command/hook-wrapper entries) — the brand-new-session fallback
//!      before any title exists; such files are short, so the tail is the whole
//!      file and "first" is genuinely first.
//!   3. Legacy `summary` (`.summary`) for older Claude Code schemas.
//!
//! We read only a bounded tail window — a multi-MB transcript is never scanned
//! end to end, keeping this cheap enough to run on the event worker. A session
//! large enough to exceed the window always has `ai-title` records in its tail
//! (titles regenerate per turn), so the prompt fallback only matters for small
//! files where the tail covers everything.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// Only ever read this many bytes from the tail of a transcript.
const MAX_TAIL_BYTES: u64 = 512 * 1024;
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

/// The pure parsing core (transcript text in, descriptor out) — file-free so it
/// can be unit-tested directly.
fn extract_from_str(text: &str) -> Option<String> {
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

    // ai-title (freshest) → first human prompt → legacy summary.
    last_ai_title
        .or(first_prompt)
        .or(first_summary)
        .and_then(|s| clean(&s))
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
    let trimmed = content.trim_start();
    if trimmed.starts_with("<command-") || trimmed.starts_with("<local-command-") {
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
    fn prefers_last_ai_title() {
        let t = r#"
{"type":"mode","mode":"default"}
{"type":"ai-title","aiTitle":"Initial title"}
{"type":"user","message":{"role":"user","content":"do the thing"}}
{"type":"ai-title","aiTitle":"Refined session title"}
"#;
        assert_eq!(extract_from_str(t).as_deref(), Some("Refined session title"));
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
        assert_eq!(extract_from_str(t).as_deref(), Some("add a session descriptor"));
    }

    #[test]
    fn legacy_summary_fallback() {
        let t = r#"
{"type":"summary","summary":"Old-schema session summary"}
{"type":"assistant","message":{"role":"assistant","content":"hi"}}
"#;
        assert_eq!(extract_from_str(t).as_deref(), Some("Old-schema session summary"));
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
        let path = std::env::temp_dir()
            .join(format!("beacon_desc_test_{}.jsonl", std::process::id()));
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n\
             {\"type\":\"ai-title\",\"aiTitle\":\"Real File Title\"}\n",
        )
        .unwrap();
        assert_eq!(extract(path.to_str().unwrap()).as_deref(), Some("Real File Title"));
        let _ = std::fs::remove_file(&path);
        // A missing path is a clean None, never a panic.
        assert_eq!(extract("/no/such/transcript.jsonl"), None);
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
