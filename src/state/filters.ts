// Mirrors the Rust `HiddenSession` / `FilterStatus` (src-tauri/src/engine.rs,
// src-tauri/src/lib.rs). Field names are snake_case to match the serde shape
// the `filter_status` command returns.

import type { SessionView } from "./types";
import type { IgnoreMatcher } from "./config";

export interface HiddenSession {
  session: SessionView;
  /// The first matching rule, in `session_hidden`'s own evaluation order
  /// (cwd before first-prompt). Not a claim that no other rule also matches.
  rule: IgnoreMatcher;
}

export interface FilterStatus {
  hidden_count: number;
  /// How many times the reveal-on-block valve fired this run. Non-zero
  /// falsifies the "headless never blocks" premise.
  reveal_count: number;
  hidden: HiddenSession[];
  /// Live observation records. Count only — never sample text.
  observed_clusters: number;
}
