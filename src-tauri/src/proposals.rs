//! Turn the observation store into an *offer*: cluster the salted
//! fingerprints `observe.rs` already records into eligible filter proposals.
//!
//! Pure core — no `AppHandle`, no `Engine` dependency — so it's fully
//! unit-testable without the Tauri test harness this codebase doesn't have
//! (see the Phase 2/3 review). The command layer (`lib.rs`) fills in
//! `matching` from the live engine after calling [`build`].

use crate::config::MIN_PROPOSE_THRESHOLD;
use crate::engine::SessionView;
use crate::ignore::{IgnoreRules, Matcher};
use crate::observe::Observations;
use serde::Serialize;

/// An offer built from a pattern the app actually observed: accept it to
/// write a `first_prompt_prefix` ignore rule, or refuse it (this run, or
/// permanently).
#[derive(Serialize, Clone, Debug)]
pub struct Proposal {
    /// Opaque cluster id — what `accept_proposal` / `dismiss_proposal` /
    /// `never_suggest_proposal` take. Never shown to the user.
    pub fingerprint: String,
    /// The literal opening a rule would be written from. Live in memory
    /// only — a pattern you cannot read is one you must not be asked to
    /// accept, so a cluster with no live sample is not a proposal.
    pub sample: String,
    pub len: u16,
    pub count: u32,
    pub first_seen: u64,
    pub last_seen: u64,
    /// Currently-visible sessions this rule would hide. Left **empty** by
    /// `build` — the command layer fills it from the engine, which this
    /// module deliberately does not depend on. May be shorter than `count`:
    /// `count` groups on the whitespace-normalized form while a rule is a
    /// literal prefix, and only sessions live *right now* can appear.
    pub matching: Vec<SessionView>,
}

