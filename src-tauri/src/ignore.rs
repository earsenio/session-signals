//! Session ignore rules — hide non-interactive / machine-spawned Claude Code
//! sessions from the widget and the tray rollup.
//!
//! Some tools launch headless `claude --print` agents that are *not* Claude
//! `Task` subagents (they carry a real UUID `session_id` and **no** `agent_id`),
//! so the engine would otherwise track them as ordinary primary sessions. The
//! observed shape (ECC's "homunculus" observer) is:
//!   - **cwd** under a spawner scratch dir, e.g.
//!     `…\.local\share\ecc-homunculus\projects\b4807c9eabf7` (hex leaf), and
//!   - a **first prompt** that starts with ECC's injected note
//!     *"IMPORTANT: You are running in non-interactive --print mode…"*.
//!
//! Rules are **data-driven** (persisted in config), so a new spawner shape — or
//! a change to Claude's/ECC's structure — can be filtered by editing config, no
//! rebuild required. Three matcher kinds are supported (the letters map to the
//! alternatives discussed in review):
//!   - `cwd_contains` (A, precise) — a substring of the cwd path.
//!   - `folder_hex` (A, general) — the cwd basename is all-hex and ≥ N chars.
//!   - `first_prompt_prefix` (B) — the session's *first* prompt starts with a
//!     known note. Anchored, so an ordinary session that merely mentions the
//!     phrase later is never hidden.
//!
//! cwd matchers are evaluated on every hook event (the cwd is always present, no
//! file read). The first-prompt matcher needs one bounded transcript head-read,
//! done off the engine lock and only when the cwd matchers didn't already hide
//! the session — so the common case pays no I/O.

use serde::{Deserialize, Serialize};

/// One ignore matcher. Serde-tagged so the persisted form is self-describing,
/// e.g. `{ "kind": "cwd_contains", "value": "ecc-homunculus" }`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Matcher {
    /// (A, precise) Hide when the session cwd contains this substring
    /// (case-insensitive). Catches spawner scratch dirs like `ecc-homunculus`.
    CwdContains { value: String },
    /// (A, general) Hide when the cwd's final path component is all hex digits
    /// and at least `min_len` characters — the machine-generated project-dir
    /// shape (e.g. `b4807c9eabf7`). `min_len` guards against short real names
    /// that happen to be hex (`beef`, `cafe`).
    FolderHex { min_len: usize },
    /// (B) Hide when the session's **first** prompt, with leading whitespace
    /// ignored, starts with this prefix (case-insensitive). Matches ECC's
    /// injected non-interactive note. Anchored to the first prompt on purpose:
    /// an ordinary session that quotes the phrase mid-conversation is not hidden.
    FirstPromptPrefix { value: String },
}

/// A compiled set of ignore matchers. A session is hidden if **any** matcher
/// fires. Cheap to clone (just a `Vec`); the engine holds one and swaps it when
/// config changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IgnoreRules {
    matchers: Vec<Matcher>,
}

impl IgnoreRules {
    pub fn new(matchers: Vec<Matcher>) -> Self {
        IgnoreRules { matchers }
    }

    /// The shipped defaults: the two cwd rules (A) plus the first-prompt rule (B)
    /// for the currently-known headless spawner. Also the serde default for
    /// `Config::ignore_rules`, so an existing config with no `ignore_rules` key
    /// still gets sensible filtering.
    pub fn defaults() -> Vec<Matcher> {
        vec![
            Matcher::CwdContains {
                value: "ecc-homunculus".to_string(),
            },
            Matcher::FolderHex { min_len: 12 },
            Matcher::FirstPromptPrefix {
                value: "IMPORTANT: You are running in non-interactive".to_string(),
            },
        ]
    }

    /// Cwd-only verdict (A). Available on every hook event — no file read.
    pub fn cwd_hidden(&self, cwd: &str) -> bool {
        self.matchers.iter().any(|m| match m {
            Matcher::CwdContains { value } => contains_ci(cwd, value),
            Matcher::FolderHex { min_len } => folder_is_hex(cwd, *min_len),
            Matcher::FirstPromptPrefix { .. } => false,
        })
    }

    /// First-prompt verdict (B). The caller supplies the session's first prompt
    /// (read once from the transcript head).
    pub fn prompt_hidden(&self, first_prompt: &str) -> bool {
        let p = first_prompt.trim_start();
        self.matchers.iter().any(|m| match m {
            Matcher::FirstPromptPrefix { value } => starts_with_ci(p, value.trim_start()),
            _ => false,
        })
    }

