//! End-to-end replay of the fixture corpus (`fixtures/`, PRD Phase 6) through
//! the *real* pipeline: `descriptor::first_prompt` → marker guard →
//! `never_hide` → `Observations` → `engine::set_first_prompt`, then
//! `proposals::build` and `Engine::is_hidden` for the verdicts. See
//! `fixtures/README.md` for what each file represents and why.
//!
//! `Replay` reproduces `lib.rs`'s ingest order exactly (see
//! `maybe_refresh_hidden` / `observe_opening`) — a harness that reordered
//! marker-guard vs. `never_hide` vs. observe, or that classified before
//! observing, would pass while production failed. It drives a real
//! `beacon_lib::engine::Engine` rather than re-implementing `session_hidden`,
//! so `never_hide` precedence and the reveal-on-block valve are exercised for
//! real.
//!
//! Every fingerprint here uses one fixed test salt — fingerprints are
//! salted-per-install and non-comparable across salts, so no test may ever
//! assert against a literal hex fingerprint.

use beacon_lib::engine::{Engine, HookEvent};
use beacon_lib::ignore::{IgnoreRules, Matcher};
use beacon_lib::observe::Observations;
use beacon_lib::{descriptor, markers, proposals};
use std::time::Duration;

const SALT: &[u8] = b"fixed-test-salt-for-corpus-replay";

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Drives fixtures through the real pipeline for one in-memory "install":
/// one salt, one observation store, one set of live rules, one `Engine`.
struct Replay {
    obs: Observations,
    ignore: IgnoreRules,
    never_hide: IgnoreRules,
    markers: markers::Registry,
    engine: Engine,
}

impl Replay {
    fn new() -> Self {
        Replay {
            obs: Observations::default(),
            ignore: IgnoreRules::default(),
            never_hide: IgnoreRules::default(),
            markers: markers::Registry::new(vec![]),
            engine: Engine::new(Duration::from_secs(600), Duration::from_secs(3600)),
        }
    }

    /// One session: start it in the engine, then run the ingest pipeline
    /// against `fixture_name`'s transcript head, in production's order —
    /// 1. read → 2. observe (marker guard, then `never_hide`) →
    /// 3. `set_first_prompt`, always last and unconditional (lib.rs:344-352).
    ///
    /// Returns the resolved first-prompt text, if any.
    fn feed(&mut self, session_id: &str, cwd: &str, fixture_name: &str) -> Option<String> {
        let path = fixture(fixture_name);
        self.engine.apply(&HookEvent {
            hook_event_name: "SessionStart".to_string(),
            session_id: session_id.to_string(),
            cwd: cwd.to_string(),
            transcript_path: Some(path.clone()),
            ..Default::default()
        });

        let fp = descriptor::first_prompt(&path);

        if let Some(fp) = &fp {
            let human = fp.human_marked || self.markers.is_human(&fp.text);
            let allowlisted = self.never_hide.matches(cwd, Some(&fp.text));
            if !human && !allowlisted {
                self.obs.observe(SALT, session_id, &fp.text);
            }
        }

        self.engine
            .set_first_prompt(session_id, fp.as_ref().map(|f| f.text.clone()));
        fp.map(|f| f.text)
    }

    /// Push a rule-set change to both the harness's own copy (used by
    /// `proposals::build`) and the live engine (used by `is_hidden`) — the
    /// two must never drift, since production keeps exactly one live
    /// `IgnoreRules` per kind.
    fn set_ignore(&mut self, rules: IgnoreRules) {
        self.engine.set_ignore_rules(rules.clone());
        self.ignore = rules;
    }

    fn set_never_hide(&mut self, rules: IgnoreRules) {
        self.engine.set_never_hide(rules.clone());
        self.never_hide = rules;
    }

    fn proposals(&self, threshold: u32) -> Vec<proposals::Proposal> {
        proposals::build(&self.obs, threshold, &self.ignore, &self.never_hide)
    }

    fn is_hidden(&self, session_id: &str) -> bool {
        self.engine.is_hidden(session_id)
    }
}

