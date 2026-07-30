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
/// `None` if the trimmed prompt is shorter than `len` — nothing to sample.
pub fn sample(prompt: &str, len: usize) -> Option<String> {
    let trimmed = prompt.trim_start();
    if trimmed.chars().count() < len {
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
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Observation {
    /// Which prefix length produced this fingerprint. A later de-duplication
    /// pass needs it; it reveals nothing about the prompt itself.
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
        for &len in PREFIX_LENS {
            let Some(s) = sample(prompt, len) else {
                continue;
            };
            let fp = fingerprint(salt, &s);
            self.samples.insert(fp.clone(), s);
            let rec = self.records.entry(fp).or_insert_with(|| Observation {
                len: len as u16,
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

    /// Load from a persisted value, tolerating a missing/garbage shape by
    /// falling back to an empty store rather than panicking.
    pub fn from_json(v: serde_json::Value) -> Self {
        let records = serde_json::from_value::<HashMap<String, Observation>>(v).unwrap_or_default();
        Observations {
            records,
            samples: HashMap::new(),
            seen: HashSet::new(),
            dirty: false,
        }
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
                assert_eq!(s.chars().count(), len);
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

    #[test]
    fn short_prompt_yields_no_sample_at_long_lengths() {
        let p = "short prompt";
        assert!(sample(p, 60).is_none());
        assert!(sample(p, 120).is_none());
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

    #[test]
    fn from_json_tolerates_garbage() {
        for v in [
            serde_json::json!([]),
            serde_json::json!(null),
            serde_json::json!({"fp": {"bad": "shape"}}),
        ] {
            let o = Observations::from_json(v);
            assert!(o.records.is_empty());
        }
    }
}
