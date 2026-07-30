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

/// A session's opening prompt plus whether Claude Code's own *human*
/// interaction markers preceded it in the transcript.
#[derive(Debug, PartialEq, Eq)]
pub struct FirstPrompt {
    pub text: String,
    /// True when a wrapper record (slash command, IDE injection) appeared
    /// before `text`. The session's true opening was a human at the keyboard,
    /// so `text` is that human's typed prompt — never a spawner's injection.
    pub human_marked: bool,
}

/// Read a transcript's **head** and return the session's *first* prompt — the
/// earliest queued/typed instruction — or `None` if unresolved. Used by the
/// first-prompt ignore rule to recognize headless `--print` sessions by their
/// injected opening note, and by observation to skip sessions a human clearly
/// opened. Deliberately anchored to the first record so an ordinary session
/// that merely quotes the phrase later is never matched.
pub fn first_prompt(transcript_path: &str) -> Option<FirstPrompt> {
    let mut file = File::open(transcript_path).ok()?;
    let mut buf = vec![0u8; MAX_HEAD_BYTES];
    let n = file.read(&mut buf).ok()?;
    buf.truncate(n);
    let text = String::from_utf8_lossy(&buf);
    first_prompt_from_str(&text)
}

/// Pure core of [`first_prompt`]: return the content of the earliest record that
/// is a queued instruction (`queue-operation`/`enqueue`) or a genuine human
/// `user` prompt, skipping slash-command/hook wrappers — but remembering
/// whether one was skipped along the way, since that skip is itself evidence
/// the session's true opening was a human at the keyboard. File-free for
/// testing.
fn first_prompt_from_str(text: &str) -> Option<FirstPrompt> {
    let mut saw_marker = false;
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
                        if is_wrapper(c) {
                            saw_marker = true;
                        } else {
                            return Some(FirstPrompt {
                                text: c.to_string(),
                                human_marked: saw_marker,
                            });
                        }
                    }
                }
            }
            // Or the first genuinely human-typed prompt.
            Some("user") => match classify_user_prompt(&v) {
                UserPromptKind::Text(t) => {
                    return Some(FirstPrompt {
                        text: t,
                        human_marked: saw_marker,
                    });
                }
                UserPromptKind::Wrapper => saw_marker = true,
                UserPromptKind::Skip => {}
            },
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

/// Claude Code's own structural markers, which open a prompt when the *user*
/// interacted rather than typed a task: a slash command, a local command, or an
/// IDE context injection (opening a file / selecting text).
///
/// These are evidence of a **human** at the keyboard, so they are never a
/// descriptor and never a session-ignore pattern. Sourced from
/// `markers::BUILTIN_HUMAN` so the four markers exist in exactly one place.
/// Deliberately a pure function over the built-in list only — no config
/// access — so this parser never depends on user state.
fn is_wrapper(s: &str) -> bool {
    let t = s.trim_start();
    crate::markers::BUILTIN_HUMAN
        .iter()
        .any(|p| t.starts_with(p))
}

/// The three outcomes of classifying a `type=="user"` transcript entry.
/// Separated from a plain `Option<String>` because [`first_prompt_from_str`]
/// needs to distinguish "wrapper, evidence of a human" from "not a prompt at
/// all" — a distinction `human_prompt` (used only for display) doesn't need.
enum UserPromptKind {
    /// A genuine human-typed prompt.
    Text(String),
    /// A slash-command/hook/IDE-injection wrapper — not a typed task, but
    /// still evidence a human was driving.
    Wrapper,
    /// Tool result, sidechain/meta entry, or non-text content: not a prompt.
    Skip,
}