/// The human/adversarial fixtures the 0-false-positive claim is measured
/// over. Deliberately excludes the machine fixtures — asserting "zero hidden"
/// over *all* fixtures would be trivially false (the machine sessions are
/// hidden on purpose).
const HUMAN_FIXTURES: &[&str] = &[
    "human_quotes_machine_phrase.jsonl",
    "human_ide_marked.jsonl",
    "human_repeated_opening.jsonl",
    "human_array_content.jsonl",
];

#[test]
fn both_machine_families_are_proposed() {
    let mut r = Replay::new();
    for i in 0..3 {
        r.feed(&format!("ma-{i}"), "/work/a", "machine_ecc_observer.jsonl");
    }
    for i in 0..3 {
        r.feed(&format!("mb-{i}"), "/work/b", "machine_ecc_summary.jsonl");
    }
    let proposals = r.proposals(3);
    assert_eq!(
        proposals.len(),
        2,
        "both machine families cross the propose threshold"
    );

    let observer_text = descriptor::first_prompt(&fixture("machine_ecc_observer.jsonl"))
        .unwrap()
        .text;
    let summary_text = descriptor::first_prompt(&fixture("machine_ecc_summary.jsonl"))
        .unwrap()
        .text;
    assert!(
        proposals
            .iter()
            .any(|p| observer_text.starts_with(&p.sample)),
        "family A must appear"
    );
    assert!(
        proposals
            .iter()
            .any(|p| summary_text.starts_with(&p.sample)),
        "family B must appear"
    );
}

/// The headline metric: accept both machine proposals as rules, replay every
/// human fixture, and assert none of them is hidden.
#[test]
fn no_human_fixture_is_hidden_by_an_accepted_machine_rule() {
    let mut r = Replay::new();
    for i in 0..3 {
        r.feed(&format!("ma-{i}"), "/work/a", "machine_ecc_observer.jsonl");
    }
    for i in 0..3 {
        r.feed(&format!("mb-{i}"), "/work/b", "machine_ecc_summary.jsonl");
    }
    let proposals = r.proposals(3);
    assert_eq!(proposals.len(), 2);
    let accepted: Vec<Matcher> = proposals
        .iter()
        .map(|p| Matcher::FirstPromptPrefix {
            value: p.sample.clone(),
        })
        .collect();
    r.set_ignore(IgnoreRules::new(accepted));

    for (i, fixture_name) in HUMAN_FIXTURES.iter().enumerate() {
        let sid = format!("human-{i}");
        r.feed(&sid, "/work/human", fixture_name);
        assert!(
            !r.is_hidden(&sid),
            "human fixture {fixture_name} was hidden by an accepted machine rule \
             — the 0-false-positive claim, mechanised"
        );
    }
}

#[test]
fn quoted_phrase_stays_visible() {
    let mut r = Replay::new();
    for i in 0..3 {
        r.feed(&format!("m-{i}"), "/work/a", "machine_ecc_observer.jsonl");
    }
    let proposals = r.proposals(3);
    assert_eq!(proposals.len(), 1);
    r.set_ignore(IgnoreRules::new(vec![Matcher::FirstPromptPrefix {
        value: proposals[0].sample.clone(),
    }]));

    r.feed("quoter", "/work/human", "human_quotes_machine_phrase.jsonl");
    assert!(
        !r.is_hidden("quoter"),
        "the spawner phrase appears mid-prompt, not anchored at the start — must stay visible"
    );
}

#[test]
fn ide_marked_never_clusters() {
    let mut r = Replay::new();
    for i in 0..5 {
        r.feed(&format!("s-{i}"), "/work/ide", "human_ide_marked.jsonl");
    }
    assert!(
        r.obs.iter_with_samples().next().is_none(),
        "an IDE-marked opening must never reach the observation store, however often it repeats"
    );
}

#[test]
fn array_content_prompt_is_read() {
    let mut r = Replay::new();
    let text = r.feed("s1", "/work/array", "human_array_content.jsonl");
    assert!(
        text.as_deref()
            == Some("please audit the observation store for any accidental plaintext leaks"),
        "array-content prompts must resolve, not just string ones"
    );
}

