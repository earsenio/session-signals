//! Session ignore rules — hide non-interactive / machine-spawned Claude Code
//! sessions from the widget and the tray rollup.
//!
//! Third-party tooling launches headless `claude --print` agents that are *not*
//! Claude `Task` subagents — they carry a real UUID `session_id` and **no**
//! `agent_id` — so the engine would otherwise track them as ordinary primary
//! sessions, cluttering the widget and colouring the tray.
//!
//! **Ships empty.** Nothing is hidden until the user opts in: a session that
//! silently disappears is this app's worst failure mode, and any pattern we could
//! ship would name one specific third-party tool. Known spawner patterns are
//! documented as a copy-paste recipe in `docs/IGNORE-RULES.md` instead.
//!
//! Two matcher kinds, both data-driven (persisted in config), so a new spawner —
//! or a change to an existing one — is handled by editing config, no rebuild:
//!   - `cwd_contains` — a substring of the cwd path. Convenient when a spawner
//!     uses a dedicated scratch directory, but **cannot** be relied on in
//!     general: machine and human sessions routinely share one cwd.
//!   - `first_prompt_prefix` — the session's *first* prompt starts with a known
//!     opening. Anchored to the first prompt, so an ordinary session that merely
//!     quotes the phrase later is never hidden. This is the load-bearing one.
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
    /// Hide when the session cwd contains this substring (case-insensitive).
    /// Catches spawner scratch dirs. Note a cwd rule can only ever separate
    /// spawners that use a dedicated directory — machine and human sessions
    /// frequently share one cwd, so this is a convenience, not the mechanism.
    CwdContains { value: String },
    /// Hide when the session's **first** prompt, with leading whitespace
    /// ignored, starts with this prefix (case-insensitive). Anchored to the
    /// first prompt on purpose: an ordinary session that quotes the phrase
    /// mid-conversation is not hidden. This is the load-bearing matcher — a
    /// spawner's injected opening is the one thing that reliably identifies it.
    FirstPromptPrefix { value: String },
}