    /// Whether any first-prompt rule exists. Lets the caller skip the transcript
    /// head-read entirely when no B-rule could match.
    pub fn has_prompt_rules(&self) -> bool {
        self.matchers
            .iter()
            .any(|m| matches!(m, Matcher::FirstPromptPrefix { .. }))
    }
}

/// Case-insensitive substring test (ASCII-lowercased; paths/notes are ASCII).
fn contains_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

/// Case-insensitive prefix test.
fn starts_with_ci(s: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return false;
    }
    s.to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
}

/// True when the final path component of `cwd` is non-empty, all hex digits, and
/// at least `min_len` characters. Handles both `/` and `\` separators and a
/// trailing separator.
fn folder_is_hex(cwd: &str, min_len: usize) -> bool {
    let trimmed = cwd.trim_end_matches(['/', '\\']);
    let base = trimmed.rsplit(['/', '\\']).next().unwrap_or("");
    !base.is_empty() && base.len() >= min_len.max(1) && base.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> IgnoreRules {
        IgnoreRules::new(IgnoreRules::defaults())
    }

    #[test]
    fn cwd_contains_matches_spawner_dir_case_insensitively() {
        let r = rules();
        assert!(r.cwd_hidden(r"C:\Users\me\.local\share\ECC-Homunculus\projects\b4807c9eabf7"));
        assert!(r.cwd_hidden("/home/me/.local/share/ecc-homunculus/projects/abcdef012345"));
        // An ordinary repo path is not hidden by the substring rule.
        assert!(!IgnoreRules::new(vec![Matcher::CwdContains {
            value: "ecc-homunculus".into()
        }])
        .cwd_hidden(r"C:\Users\me\Codes\session-signals"));
    }

    #[test]
    fn folder_hex_matches_only_long_all_hex_basenames() {
        let r = IgnoreRules::new(vec![Matcher::FolderHex { min_len: 12 }]);
        assert!(r.cwd_hidden(r"C:\x\b4807c9eabf7")); // 12 hex
        assert!(r.cwd_hidden("/x/abcdef012345/")); // trailing slash tolerated
        assert!(!r.cwd_hidden(r"C:\x\session-signals")); // hyphen → not hex
        assert!(!r.cwd_hidden("/x/beef")); // hex but too short
        assert!(!r.cwd_hidden("/x/Codes")); // letters outside hex
    }

    #[test]
    fn empty_min_len_still_needs_nonempty_basename() {
        // A degenerate rule must not hide every session (empty basename guard).
        let r = IgnoreRules::new(vec![Matcher::FolderHex { min_len: 0 }]);
        assert!(!r.cwd_hidden("/some/path/")); // basename "path" isn't hex
        assert!(!r.cwd_hidden("/")); // no basename
    }

    #[test]
    fn first_prompt_prefix_is_anchored() {
        let r = rules();
        // The headless note as the first prompt → hidden.
        assert!(r.prompt_hidden(
            "IMPORTANT: You are running in non-interactive --print mode. You MUST use the Write tool"
        ));
        // Leading whitespace ignored.
        assert!(r.prompt_hidden("\n  IMPORTANT: You are running in non-interactive --print mode"));
        // The SAME phrase not at the start (e.g. this very session quoting it)
        // must NOT hide the session.
        assert!(!r.prompt_hidden(
            "Please check why IMPORTANT: You are running in non-interactive appears in my logs"
        ));
        // cwd_hidden never fires on a prompt-only rule.
        assert!(!IgnoreRules::new(vec![Matcher::FirstPromptPrefix {
            value: "IMPORTANT".into()
        }])
        .cwd_hidden("IMPORTANT"));
    }

    #[test]
    fn has_prompt_rules_reflects_presence() {
        assert!(rules().has_prompt_rules());
        assert!(!IgnoreRules::new(vec![Matcher::FolderHex { min_len: 12 }]).has_prompt_rules());
        assert!(!IgnoreRules::default().has_prompt_rules());
    }

    #[test]
    fn empty_rules_hide_nothing() {
        let r = IgnoreRules::default();
        assert!(!r.cwd_hidden(r"C:\x\b4807c9eabf7"));
        assert!(!r.prompt_hidden("IMPORTANT: You are running in non-interactive"));
    }
}
