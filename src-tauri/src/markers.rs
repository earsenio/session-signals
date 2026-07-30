//! Marker registry: classify a prompt's opening as evidence of a *human* or a
//! *machine* interaction, so observation (see `observe.rs`) can skip openings
//! that should never be clustered into a filter proposal.
//!
//! Built-in Human markers are Claude Code's own structural records — a slash
//! command, a local command, an IDE context injection — and are immutable:
//! a user who could reclassify `<ide_selection>` would silently reintroduce a
//! known false-positive class (a human's IDE selection looking like a
//! repeatable machine pattern). Config `markers` are additive only, letting a
//! user teach the registry about a spawner's own opening note without a
//! rebuild — but they can never shadow a built-in.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarkerPolarity {
    Human,
    Machine,
}

/// Claude Code's own structural markers for a *human* interaction. Built in
/// and immutable: a user who could delete `<ide_selection>` would silently
/// reintroduce a known false-positive class (PRD decision). `descriptor::
/// is_wrapper` sources its list from here, so the four markers exist in
/// exactly one place. Matched case-sensitively, as `is_wrapper` always has.
pub const BUILTIN_HUMAN: &[&str] = &[
    "<command-",
    "<local-command-",
    "<ide_opened_file>",
    "<ide_selection>",
];

/// One user-configured marker rule, additive to `BUILTIN_HUMAN`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MarkerRule {
    pub prefix: String,
    pub polarity: MarkerPolarity,
}

/// A compiled marker registry: the immutable built-ins plus user-configured
/// additions.
pub struct Registry {
    extra: Vec<MarkerRule>,
}

impl Registry {
    /// Config markers are **additive only**. An entry colliding with a
    /// built-in prefix is dropped with a note — overriding `<ide_selection>`
    /// to Machine would be worse than deleting it.
    pub fn new(extra: Vec<MarkerRule>) -> Self {
        let mut out = Vec::with_capacity(extra.len());
        for rule in extra {
            if BUILTIN_HUMAN.contains(&rule.prefix.as_str()) {
                eprintln!(
                    "beacon: dropping config marker for built-in prefix {:?} — built-ins cannot be overridden",
                    rule.prefix
                );
                continue;
            }
            out.push(rule);
        }
        Registry { extra: out }
    }

    /// Classify `text`'s opening. Built-ins are checked first and can never be
    /// shadowed. `None` means unclassified — merely *eligible* for
    /// observation, neither forced nor blocked (PRD decision 4).
    pub fn polarity_of(&self, text: &str) -> Option<MarkerPolarity> {
        let t = text.trim_start();
        if BUILTIN_HUMAN.iter().any(|p| t.starts_with(p)) {
            return Some(MarkerPolarity::Human);
        }
        self.extra
            .iter()
            .find(|r| t.starts_with(r.prefix.as_str()))
            .map(|r| r.polarity)
    }

    /// Shorthand for `polarity_of(text) == Some(Human)`.
    pub fn is_human(&self, text: &str) -> bool {
        self.polarity_of(text) == Some(MarkerPolarity::Human)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_markers_classify_human() {
        let r = Registry::new(vec![]);
        assert!(r.is_human("<ide_selection>The user selected lines 1 to 2"));
        assert!(r.is_human("<command-name>/compact</command-name>"));
        assert!(r.is_human("<local-command-caveat>Caveat: …"));
        assert!(r.is_human("<ide_opened_file>The user opened x.rs"));
    }

    #[test]
    fn config_marker_classifies_machine() {
        let r = Registry::new(vec![MarkerRule {
            prefix: "<task-notification>".to_string(),
            polarity: MarkerPolarity::Machine,
        }]);
        assert_eq!(
            r.polarity_of("<task-notification>build finished"),
            Some(MarkerPolarity::Machine)
        );
    }

    /// Overriding a built-in is strictly worse than deleting it (which isn't
    /// possible either) — the config entry is dropped and the built-in Human
    /// verdict stands.
    #[test]
    fn builtin_markers_cannot_be_overridden() {
        let r = Registry::new(vec![MarkerRule {
            prefix: "<ide_selection>".to_string(),
            polarity: MarkerPolarity::Machine,
        }]);
        assert_eq!(
            r.polarity_of("<ide_selection>The user selected lines 1 to 2"),
            Some(MarkerPolarity::Human)
        );
    }

    /// Unclassified means *eligible*, not blocked — the worst case is one
    /// proposal the user reviews later.
    #[test]
    fn unclassified_marker_is_eligible() {
        let r = Registry::new(vec![]);
        assert_eq!(r.polarity_of("<task-notification>build finished"), None);
        assert_eq!(r.polarity_of("please fix the listener"), None);
    }
}