#[test]
fn sha_worktree_is_not_hidden_by_shape() {
    let mut r = Replay::new();
    let sha_cwd = r"C:\Users\me\.local\share\ecc-homunculus\projects\b4807c9eabf7a1928ef";

    for i in 0..3 {
        r.feed(&format!("m-{i}"), sha_cwd, "machine_sha_worktree.jsonl");
    }
    let proposals = r.proposals(3);
    assert_eq!(proposals.len(), 1);
    r.set_ignore(IgnoreRules::new(vec![Matcher::FirstPromptPrefix {
        value: proposals[0].sample.clone(),
    }]));
    assert!(
        r.is_hidden("m-0"),
        "the machine session in the SHA-named worktree is hidden — by its prompt"
    );

    r.feed("human-in-sha", sha_cwd, "human_array_content.jsonl");
    assert!(
        !r.is_hidden("human-in-sha"),
        "a hex-named folder alone must not hide a session — folder_hex was deleted for this reason"
    );
}

#[test]
fn shared_cwd_separates_by_prompt_not_path() {
    let mut r = Replay::new();
    let cwd = "/home/user/shared-project";
    r.feed("machine-here", cwd, "shared_cwd_machine.jsonl");
    r.feed("human-here", cwd, "shared_cwd_human.jsonl");

    let machine_text = descriptor::first_prompt(&fixture("shared_cwd_machine.jsonl"))
        .unwrap()
        .text;
    r.set_ignore(IgnoreRules::new(vec![Matcher::FirstPromptPrefix {
        value: machine_text.chars().take(60).collect(),
    }]));

    assert!(r.is_hidden("machine-here"));
    assert!(
        !r.is_hidden("human-here"),
        "one cwd, two sessions, opposite verdicts — driven by the prompt, not the path"
    );
}

/// PRD success signal, verbatim: an allowlisted opening never even reaches
/// the store. Asserted on the *store* (`to_json`), not the proposal list —
/// that is the whole point of the guard living at ingest.
#[test]
fn allowlisted_opening_never_reaches_the_store() {
    let mut r = Replay::new();
    let full = descriptor::first_prompt(&fixture("human_repeated_opening.jsonl"))
        .unwrap()
        .text;
    let rule_value: String = full.chars().take(40).collect();
    r.set_never_hide(IgnoreRules::new(vec![Matcher::FirstPromptPrefix {
        value: rule_value,
    }]));

    for i in 0..3 {
        r.feed(
            &format!("h-{i}"),
            "/work/repeat",
            "human_repeated_opening.jsonl",
        );
    }

    assert!(
        r.obs
            .to_json()
            .as_object()
            .expect("store serializes as an object")
            .is_empty(),
        "an allowlisted opening must never be fingerprinted at all"
    );
}

#[test]
fn never_hide_overlapping_an_accepted_rule_stays_visible() {
    let mut r = Replay::new();
    for i in 0..3 {
        r.feed(&format!("m-{i}"), "/work/a", "machine_ecc_observer.jsonl");
    }
    let proposals = r.proposals(3);
    let sample = proposals[0].sample.clone();
    r.set_ignore(IgnoreRules::new(vec![Matcher::FirstPromptPrefix {
        value: sample.clone(),
    }]));
    assert!(r.is_hidden("m-0"), "premise: the ignore rule hides it");

    r.set_never_hide(IgnoreRules::new(vec![Matcher::FirstPromptPrefix {
        value: sample,
    }]));
    assert!(
        !r.is_hidden("m-0"),
        "never_hide outranks an overlapping ignore rule"
    );
}