/// Deserialize a matcher list, **skipping entries we don't recognise** instead of
/// failing the whole parse. Without this, one stale entry (e.g. a `folder_hex`
/// rule written by an older build) would abort `Config` deserialization and
/// silently reset *every* unrelated setting — port, theme, notifications — back
/// to defaults. Unknown kinds are dropped with a one-line note.
pub fn deserialize_lenient<'de, D>(d: D) -> Result<Vec<Matcher>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Vec::<serde_json::Value>::deserialize(d)?;
    let mut out = Vec::with_capacity(raw.len());
    for v in raw {
        match serde_json::from_value::<Matcher>(v.clone()) {
            Ok(m) => out.push(m),
            Err(_) => eprintln!("beacon: dropping unrecognized ignore rule: {v}"),
        }
    }
    Ok(out)
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

    /// **Empty.** Session Signals ships hiding *nothing*.
    ///
    /// Filtering is opt-in because a silently-vanished session is this app's
    /// worst failure mode, and because any pattern we could ship would name a
    /// specific third-party tool — every user would then carry filters for
    /// software they've never installed. Known spawner patterns are documented
    /// as a copy-paste recipe instead (see `docs/IGNORE-RULES.md`).
    pub fn defaults() -> Vec<Matcher> {
        Vec::new()
    }

    /// Cwd-only match. Available on every hook event — no file read. Named
    /// for the deny path; see `cwd_hidden` (kept as an alias) and `matches`
    /// (the allowlist-shaped call the `never_hide` guard uses).
    pub fn matches_cwd(&self, cwd: &str) -> bool {
        self.matchers.iter().any(|m| match m {
            Matcher::CwdContains { value } => contains_ci(cwd, value),
            Matcher::FirstPromptPrefix { .. } => false,
        })
    }

    /// Cwd-only verdict. Available on every hook event — no file read.
    #[inline]
    pub fn cwd_hidden(&self, cwd: &str) -> bool {
        self.matches_cwd(cwd)
    }

    /// First-prompt match. The caller supplies the session's first prompt
    /// (read once from the transcript head). Named for the deny path; see
    /// `prompt_hidden` (kept as an alias).
    pub fn matches_prompt(&self, first_prompt: &str) -> bool {
        let p = first_prompt.trim_start();
        self.matchers.iter().any(|m| match m {
            Matcher::FirstPromptPrefix { value } => starts_with_ci(p, value.trim_start()),
            _ => false,
        })
    }

    /// First-prompt verdict (B). The caller supplies the session's first prompt
    /// (read once from the transcript head).
    #[inline]
    pub fn prompt_hidden(&self, first_prompt: &str) -> bool {
        self.matches_prompt(first_prompt)
    }

    /// Whether this rule set matches a session, by cwd alone, prompt alone,
    /// or both. Used by the `never_hide` allowlist, which needs an
    /// affirmative "does this apply" call rather than the deny-path's
    /// `cwd_hidden`/`prompt_hidden` naming.
    pub fn matches(&self, cwd: &str, first_prompt: Option<&str>) -> bool {
        self.matches_cwd(cwd) || first_prompt.is_some_and(|p| self.matches_prompt(p))
    }

    /// Whether any first-prompt rule exists. Lets the caller skip the transcript
    /// head-read entirely when no B-rule could match.
    pub fn has_prompt_rules(&self) -> bool {
        self.matchers
            .iter()
            .any(|m| matches!(m, Matcher::FirstPromptPrefix { .. }))
    }

    /// The "what would happen if…" constructor: a new rule set with `extra`
    /// appended, leaving `self` untouched. Used to build a candidate rule set
    /// for a proposal preview without ever mutating the engine's live rules.
    pub fn with(&self, extra: Matcher) -> IgnoreRules {
        let mut matchers = self.matchers.clone();
        matchers.push(extra);
        IgnoreRules { matchers }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The ECC recipe from `docs/IGNORE-RULES.md`. Deliberately built here rather
    /// than shipped in `defaults()` — see `empty_defaults_hide_nothing`.
    fn rules() -> IgnoreRules {
        IgnoreRules::new(vec![
            Matcher::CwdContains {
                value: "ecc-homunculus".to_string(),
            },
            Matcher::FirstPromptPrefix {
                value: "IMPORTANT: You are running in non-interactive".to_string(),
            },
        ])
    }

    /// Nothing is hidden out of the box. A shipped pattern would name a specific
    /// third-party tool and silently hide sessions for users who don't run it.
    #[test]
    fn empty_defaults_hide_nothing() {
        assert!(IgnoreRules::defaults().is_empty());
        let r = IgnoreRules::new(IgnoreRules::defaults());
        assert!(!r.cwd_hidden(r"C:\x\.local\share\ecc-homunculus\projects\b4807c9eabf7"));
        assert!(!r.prompt_hidden("IMPORTANT: You are running in non-interactive --print mode"));
        assert!(!r.has_prompt_rules());
    }

    /// A rule kind we no longer understand (e.g. `folder_hex`, removed because it
    /// added zero coverage over `cwd_contains` while risking SHA-named worktrees)
    /// must be dropped, NOT abort the parse — otherwise one stale entry resets
    /// every unrelated setting to defaults.
    #[test]
    fn unknown_rule_kinds_are_dropped_not_fatal() {
        let json = r#"[
            { "kind": "folder_hex", "min_len": 12 },
            { "kind": "cwd_contains", "value": "ecc-homunculus" },
            { "kind": "totally_made_up", "x": 1 }
        ]"#;
        let mut de = serde_json::Deserializer::from_str(json);
        let parsed = deserialize_lenient(&mut de).expect("must not fail the parse");
        assert_eq!(
            parsed,
            vec![Matcher::CwdContains {
                value: "ecc-homunculus".to_string()
            }],
            "only the recognized rule survives"
        );
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
        assert!(
            !IgnoreRules::new(vec![Matcher::CwdContains { value: "x".into() }]).has_prompt_rules()
        );
        assert!(!IgnoreRules::default().has_prompt_rules());
    }

    #[test]
    fn matches_fires_on_cwd_alone_prompt_alone_or_neither() {
        let r = rules();
        assert!(
            r.matches(r"C:\Users\me\.local\share\ecc-homunculus\projects\x", None),
            "cwd alone"
        );
        assert!(
            r.matches(
                "/home/me/ordinary",
                Some("IMPORTANT: You are running in non-interactive --print mode")
            ),
            "prompt alone"
        );
        assert!(!r.matches("/home/me/ordinary", Some("please fix the listener")));
        assert!(!r.matches("/home/me/ordinary", None));
    }

    #[test]
    fn with_appends_without_mutating_self() {
        let base = IgnoreRules::new(vec![Matcher::CwdContains { value: "a".into() }]);
        let extended = base.with(Matcher::FirstPromptPrefix {
            value: "hello".into(),
        });
        assert!(!base.has_prompt_rules(), "self is untouched");
        assert!(extended.has_prompt_rules());
        assert!(extended.cwd_hidden("has an a in it"), "original rule kept");
    }

    #[test]
    fn empty_rules_hide_nothing() {
        let r = IgnoreRules::default();
        assert!(!r.cwd_hidden(r"C:\x\b4807c9eabf7"));
        assert!(!r.prompt_hidden("IMPORTANT: You are running in non-interactive"));
    }
}