/// Classify a `type=="user"` entry. `message.content` is either a plain string
/// **or an array of content blocks** — the array form is common (empirically
/// the majority of sessions) and was previously dropped, so those prompts were
/// invisible to both the descriptor fallback and the session-ignore rules.
/// Text blocks are concatenated; arrays carrying only `tool_result` (or other
/// non-text) blocks still yield `Skip`.
fn classify_user_prompt(v: &serde_json::Value) -> UserPromptKind {
    if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
        return UserPromptKind::Skip;
    }
    if v.get("isMeta").and_then(|b| b.as_bool()) == Some(true) {
        return UserPromptKind::Skip;
    }
    let Some(content) = v.get("message").and_then(|m| m.get("content")) else {
        return UserPromptKind::Skip;
    };
    let text = match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => {
            let joined = blocks
                .iter()
                .filter(|b| b.get("type").and_then(serde_json::Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(serde_json::Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            if joined.trim().is_empty() {
                return UserPromptKind::Skip;
            }
            joined
        }
        _ => return UserPromptKind::Skip,
    };
    if is_wrapper(&text) {
        UserPromptKind::Wrapper
    } else {
        UserPromptKind::Text(text)
    }
}

/// Extract a genuinely human-typed prompt string from a `type=="user"` entry, or
/// `None` if it's a tool result, a sidechain/meta entry, or a slash-command/hook
/// wrapper rather than something the user actually typed. Used by the display
/// descriptor ([`extract_from_str`]), which has no need for the wrapper/skip
/// distinction — see [`classify_user_prompt`] for that.
fn human_prompt(v: &serde_json::Value) -> Option<String> {
    match classify_user_prompt(v) {
        UserPromptKind::Text(t) => Some(t),
        UserPromptKind::Wrapper | UserPromptKind::Skip => None,
    }
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
        let fp = first_prompt_from_str(t).unwrap();
        assert_eq!(
            fp.text,
            "IMPORTANT: You are running in non-interactive --print mode. Do the thing."
        );
        assert!(!fp.human_marked, "no wrapper preceded the enqueue note");
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
        let fp = first_prompt_from_str(t).unwrap();
        assert_eq!(fp.text, "the real first prompt");
        // The skipped slash-command wrapper is evidence a human was present.
        assert!(fp.human_marked);
    }

    /// Prompts arrive as an **array of content blocks** at least as often as a
    /// plain string. The string-only assumption made those sessions invisible to
    /// the descriptor fallback and to the session-ignore rules.
    #[test]
    fn array_content_prompts_are_read() {
        let t = r#"
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"please audit the listener"}]}}
"#;
        let fp = first_prompt_from_str(t).unwrap();
        assert_eq!(fp.text, "please audit the listener");
        assert!(!fp.human_marked);
        // Multiple text blocks are joined in order.
        let t2 = r#"
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"first"},{"type":"text","text":"second"}]}}
"#;
        assert_eq!(first_prompt_from_str(t2).unwrap().text, "first\nsecond");
        // A tool-result-only array is still not a prompt, and (unlike a wrapper)
        // is not evidence of a human either.
        let t3 = r#"
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"x"}]}}
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"the real prompt"}]}}
"#;
        let fp3 = first_prompt_from_str(t3).unwrap();
        assert_eq!(fp3.text, "the real prompt");
        assert!(!fp3.human_marked, "a tool-result skip is not a marker");
    }

    /// The substantive finding this task fixes: the in-app reader *skips*
    /// wrapper records and returns the next real prompt, so a marker check on
    /// the returned text alone could never fire. `human_marked` carries that
    /// fact forward instead.
    #[test]
    fn human_marker_before_prompt_is_flagged() {
        let t = r#"
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"<ide_selection>The user selected lines 1 to 2"}]}}
{"type":"user","message":{"role":"user","content":"please review the listener"}}
"#;
        let fp = first_prompt_from_str(t).unwrap();
        assert_eq!(fp.text, "please review the listener");
        assert!(fp.human_marked);

        // The same prompt with no preceding marker is not flagged.
        let bare =
            r#"{"type":"user","message":{"role":"user","content":"please review the listener"}}"#;
        let fp = first_prompt_from_str(bare).unwrap();
        assert_eq!(fp.text, "please review the listener");
        assert!(!fp.human_marked);
    }

    /// `isMeta` and tool-result skips are not evidence of a human — only a
    /// wrapper record is. Conflating them would flag ordinary machine-spawned
    /// openings as human-marked and defeat the ignore rule they're meant for.
    #[test]
    fn meta_and_tool_results_do_not_flag_human() {
        let t = r#"
{"type":"user","isMeta":true,"message":{"role":"user","content":"meta noise"}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"x"}]}}
{"type":"user","message":{"role":"user","content":"the real prompt"}}
"#;
        let fp = first_prompt_from_str(t).unwrap();
        assert_eq!(fp.text, "the real prompt");
        assert!(!fp.human_marked);
    }

    /// IDE context injections are Claude Code's own markers for a *human*
    /// interaction (opening a file, selecting text) — never a typed task, and
    /// never a machine-spawn pattern.
    #[test]
    fn ide_markers_are_wrappers() {
        assert!(is_wrapper("<ide_opened_file>The user opened the file x.rs"));
        assert!(is_wrapper("<ide_selection>The user selected lines 1 to 2"));
        assert!(is_wrapper("<command-name>/compact</command-name>"));
        assert!(is_wrapper("<local-command-caveat>Caveat: …"));
        assert!(!is_wrapper("please fix the listener"));

        // ...so a session opening with one yields no first prompt (fail-open:
        // it stays visible rather than becoming an ignore candidate).
        let t = r#"
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"<ide_opened_file>The user opened the file c:\\x\\y.rs"}]}}
"#;
        assert_eq!(first_prompt_from_str(t), None);
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
