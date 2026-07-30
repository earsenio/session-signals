// Mirrors the Rust `Proposal` (src-tauri/src/proposals.rs). Field names are
// snake_case to match the serde shape the `list_proposals` command returns.

import type { SessionView } from "./types";

export interface Proposal {
  /// Opaque cluster id — what `accept_proposal` / `dismiss_proposal` /
  /// `never_suggest_proposal` take. Never shown to the user.
  fingerprint: string;
  /// The literal opening a rule would be written from. Live in memory only —
  /// a cluster with no live sample is never returned as a proposal.
  sample: string;
  /// Chars in `sample`.
  len: number;
  /// Sessions in this cluster (may be larger than `matching.length` — see
  /// `matching`'s doc).
  count: number;
  /// Unix **seconds** (not milliseconds) — mirrors the Rust `u64` wall-clock
  /// timestamp.
  first_seen: number;
  last_seen: number;
  /// Currently-visible sessions this rule would hide. May be shorter than
  /// `count`: `count` groups on the whitespace-normalized form while a rule
  /// is a literal prefix, and only sessions live *right now* can appear.
  matching: SessionView[];
}
