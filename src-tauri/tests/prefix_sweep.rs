//! Decision 6 (PRD) + M1 follow-through #2: at what prefix length does a
//! prefix stop discriminating between human and machine-shaped openings, and
//! what does that look like specifically for samples under 60 chars?
//!
//! **Reads a real, LOCAL, uncommitted corpus.** Never runs in CI, asserts
//! nothing about a specific number, and never touches a fixture — only that
//! the sweep executed and produced a table. The table is the deliverable;
//! paste it into `docs/internal/measurements.md` (the archived raw record); the
//! published summary lives in `docs/IGNORING_BOT_SPAWNED_SESSIONS.md`.
//!
//! ```text
//! BEACON_CORPUS=/path/to/.claude/projects cargo test --test prefix_sweep -- --ignored --nocapture
//! ```
//!
//! Reuses the real pipeline for both hard parts the original ad-hoc research
//! got wrong: `descriptor::first_prompt` (a depth-1 glob missed 87 nested
//! files; a string-only parse missed 99 array-content prompts — both bugs
//! are impossible to reintroduce here because this walks the whole tree and
//! calls the same function production calls) and `observe::{sample,
//! fingerprint}` (never re-implements the grouping key).

use beacon_lib::{descriptor, markers, observe};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SALT: &[u8] = b"prefix-sweep-local-only-salt";

/// Recurse the whole tree looking for `.jsonl` files — a depth-1 glob missed
/// 87 nested transcripts during the PRD's original research; this is why
/// that bug can't recur.
fn walk_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_jsonl(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

struct Resolved {
    text: String,
    /// Would this opening be skipped by the marker guard at ingest —
    /// `fp.human_marked` (a wrapper preceded it) or a built-in Human marker
    /// classifies its text. Mirrors `observe_opening`'s first guard exactly.
    human: bool,
}

/// Group `resolved` by fingerprint at prefix length `len`, restricted to
/// samples shorter than `max_chars` chars (pass `usize::MAX` for no cap), and
/// return (cluster count, mixed-polarity cluster count, sample count).
/// "Mixed" is the discrimination failure: a cluster containing both a
/// human-marked and an unmarked opening at this prefix length.
fn cluster_at(resolved: &[Resolved], len: usize, max_chars: usize) -> (usize, usize, usize) {
    let mut groups: HashMap<String, Vec<bool>> = HashMap::new();
    for r in resolved {
        let Some(s) = observe::sample(&r.text, len) else {
            continue;
        };
        if s.chars().count() >= max_chars {
            continue;
        }
        let fp = observe::fingerprint(SALT, &s);
        groups.entry(fp).or_default().push(r.human);
    }
    let clusters = groups.values().filter(|v| v.len() > 1).count();
    let mixed = groups
        .values()
        .filter(|v| v.len() > 1 && v.iter().any(|h| *h) && v.iter().any(|h| !*h))
        .count();
    let n: usize = groups.values().map(|v| v.len()).sum();
    (clusters, mixed, n)
}

#[test]
#[ignore = "reads a local corpus; run with `cargo test --test prefix_sweep -- --ignored --nocapture` and BEACON_CORPUS set"]
fn prefix_discrimination_sweep() {
    let Ok(root) = std::env::var("BEACON_CORPUS") else {
        eprintln!("BEACON_CORPUS not set — skipping (must never fail CI; see the doc comment)");
        return;
    };
    let root = Path::new(&root);
    assert!(root.is_dir(), "BEACON_CORPUS must point at a directory");

    let mut files = Vec::new();
    walk_jsonl(root, &mut files);
    assert!(
        !files.is_empty(),
        "BEACON_CORPUS contained no .jsonl files — wrong path?"
    );

    let registry = markers::Registry::new(vec![]);
    let mut resolved: Vec<Resolved> = Vec::new();
    for f in &files {
        let Some(path) = f.to_str() else { continue };
        if let Some(fp) = descriptor::first_prompt(path) {
            let human = fp.human_marked || registry.is_human(&fp.text);
            resolved.push(Resolved {
                text: fp.text,
                human,
            });
        }
    }

    println!(
        "prefix_discrimination_sweep: {} files walked, {} resolved a first prompt \
         ({} human-marked, {} unmarked)",
        files.len(),
        resolved.len(),
        resolved.iter().filter(|r| r.human).count(),
        resolved.iter().filter(|r| !r.human).count(),
    );
    println!(
        "{:>5} | {:>9} | {:>6} | {:>10} | {:>9} | {:>8}",
        "len", "clusters", "mixed", "sub60_clus", "sub60_mix", "sub60_n"
    );

    for len in 4..=120usize {
        let (clusters, mixed, _n) = cluster_at(&resolved, len, usize::MAX);
        let (sub60_clusters, sub60_mixed, sub60_n) = cluster_at(&resolved, len, 60);
        println!(
            "{len:>5} | {clusters:>9} | {mixed:>6} | {sub60_clusters:>10} | {sub60_mixed:>9} | {sub60_n:>8}"
        );
    }
}