/// The blind spot named throughout the PRD: a human typing the same short
/// request repeatedly clusters exactly like a machine spawner from this
/// module's point of view. Documenting the uncomfortable outcome, then
/// proving the safety valve (`never_suggest` semantics) actually removes it.
#[test]
fn repeated_human_opening_clusters_and_is_removable() {
    let mut r = Replay::new();
    for i in 0..3 {
        r.feed(
            &format!("h-{i}"),
            "/work/repeat",
            "human_repeated_opening.jsonl",
        );
    }
    let proposals = r.proposals(3);
    assert_eq!(
        proposals.len(),
        1,
        "a repeated human opening clusters like a machine family — the known blind spot"
    );

    let fp = proposals[0].fingerprint.clone();
    let sample = proposals[0].sample.clone();
    // `never_suggest_proposal` (lib.rs) purges the family from the store AND
    // adds a never_hide rule; mirror both here.
    r.obs.purge_family(&fp);
    r.set_never_hide(IgnoreRules::new(vec![Matcher::FirstPromptPrefix {
        value: sample,
    }]));

    assert!(
        r.proposals(3).is_empty(),
        "never_suggest semantics remove the proposal"
    );

    // It does not return even after being observed again.
    r.feed("h-extra", "/work/repeat", "human_repeated_opening.jsonl");
    assert!(
        r.proposals(3).is_empty(),
        "the allowlisted opening does not return once re-observed"
    );
}

/// H1 at integration level: dismissing the proposal the user was actually
/// shown must hold across a full replay of the rest of the corpus, not just
/// in isolation.
#[test]
fn dismissal_holds_across_the_fixture_corpus() {
    let mut r = Replay::new();
    for i in 0..3 {
        r.feed(&format!("m-{i}"), "/work/a", "machine_ecc_observer.jsonl");
    }
    let proposals = r.proposals(3);
    assert_eq!(proposals.len(), 1);
    let fp = proposals[0].fingerprint.clone();
    let count = proposals[0].count;
    r.obs.dismiss(&fp, count);
    assert!(
        r.proposals(3).is_empty(),
        "a dismissed proposal does not resurface immediately"
    );

    for (i, name) in [
        "machine_ecc_summary.jsonl",
        "human_quotes_machine_phrase.jsonl",
        "human_ide_marked.jsonl",
        "human_array_content.jsonl",
    ]
    .into_iter()
    .enumerate()
    {
        r.feed(&format!("extra-{i}"), "/work/extra", name);
    }

    let proposals = r.proposals(3);
    assert!(
        proposals.iter().all(|p| p.fingerprint != fp),
        "the dismissed machine family must not resurface while replaying the rest of the corpus"
    );
}

/// PRD metric: after a full replay, no fixture's prompt text appears anywhere
/// in the persisted store JSON — only hashes and counts.
#[test]
fn store_contains_no_fixture_prompt_text() {
    let mut r = Replay::new();
    let sessions: &[(&str, &str, &str)] = &[
        ("m1", "/work/a", "machine_ecc_observer.jsonl"),
        ("m2", "/work/a", "machine_ecc_observer.jsonl"),
        ("m3", "/work/a", "machine_ecc_observer.jsonl"),
        ("m4", "/work/b", "machine_ecc_summary.jsonl"),
        ("m5", "/work/b", "machine_ecc_summary.jsonl"),
        ("m6", "/work/b", "machine_ecc_summary.jsonl"),
        ("h1", "/work/c", "human_quotes_machine_phrase.jsonl"),
        ("h2", "/work/d", "human_ide_marked.jsonl"),
        ("h3a", "/work/e", "human_repeated_opening.jsonl"),
        ("h3b", "/work/e", "human_repeated_opening.jsonl"),
        ("h3c", "/work/e", "human_repeated_opening.jsonl"),
        ("h4", "/work/f", "human_array_content.jsonl"),
    ];

    let mut texts = Vec::new();
    for (sid, cwd, name) in sessions {
        if let Some(t) = r.feed(sid, cwd, name) {
            texts.push(t);
        }
    }

    let store_text = r.obs.to_json().to_string();
    for text in &texts {
        for word in text.split_whitespace() {
            let cleaned: String = word.chars().filter(|c| c.is_alphanumeric()).collect();
            if cleaned.len() < 4 {
                continue;
            }
            assert!(
                !store_text.contains(&cleaned),
                "fixture word {cleaned:?} leaked into the persisted store JSON"
            );
        }
    }
}
