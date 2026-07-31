//! Observation store: records which prompt *openings* repeat across sessions,
//! as salted hashes — never plaintext — so a later phase can offer the user a
//! filter rule built from their own repeated pattern instead of a guessed one.
//!
//! Three layers, in the order data flows through them:
//!   - `salt`: a per-install secret, independent of the listener auth token.
//!   - `sample`/`fingerprint`: turn a prompt into a handful of salted hashes,
//!     one per tracked prefix length.
//!   - [`Observations`]: the in-memory + persisted counts, deduped per
//!     session per run.
//!
//! Nothing here ever writes prompt text to disk — see [`Observations::to_json`].

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

const STORE_FILE: &str = "beacon.json";
const SALT_KEY: &str = "observe_salt";

pub mod salt {
    use super::*;
    use tauri::AppHandle;
    use tauri_plugin_store::StoreExt;

    /// Mint a new 32-byte salt, hex-encoded (64 chars). Mirrors
    /// `token::generate` exactly, but under a **separate** store key —
    /// [`SALT_KEY`], never `auth_token`. Different purpose, different
    /// lifetime: `regenerate_token` mints a fresh token on demand, and
    /// reusing its key would silently re-key (and so orphan) every stored
    /// fingerprint the moment a user rotates their listener secret.
    pub fn generate() -> String {
        use std::fmt::Write;
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("OS CSPRNG unavailable — refusing to mint a weak salt");
        let mut s = String::with_capacity(64);
        for b in bytes {
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    /// Read the persisted salt, generating + saving one on first run. Always
    /// returns a usable salt (a save failure is tolerated — the in-memory
    /// value still works for this run).
    pub fn load_or_create(app: &AppHandle) -> String {
        if let Ok(store) = app.store(STORE_FILE) {
            if let Some(v) = store.get(SALT_KEY) {
                if let Some(s) = v.as_str() {
                    if !s.is_empty() {
                        return s.to_string();
                    }
                }
            }
            let salt = generate();
            store.set(SALT_KEY, serde_json::Value::String(salt.clone()));
            let _ = store.save();
            return salt;
        }
        generate()
    }

    /// Decode a hex salt string to raw bytes for hashing. Char-based (not
    /// byte-slicing) so malformed input can never panic on a non-boundary
    /// split; falls back to hashing the raw string bytes, which should be
    /// unreachable since we only ever decode what `generate` produced.
    pub fn bytes(hex: &str) -> Vec<u8> {
        let chars: Vec<char> = hex.chars().collect();
        let mut out = Vec::with_capacity(chars.len() / 2);
        for pair in chars.chunks(2) {
            let s: String = pair.iter().collect();
            match u8::from_str_radix(&s, 16) {
                Ok(b) => out.push(b),
                Err(_) => return hex.as_bytes().to_vec(),
            }
        }
        out
    }
}

/// Prefix lengths fingerprinted per session. Identical clusters on today's
/// corpus at every length — carried as insurance against a spawner that
/// varies before char 120, not as measured gain (PRD).
pub const PREFIX_LENS: &[usize] = &[60, 70, 85, 100, 120];

/// The text a rule would be written from: raw prompt, leading whitespace
/// trimmed, first `len` chars (char-boundary safe). Case and internal
/// whitespace preserved so the result is a **literal prefix** of the prompt.
/// No length floor — a prompt shorter than `len` is sampled at its own
/// length rather than refused, so short openings are still counted. `None`
/// only when the trimmed prompt is empty: a whitespace-only opening would
/// otherwise fingerprint the empty string, and a cluster of those would
/// propose a rule with an empty `value` — `starts_with_ci` returns `false`
/// on an empty prefix, so it would hide nothing while looking accepted.
pub fn sample(prompt: &str, len: usize) -> Option<String> {
    let trimmed = prompt.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(len).collect())
}

/// Grouping key input: `sample` lowercased with whitespace runs collapsed, so
/// a spawner varying indentation still clusters.
fn normalize(sample: &str) -> String {
    sample
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// `sha256(salt || normalize(sample))`, first 16 bytes as 32 hex chars.
/// Truncated to 128 bits deliberately: halves store size, and the realistic
/// attack is a dictionary of candidate prefixes, which full width wouldn't
/// prevent either.
pub fn fingerprint(salt: &[u8], sample: &str) -> String {
    use std::fmt::Write;
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(normalize(sample).as_bytes());
    let digest = hasher.finalize();
    let mut s = String::with_capacity(32);
    for b in &digest[..16] {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// One fingerprint's persisted record.
///
/// Container-level `#[serde(default)]` + `Default` so a record missing a
/// field (e.g. a truncated write) still loads instead of dropping the whole
/// entry. A record that loses `last` defaults to `0` and is pruned on the
/// next sweep — a record we cannot date should not accumulate.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Observation {
    /// The actual char count of the sample that produced this fingerprint —
    /// may be shorter than the nominal `PREFIX_LENS` entry for a short
    /// prompt. Retained for display on the proposal card; `proposals::build`'s
    /// dedup uses `sample.chars().count()` directly, not this field.
    pub len: u16,
    pub n: u32,
    /// Unix seconds (wall clock — `Instant` can't survive a restart).
    pub first: u64,
    pub last: u64,
}

/// The observation store: persisted counts plus two run-only side tables.
#[derive(Default)]
pub struct Observations {
    /// PERSISTED: fingerprint → record. The only thing that reaches disk.
    records: HashMap<String, Observation>,
    /// NOT PERSISTED: fingerprint → sample text, this run only. Upholds the
    /// invariant that a proposal is only ever surfaced while its text is live
    /// in memory — a pattern you cannot read is one you must not be asked to
    /// accept.
    samples: HashMap<String, String>,
    /// NOT PERSISTED: session ids already counted this run. The first-prompt
    /// read retries every few seconds until resolved, so without this one
    /// session would inflate its own count on every retry.
    seen: HashSet<String>,
    /// NOT PERSISTED: fingerprint → cluster count at the moment it was
    /// dismissed ("not now" — this run only). A dismissal lapses once the
    /// cluster grows past that count, matching the sample table's lifetime:
    /// a dismissal cannot outlive the sample that justified it.
    dismissed: HashMap<String, u32>,
    dirty: bool,
}

impl Observations {
    /// Record `prompt` as this session's opening, once per session per run.
    /// Returns whether anything was recorded (worth marking dirty upstream).
    pub fn observe(&mut self, salt: &[u8], session_id: &str, prompt: &str) -> bool {
        if self.seen.contains(session_id) {
            return false;
        }
        self.seen.insert(session_id.to_string());
        let now = now_secs();
        let mut recorded = false;
        // Fingerprints already counted this call. `normalize` collapses
        // whitespace, so two different prefix lengths can fingerprint
        // identically (e.g. a newline-plus-indentation tail) — without this,
        // one opening would double-count within a single `observe`.
        let mut counted: HashSet<String> = HashSet::new();
        for &len in PREFIX_LENS {
            let Some(s) = sample(prompt, len) else {
                continue;
            };
            let fp = fingerprint(salt, &s);
            if !counted.insert(fp.clone()) {
                continue;
            }
            // The record's `len` is the sample's actual char count, not the
            // nominal `PREFIX_LENS` entry — `sample` no longer floors on
            // length, so a short prompt's sample can be shorter than `len`.
            let actual_len = s.chars().count() as u16;
            self.samples.insert(fp.clone(), s);
            let rec = self.records.entry(fp).or_insert_with(|| Observation {
                len: actual_len,
                n: 0,
                first: now,
                last: now,
            });
            rec.n += 1;
            rec.last = now;
            recorded = true;
        }
        if recorded {
            self.dirty = true;
        }
        recorded
    }

    /// Drop records whose `last` is older than `retain_days` (relative to
    /// `now`, injected for testability). Orphaned samples are dropped too.
    pub fn prune(&mut self, retain_days: u64, now: u64) -> bool {
        let cutoff = now.saturating_sub(retain_days.saturating_mul(86_400));
        let before = self.records.len();
        self.records.retain(|_, r| r.last >= cutoff);
        let live: HashSet<&String> = self.records.keys().collect();
        self.samples.retain(|k, _| live.contains(k));
        let changed = self.records.len() != before;
        if changed {
            self.dirty = true;
        }
        changed
    }

    /// Wipe every observation. An explicit user act (a later phase's clear
    /// command); free to expose here.
    pub fn clear(&mut self) {
        self.records.clear();
        self.samples.clear();
        self.dismissed.clear();
        self.dirty = true;
    }

    /// Consume the dirty flag: true if anything changed since the last call.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Serialize the **persisted** records only — never `samples`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.records).unwrap_or(serde_json::Value::Null)
    }

    /// Load from a persisted value, tolerating a missing/garbage shape (falls
    /// back to an empty store) and a per-entry malformed record (dropped
    /// individually, never aborting the whole load). Only the key is ever
    /// logged on a drop — never the value — matching the habit of keeping
    /// prompt text out of logs, even though a value here is only hash + counts.
    pub fn from_json(v: serde_json::Value) -> Self {
        let raw =
            serde_json::from_value::<HashMap<String, serde_json::Value>>(v).unwrap_or_default();
        let mut records = HashMap::with_capacity(raw.len());
        for (k, val) in raw {
            match serde_json::from_value::<Observation>(val) {
                Ok(o) => {
                    records.insert(k, o);
                }
                Err(_) => eprintln!("beacon: dropping unreadable observation record {k}"),
            }
        }
        Observations {
            records,
            samples: HashMap::new(),
            seen: HashSet::new(),
            dismissed: HashMap::new(),
            dirty: false,
        }
    }

    /// Fingerprints whose sample text is still live this run, paired with
    /// their record and sample. This *is* the in-memory-sample invariant,
    /// expressed as the only way to enumerate: a record carried over from a
    /// prior run (no live sample) can never be iterated here, so it can never
    /// be surfaced as a proposal.
    pub fn iter_with_samples(&self) -> impl Iterator<Item = (&str, &Observation, &str)> {
        self.records.iter().filter_map(move |(fp, rec)| {
            self.samples.get(fp).map(|s| (fp.as_str(), rec, s.as_str()))
        })
    }

    /// The live sample text for a fingerprint, if this run has seen it.
    pub fn sample_for(&self, fp: &str) -> Option<&str> {
        self.samples.get(fp).map(|s| s.as_str())
    }

    /// Record that the user dismissed `fp` "for now", at its cluster count
    /// when dismissed. Run-only — see the `dismissed` field doc.
    pub fn dismiss(&mut self, fp: &str, at_count: u32) {
        self.dismissed.insert(fp.to_string(), at_count);
    }

    /// The cluster count at which `fp` was dismissed this run, if it was.
    pub fn dismissed_at(&self, fp: &str) -> Option<u32> {
        self.dismissed.get(fp).copied()
    }

    /// Remove `fp` and every other fingerprint whose *live sample* is
    /// prefix-related to `fp`'s (either direction, case-insensitively — the
    /// 60-char parent and the 120-char child of one opening are one family).
    /// Drops the matching `samples` and `dismissed` entries too. Returns how
    /// many records were removed.
    ///
    /// Relates fingerprints via `normalize` (lowercased, whitespace-collapsed
    /// prefix containment) — broader than `proposals::build`'s dedup, which
    /// relates them via `matches_prompt` (a raw literal, case-insensitive
    /// prefix). The two diverge when interior whitespace differs; broader is
    /// the safe direction for an explicit "never suggest".
    ///
    /// Can only relate fingerprints that currently have a live sample: a
    /// record carried over from a previous run has none and cannot be
    /// matched to a family, so this purge is never total across restarts.
    /// Hashes are one-way — there's no reversing a fingerprint back to text
    /// to compare it. `clear` is the escape hatch when a total wipe is
    /// wanted. `fp` itself is always removed regardless of whether it has a
    /// live sample.
    pub fn purge_family(&mut self, fp: &str) -> usize {
        let mut targets: HashSet<String> = HashSet::new();
        targets.insert(fp.to_string());
        if let Some(anchor) = self.samples.get(fp) {
            let anchor_norm = normalize(anchor);
            for (other_fp, other_sample) in &self.samples {
                if other_fp == fp {
                    continue;
                }
                let other_norm = normalize(other_sample);
                if anchor_norm.starts_with(&other_norm) || other_norm.starts_with(&anchor_norm) {
                    targets.insert(other_fp.clone());
                }
            }
        }
        let mut removed = 0;
        for t in &targets {
            if self.records.remove(t).is_some() {
                removed += 1;
            }
            self.samples.remove(t);
            self.dismissed.remove(t);
        }
        if removed > 0 {
            self.dirty = true;
        }
        removed
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salt_generate_is_64_hex_chars_and_unique() {
        // Mirrors `token::generate`'s own test — no Tauri store harness
        // exists in this codebase for `load_or_create`'s AppHandle-dependent
        // path (see `token.rs`, which likewise tests only `generate`).
        let a = salt::generate();
        let b = salt::generate();
        assert_eq!(a.len(), 64, "32 bytes → 64 hex chars");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "two salts must differ");
    }

    /// Reusing `auth_token`'s store key would let `regenerate_token` silently
    /// re-key (and so orphan) every stored fingerprint.
    #[test]
    fn salt_store_key_is_not_the_auth_token_key() {
        assert_ne!(SALT_KEY, "auth_token");
    }

    #[test]
    fn salt_bytes_roundtrips_hex() {
        let hex = salt::generate();
        let b = salt::bytes(&hex);
        assert_eq!(b.len(), 32);
    }

    #[test]
    fn same_prompt_same_fingerprint() {
        let salt = b"fixed-salt";
        let p = "please review the listener implementation end to end thoroughly";
        let a = fingerprint(salt, &sample(p, 60).unwrap());
        let b = fingerprint(salt, &sample(p, 60).unwrap());
        assert_eq!(a, b);
    }

    /// Whitespace variation (tab vs. space) still clusters — the grouping key
    /// is normalized even though the literal sample differs. Uses two
    /// single-char whitespace variants directly (not via `sample`'s
    /// truncation) so the comparison isn't confounded by the two source
    /// strings truncating to different content.
    #[test]
    fn fingerprint_survives_whitespace_variation() {
        let salt = b"fixed-salt";
        let a = "please review\tthe listener implementation end to end";
        let b = "please review the listener implementation end to end";
        assert_eq!(fingerprint(salt, a), fingerprint(salt, b));
    }

    #[test]
    fn fingerprint_changes_with_salt() {
        let p = "please review the listener implementation end to end thoroughly";
        let s = sample(p, 60).unwrap();
        assert_ne!(fingerprint(b"salt-one", &s), fingerprint(b"salt-two", &s));
    }

    #[test]
    fn sample_is_a_literal_prefix() {
        let p = "  please review the listener implementation end to end thoroughly and carefully";
        for &len in PREFIX_LENS {
            if let Some(s) = sample(p, len) {
                assert!(p.trim_start().starts_with(&s));
                assert!(s.chars().count() <= len);
            }
        }
    }

    #[test]
    fn multibyte_prompt_truncates_safely() {
        let p = "日本語のプロンプトです。これはテストのための長い文章になります。よろしくお願いします。".repeat(3);
        for &len in PREFIX_LENS {
            if let Some(s) = sample(&p, len) {
                assert!(s.chars().count() <= len);
            }
        }
        // Also exercise fingerprinting on it — must not panic.
        if let Some(s) = sample(&p, 60) {
            let _ = fingerprint(b"salt", &s);
        }
    }

    /// A prompt shorter than every `PREFIX_LENS` entry is sampled at its own
    /// length rather than refused (M2's clamp).
    #[test]
    fn short_prompt_is_sampled_at_its_own_length() {
        let p = "short prompt";
        let expected = p.chars().count();
        for &len in PREFIX_LENS {
            let s = sample(p, len).expect("a non-empty prompt always samples");
            assert_eq!(s.chars().count(), expected);
        }
    }

    #[test]
    fn empty_and_whitespace_prompts_are_not_sampled() {
        assert!(sample("", 60).is_none());
        assert!(sample("   \n\t ", 60).is_none());
    }

    /// A 40-char prompt observed once produces exactly one record (Task 3's
    /// intra-call dedup collapses all five `PREFIX_LENS` samples, since a
    /// prompt shorter than every length now samples identically at each),
    /// and that record's `len` is the actual sample length, not a nominal
    /// `PREFIX_LENS` entry.
    #[test]
    fn short_prompt_observed_once_yields_single_record_with_actual_len() {
        let mut o = Observations::default();
        let salt = b"salt";
        let p = "a prompt exactly forty characters long!";
        let expected_len = p.chars().count() as u16;
        o.observe(salt, "session-1", p);
        assert_eq!(o.records.len(), 1);
        let rec = o.records.values().next().unwrap();
        assert_eq!(rec.len, expected_len);
        assert_eq!(rec.n, 1);
    }

    /// A multibyte short prompt records the correct grapheme-agnostic char
    /// count and never panics on truncation.
    #[test]
    fn multibyte_short_prompt_records_char_count() {
        let mut o = Observations::default();
        let salt = b"salt";
        let p = "日本語のテスト";
        assert_eq!(p.chars().count(), 7);
        o.observe(salt, "session-1", p);
        assert_eq!(o.records.len(), 1);
        let rec = o.records.values().next().unwrap();
        assert_eq!(rec.len, 7);
    }

    /// M2 prereq: `normalize` collapses whitespace, so a prompt whose chars
    /// 61–70 are all whitespace makes `sample(60)` and `sample(70)`
    /// fingerprint identically. That collision must count once, not twice —
    /// reachable today, not just under the M2 clamp.
    #[test]
    fn whitespace_tail_collision_counts_once() {
        let mut o = Observations::default();
        let salt = b"salt";
        // 60 visible chars, then 15 whitespace chars, then more text — chars
        // 61-70 (the gap between PREFIX_LENS[0]=60 and [1]=70) are whitespace.
        let head = "a".repeat(60);
        let gap = " ".repeat(15);
        let p = format!("{head}{gap}more distinguishing text follows here for length");
        o.observe(salt, "session-1", &p);
        let fp60 = fingerprint(salt, &sample(&p, PREFIX_LENS[0]).unwrap());
        assert_eq!(o.records.get(&fp60).unwrap().n, 1);
    }

    /// Repeated retries of the same session's first-prompt read (the listener
    /// retries every few seconds until resolved) must not inflate its count.
    #[test]
    fn retry_reads_count_once() {
        let mut o = Observations::default();
        let salt = b"salt";
        let p = "please review the listener implementation end to end thoroughly";
        for _ in 0..5 {
            o.observe(salt, "session-1", p);
        }
        let fp = fingerprint(salt, &sample(p, PREFIX_LENS[0]).unwrap());
        assert_eq!(o.records.get(&fp).unwrap().n, 1);
    }

    #[test]
    fn two_sessions_same_opening_reach_two() {
        let mut o = Observations::default();
        let salt = b"salt";
        let p = "please review the listener implementation end to end thoroughly";
        assert!(o.observe(salt, "session-1", p));
        let fp = fingerprint(salt, &sample(p, PREFIX_LENS[0]).unwrap());
        // Force an artificial `first` so a later upsert overwriting it would
        // be caught — the entry API must only set `first` on insert.
        o.records.get_mut(&fp).unwrap().first = 1_000;
        assert!(o.observe(salt, "session-2", p));
        let rec = o.records.get(&fp).unwrap();
        assert_eq!(rec.n, 2);
        assert_eq!(rec.first, 1_000, "first must not be overwritten");
        assert!(rec.last >= 1_000);
    }

    #[test]
    fn prune_drops_expired_keeps_fresh() {
        let mut o = Observations::default();
        let now = 100_000_000u64;
        o.records.insert(
            "fp_old".into(),
            Observation {
                len: 60,
                n: 1,
                first: now - 31 * 86_400,
                last: now - 31 * 86_400,
            },
        );
        o.records.insert(
            "fp_fresh".into(),
            Observation {
                len: 60,
                n: 1,
                first: now - 29 * 86_400,
                last: now - 29 * 86_400,
            },
        );
        assert!(o.prune(30, now));
        assert!(!o.records.contains_key("fp_old"));
        assert!(o.records.contains_key("fp_fresh"));
    }

    #[test]
    fn store_json_contains_no_prompt_text() {
        let mut o = Observations::default();
        let salt = b"salt";
        let phrase = "the quick brown fox jumps over the lazy dog in a distinctive sentence";
        o.observe(salt, "s1", phrase);
        let text = o.to_json().to_string();
        for word in phrase.split_whitespace().filter(|w| w.len() >= 4) {
            assert!(
                !text.contains(word),
                "prompt word {word:?} leaked into store JSON"
            );
        }
    }

    /// M1: one malformed record must not discard the entire history — the
    /// pre-fix `from_value::<HashMap<String, Observation>>` failed the whole
    /// parse on a single bad entry.
    #[test]
    fn from_json_keeps_good_records_beside_bad() {
        let v = serde_json::json!({
            "good": {"len": 60, "n": 3, "first": 100, "last": 200},
            "bad": {"len": "not-a-number"},
        });
        let o = Observations::from_json(v);
        assert_eq!(o.records.len(), 1, "only the malformed entry is dropped");
        assert!(o.records.contains_key("good"));
        assert!(!o.records.contains_key("bad"));
    }

    /// A record missing a field (e.g. a truncated write) still loads — the
    /// container-level `#[serde(default)]` fills the gap rather than
    /// rejecting the whole entry.
    #[test]
    fn observation_missing_field_defaults() {
        let v = serde_json::json!({
            "fp": {"n": 3, "first": 100, "last": 200},
        });
        let o = Observations::from_json(v);
        let rec = o.records.get("fp").expect("entry must still load");
        assert_eq!(rec.len, 0, "missing len defaults to 0");
        assert_eq!(rec.n, 3);
    }

    /// A top-level shape that isn't an object (array, null) yields an empty
    /// store rather than panicking.
    #[test]
    fn from_json_tolerates_non_object() {
        for v in [serde_json::json!([]), serde_json::json!(null)] {
            let o = Observations::from_json(v);
            assert!(o.records.is_empty());
        }
    }

    /// Purging via the 100-length fingerprint of a long, repeatedly-observed
    /// prompt drops every prefix-related record (all five `PREFIX_LENS`
    /// samples of the same opening), while an unrelated cluster survives.
    #[test]
    fn purge_family_drops_the_whole_prefix_chain() {
        let mut o = Observations::default();
        let salt = b"salt";
        let long_prompt = "please review the listener implementation end to end thoroughly \
            and carefully, checking every code path for correctness and safety concerns";
        o.observe(salt, "s1", long_prompt);
        let unrelated = "a completely different opening about something else entirely today";
        o.observe(salt, "s2", unrelated);

        let fp100 = fingerprint(salt, &sample(long_prompt, 100).unwrap());
        let removed = o.purge_family(&fp100);
        assert!(removed >= 1, "at least the anchor itself is removed");
        for &len in PREFIX_LENS {
            if let Some(s) = sample(long_prompt, len) {
                let fp = fingerprint(salt, &s);
                assert!(
                    !o.records.contains_key(&fp),
                    "prefix-related record for len {len} must be gone"
                );
            }
        }
        let unrelated_fp = fingerprint(salt, &sample(unrelated, PREFIX_LENS[0]).unwrap());
        assert!(
            o.records.contains_key(&unrelated_fp),
            "unrelated cluster survives"
        );
    }

    /// A dismissal never reaches disk — it's a run-only side table, matching
    /// the sample table's lifetime.
    #[test]
    fn dismissed_survives_only_the_run() {
        let mut o = Observations::default();
        let salt = b"salt";
        let p = "please review the listener implementation end to end thoroughly";
        o.observe(salt, "s1", p);
        let fp = fingerprint(salt, &sample(p, PREFIX_LENS[0]).unwrap());
        o.dismiss(&fp, 1);
        assert_eq!(o.dismissed_at(&fp), Some(1));

        let reloaded = Observations::from_json(o.to_json());
        assert_eq!(
            reloaded.dismissed_at(&fp),
            None,
            "dismissed is empty after a round-trip"
        );
    }
}