/// Build the eligible proposal list, highest count first. `matching` is
/// always empty here — see the field doc.
///
/// Pipeline, in order:
/// 1. Only fingerprints with a live sample (`Observations::iter_with_samples`).
/// 2. Cluster size `>= threshold.max(MIN_PROPOSE_THRESHOLD)` — the floor holds
///    even if `threshold` arrives unsanitized.
/// 3. Drop clusters already covered by an existing `ignore_rules` entry.
/// 4. Drop clusters matching `never_hide` — **prompt-only**: a `never_hide`
///    cwd rule can't be evaluated here, there's no cwd in the store, by
///    design. A real but fail-open gap (the proposal still surfaces; the user
///    still decides).
/// 5. Drop clusters dismissed at or above their current count (a dismissal
///    lapses once the cluster grows).
/// 6. Shortest-prefix-wins: of the survivors, keep a candidate only if no
///    already-kept (shorter) sample is a literal prefix of it — evaluated via
///    a throwaway `IgnoreRules` so the dedup can never disagree with the rule
///    semantics it mirrors.
/// 7. Sort by count desc, then last_seen desc, then fingerprint asc.
pub fn build(
    obs: &Observations,
    threshold: u32,
    ignore: &IgnoreRules,
    never_hide: &IgnoreRules,
) -> Vec<Proposal> {
    let floor = threshold.max(MIN_PROPOSE_THRESHOLD);

    let mut candidates: Vec<Proposal> = obs
        .iter_with_samples()
        .filter(|(_, rec, _)| rec.n >= floor)
        .filter(|(_, _, sample)| !ignore.matches_prompt(sample))
        .filter(|(_, _, sample)| !never_hide.matches_prompt(sample))
        .filter(|(fp, rec, _)| obs.dismissed_at(fp).is_none_or(|n| rec.n > n))
        .map(|(fp, rec, sample)| Proposal {
            fingerprint: fp.to_string(),
            sample: sample.to_string(),
            len: rec.len,
            count: rec.n,
            first_seen: rec.first,
            last_seen: rec.last,
            matching: Vec::new(),
        })
        .collect();

    // Shortest-prefix-wins de-duplication: shortest first, ties broken by
    // fingerprint for determinism.
    candidates.sort_by(|a, b| {
        a.sample
            .chars()
            .count()
            .cmp(&b.sample.chars().count())
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });

    let mut kept: Vec<Proposal> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let already_covered = kept.iter().any(|k| {
            let probe = IgnoreRules::new(vec![Matcher::FirstPromptPrefix {
                value: k.sample.clone(),
            }]);
            probe.matches_prompt(&candidate.sample)
        });
        if !already_covered {
            kept.push(candidate);
        }
    }

    kept.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    const SALT: &[u8] = b"fixed-test-salt";

    /// `seen` (the intra-run dedup set) is keyed by session id across the
    /// **whole store**, not per-prompt — so session ids must be unique across
    /// every call sharing one `Observations`, not just within one call.
    fn observe_n_times(obs: &mut Observations, tag: &str, prompt: &str, n: usize) {
        for i in 0..n {
            obs.observe(SALT, &format!("session-{tag}-{i}"), prompt);
        }
    }

    #[test]
    fn below_threshold_is_not_proposed() {
        let mut obs = Observations::default();
        observe_n_times(
            &mut obs,
            "a",
            "please review the listener implementation end to end thoroughly",
            2,
        );
        let proposals = build(&obs, 3, &IgnoreRules::default(), &IgnoreRules::default());
        assert!(proposals.is_empty());
    }

    /// A record loaded from a persisted JSON store (this run never observed
    /// it) has no live sample and must never be proposed — the in-memory
    /// invariant.
    #[test]
    fn cluster_without_live_sample_is_not_proposed() {
        let mut obs = Observations::default();
        observe_n_times(
            &mut obs,
            "a",
            "an opening that repeats plenty of times here",
            5,
        );
        let reloaded = Observations::from_json(obs.to_json());
        let proposals = build(
            &reloaded,
            3,
            &IgnoreRules::default(),
            &IgnoreRules::default(),
        );
        assert!(proposals.is_empty());
    }

    /// One long prompt observed 3x fingerprints at all five `PREFIX_LENS`;
    /// shortest-prefix-wins must collapse that to exactly one proposal, the
    /// shortest sample.
    #[test]
    fn shortest_prefix_wins_across_lengths() {
        let mut obs = Observations::default();
        let long_prompt = "please review the listener implementation end to end thoroughly \
            and carefully, checking every code path for correctness and safety concerns \
            before merging this into the main branch for release";
        observe_n_times(&mut obs, "a", long_prompt, 3);
        let proposals = build(&obs, 3, &IgnoreRules::default(), &IgnoreRules::default());
        assert_eq!(proposals.len(), 1);
        assert!(long_prompt.starts_with(&proposals[0].sample));
    }

    #[test]
    fn two_families_yield_two_proposals() {
        let mut obs = Observations::default();
        observe_n_times(
            &mut obs,
            "a",
            "first distinct family opening repeated several times for testing purposes",
            3,
        );
        observe_n_times(
            &mut obs,
            "b",
            "second entirely different family opening also repeated for testing",
            3,
        );
        let proposals = build(&obs, 3, &IgnoreRules::default(), &IgnoreRules::default());
        assert_eq!(proposals.len(), 2);
    }

    #[test]
    fn already_covered_by_ignore_rule_is_skipped() {
        let mut obs = Observations::default();
        let prompt = "IMPORTANT: You are running in non-interactive --print mode right now";
        observe_n_times(&mut obs, "a", prompt, 3);
        let ignore = IgnoreRules::new(vec![Matcher::FirstPromptPrefix {
            value: "IMPORTANT: You are running in non-interactive".to_string(),
        }]);
        let proposals = build(&obs, 3, &ignore, &IgnoreRules::default());
        assert!(proposals.is_empty());
    }

    #[test]
    fn never_hide_prefix_suppresses_the_proposal() {
        let mut obs = Observations::default();
        let prompt = "a pattern the user has already declared their own for testing purposes";
        observe_n_times(&mut obs, "a", prompt, 3);
        let never_hide = IgnoreRules::new(vec![Matcher::FirstPromptPrefix {
            value: "a pattern the user has already declared".to_string(),
        }]);
        let proposals = build(&obs, 3, &IgnoreRules::default(), &never_hide);
        assert!(proposals.is_empty());
    }

    #[test]
    fn dismissed_returns_when_the_cluster_grows() {
        let mut obs = Observations::default();
        // Kept under 60 chars so all five `PREFIX_LENS` collapse to a single
        // record (Task 3's dedup) — a longer prompt would fingerprint at
        // several lengths, and dismissing just one wouldn't clear the others.
        let prompt = "a short dismissed opening for growth testing";
        observe_n_times(&mut obs, "a", prompt, 3);
        let fp = obs
            .iter_with_samples()
            .next()
            .map(|(fp, _, _)| fp.to_string())
            .unwrap();
        obs.dismiss(&fp, 3);
        assert!(build(&obs, 3, &IgnoreRules::default(), &IgnoreRules::default()).is_empty());

        obs.observe(SALT, "session-extra", prompt);
        let proposals = build(&obs, 3, &IgnoreRules::default(), &IgnoreRules::default());
        assert_eq!(proposals.len(), 1, "reappears once the cluster grew past 3");
    }

    #[test]
    fn ordering_is_highest_count_first() {
        let mut obs = Observations::default();
        observe_n_times(
            &mut obs,
            "a",
            "cluster with a modest count of three for testing",
            3,
        );
        observe_n_times(
            &mut obs,
            "b",
            "cluster with a much larger count of seven right here",
            7,
        );
        observe_n_times(
            &mut obs,
            "c",
            "cluster with a middling count of five for this test",
            5,
        );
        let proposals = build(&obs, 3, &IgnoreRules::default(), &IgnoreRules::default());
        let counts: Vec<u32> = proposals.iter().map(|p| p.count).collect();
        assert_eq!(counts, vec![7, 5, 3]);
    }

    #[test]
    fn threshold_below_floor_is_clamped_in_build() {
        let mut obs = Observations::default();
        observe_n_times(
            &mut obs,
            "a",
            "a cluster observed exactly three times for this test",
            3,
        );
        let mut low = Observations::default();
        observe_n_times(
            &mut low,
            "a",
            "a cluster observed only twice for this test case",
            2,
        );
        let proposals = build(&low, 1, &IgnoreRules::default(), &IgnoreRules::default());
        assert!(
            proposals.is_empty(),
            "floor holds even with an unsanitized threshold"
        );
        let proposals = build(&obs, 1, &IgnoreRules::default(), &IgnoreRules::default());
        assert_eq!(
            proposals.len(),
            1,
            "a 3-count cluster still qualifies at the floor"
        );
    }

    /// Synthetic corpus replay over the whole pure pipeline. NOT a claim
    /// about the real 577-session research corpus (that's Phase 6's
    /// deliverable, PRD-sourced) — this builds its own small in-test corpus:
    /// two machine families (long, stable, observed ≥3x each) and three human
    /// openings — one repeated verbatim 3x (the PRD's known blind spot), one
    /// that would be wrapper-marked in real ingest (simulated by simply never
    /// observing it — the wrapper guard lives in `observe_opening`/ingest,
    /// out of this pure module's scope), and one one-off.
    #[test]
    fn synthetic_corpus_yields_one_proposal_per_family() {
        let mut obs = Observations::default();

        let machine_family_a = "IMPORTANT: You are running in non-interactive --print mode. \
            You MUST use the Write tool to record your findings before exiting.";
        let machine_family_b = "SYSTEM: This is an automated agent run. Follow the task \
            specification exactly and report status via the designated output channel.";
        observe_n_times(&mut obs, "ma", machine_family_a, 4);
        observe_n_times(&mut obs, "mb", machine_family_b, 3);

        // The PRD's known blind spot: a human typing the same request three
        // times looks identical to a repeating machine spawner from here —
        // this pure module has no marker/ingest context to tell them apart.
        let repeated_human = "please review the pull request and leave comments on anything \
            that looks off before we merge it into main";
        observe_n_times(&mut obs, "h", repeated_human, 3);

        // A wrapper-marked opening is never fed in at all: that guard lives
        // in `observe_opening` (ingest), not in this pure clustering module,
        // so its absence here is definitional, not a pipeline outcome.
        let one_off = "just a single unique question about the config file format";
        observe_n_times(&mut obs, "o", one_off, 1);

        let proposals = build(&obs, 3, &IgnoreRules::default(), &IgnoreRules::default());
        let samples: Vec<&str> = proposals.iter().map(|p| p.sample.as_str()).collect();
        assert!(
            samples
                .iter()
                .any(|s| machine_family_a.starts_with(s) && !s.is_empty()),
            "machine family A must appear"
        );
        assert!(
            samples
                .iter()
                .any(|s| machine_family_b.starts_with(s) && !s.is_empty()),
            "machine family B must appear"
        );
        assert!(
            samples
                .iter()
                .any(|s| repeated_human.starts_with(s) && !s.is_empty()),
            "the repeated human opening also appears — documenting the \
             over-suggestion this corpus cannot rule out, not asserting it's fine"
        );
        assert!(
            !samples.iter().any(|s| one_off.starts_with(s)),
            "a one-off opening never crosses the threshold"
        );
        assert_eq!(proposals.len(), 3);

        // Declaring the repeated human opening one's own removes it — the
        // safety valve for exactly this blind spot. Uses the *proposal's own
        // sample* as the rule value (what `never_suggest_proposal` actually
        // writes), not the raw prompt — a rule longer than the sample it's
        // tested against could never match via a prefix test.
        let human_sample = proposals
            .iter()
            .find(|p| repeated_human.starts_with(&p.sample))
            .map(|p| p.sample.clone())
            .expect("premise: the repeated human opening was proposed above");
        let never_hide = IgnoreRules::new(vec![Matcher::FirstPromptPrefix {
            value: human_sample,
        }]);
        let proposals = build(&obs, 3, &IgnoreRules::default(), &never_hide);
        assert_eq!(proposals.len(), 2, "the two machine families remain");
        assert!(
            proposals
                .iter()
                .all(|p| !repeated_human.starts_with(&p.sample)),
            "the allowlisted human opening is gone"
        );
    }
}
