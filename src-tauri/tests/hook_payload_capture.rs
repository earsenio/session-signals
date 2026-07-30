//! Decision 3 (PRD): does any wired hook payload field carry the session's
//! first prompt, obviating the transcript-head read `descriptor::first_prompt`
//! does today?
//!
//! Two parts:
//! 1. A committed, **keys-only** sanitized capture of the wired events'
//!    schema — sourced from CLAUDE.md's two empirically-verified blocks (the
//!    only empirical record this repo has; not fabricated, and not gated on a
//!    live-capture harness this codebase doesn't build — see `capture.rs`,
//!    which captures a *synthetic* `BeaconTerminal` event, not the real hook
//!    traffic). Every value is a placeholder; the keys are the measurement.
//! 2. A verdict table, checked against those bodies so the recorded answer
//!    cannot silently drift from the committed data.
//!
//! **This measurement cannot block and gates nothing.** A "yes" would unlock
//! a future fast path (skip the transcript read for that event); a "no"
//! documents its absence. Both outcomes are recorded, never just one.

use serde_json::Value;

/// The wired subset that drives state (CLAUDE.md's "Hook contract"). Every
/// value is a placeholder — `"<redacted>"` / `null` — because a value here is
/// exactly the privacy risk this whole feature exists to avoid capturing.
/// Field sets come only from what this repo has already empirically verified
/// (CLAUDE.md's two "Verified" blocks) plus `engine::HookEvent`'s own field
/// docs, which assert `tool_name` is genuinely present on Pre/PostToolUse.
const SANITIZED_BODIES: &[(&str, &str)] = &[
    (
        "SessionStart",
        r#"{"hook_event_name":"SessionStart","session_id":"<redacted>","cwd":"<redacted>","transcript_path":"<redacted>"}"#,
    ),
    (
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"<redacted>","cwd":"<redacted>","transcript_path":"<redacted>"}"#,
    ),
    (
        "PreToolUse",
        r#"{"hook_event_name":"PreToolUse","session_id":"<redacted>","cwd":"<redacted>","transcript_path":"<redacted>","tool_name":"<redacted>","agent_id":null,"agent_type":null}"#,
    ),
    (
        "PostToolUse",
        r#"{"hook_event_name":"PostToolUse","session_id":"<redacted>","cwd":"<redacted>","transcript_path":"<redacted>","tool_name":"<redacted>","agent_id":null,"agent_type":null}"#,
    ),
    (
        "Notification",
        r#"{"hook_event_name":"Notification","session_id":"<redacted>","cwd":"<redacted>","transcript_path":"<redacted>","notification_type":"<redacted>"}"#,
    ),
    (
        "Stop",
        r#"{"hook_event_name":"Stop","session_id":"<redacted>","cwd":"<redacted>","transcript_path":"<redacted>"}"#,
    ),
    (
        "SubagentStop",
        r#"{"hook_event_name":"SubagentStop","session_id":"<redacted>","cwd":"<redacted>","transcript_path":"<redacted>","agent_id":"<redacted>","agent_type":"<redacted>"}"#,
    ),
    (
        "SessionEnd",
        r#"{"hook_event_name":"SessionEnd","session_id":"<redacted>","cwd":"<redacted>","transcript_path":"<redacted>"}"#,
    ),
];

/// Decision 3's recorded verdict, one row per wired event: does the
/// currently-verified schema carry the first prompt? Checked below against
/// `SANITIZED_BODIES`' actual keys — this table cannot say "no" while a
/// prompt-shaped key is present in the committed body, or vice versa.
///
/// **Caveat, recorded rather than silently assumed**: Claude Code's public
/// hooks documentation describes a `prompt` field on `UserPromptSubmit`
/// carrying the *current* turn's typed text. This repo has never empirically
/// captured a live `UserPromptSubmit` body against its own listener (only the
/// subagent `agent_id` finding was captured that way — see CLAUDE.md), so
/// that field is absent from `SANITIZED_BODIES` above rather than asserted as
/// fact. Even if present, it would answer "carries *a* prompt", not
/// necessarily "carries the *first* prompt" — a headless `--print` spawn's
/// injected opening may never fire `UserPromptSubmit` at all. Verifying that
/// is a good candidate for a future, dedicated capture; it is not claimed
/// here.
const PROMPT_FIELD_VERDICT: &[(&str, bool)] = &[
    ("SessionStart", false),
    ("UserPromptSubmit", false),
    ("PreToolUse", false),
    ("PostToolUse", false),
    ("Notification", false),
    ("Stop", false),
    ("SubagentStop", false),
    ("SessionEnd", false),
];

/// Keys that could plausibly carry free-form prompt text, if one were
/// present. An explicit list, not a heuristic over all strings, so the
/// cross-check below is unambiguous.
const PROMPT_SHAPED_KEYS: &[&str] = &["prompt", "last_prompt", "first_prompt", "message"];

#[test]
fn hook_payload_prompt_presence_is_recorded() {
    assert_eq!(
        SANITIZED_BODIES.len(),
        PROMPT_FIELD_VERDICT.len(),
        "every wired event must have both a sanitized body and a verdict row"
    );

    for (event, verdict) in PROMPT_FIELD_VERDICT {
        let (_, body) = SANITIZED_BODIES
            .iter()
            .find(|(name, _)| name == event)
            .unwrap_or_else(|| panic!("no sanitized body committed for {event}"));
        let v: Value = serde_json::from_str(body).expect("committed body must be valid JSON");
        let obj = v.as_object().expect("body is a JSON object");
        let has_prompt_key = obj.keys().any(|k| PROMPT_SHAPED_KEYS.contains(&k.as_str()));
        assert_eq!(
            has_prompt_key, *verdict,
            "{event}: verdict table says carries_prompt={verdict}, but the sanitized \
             body's keys disagree — the recorded verdict must be source, not folklore"
        );
    }

    // Decision 3, recorded either way: within the currently empirically-
    // verified schema, no wired event carries the session's first prompt
    // directly. `descriptor::first_prompt`'s transcript-head read stays
    // load-bearing; there is no fast path to retire it today.
    assert!(
        PROMPT_FIELD_VERDICT.iter().all(|(_, carries)| !carries),
        "record: no wired event's currently-verified schema carries the first prompt"
    );
}

/// Every event this measurement is scoped to (CLAUDE.md's state-driving
/// subset) has exactly one sanitized body — catches this measurement
/// silently drifting from the wired event list.
#[test]
fn sanitized_bodies_cover_the_state_driving_events() {
    const STATE_DRIVING_EVENTS: &[&str] = &[
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "Notification",
        "Stop",
        "SubagentStop",
        "SessionEnd",
    ];
    for ev in STATE_DRIVING_EVENTS {
        assert!(
            SANITIZED_BODIES.iter().any(|(name, _)| name == ev),
            "missing a sanitized body for wired event {ev}"
        );
    }
    assert_eq!(SANITIZED_BODIES.len(), STATE_DRIVING_EVENTS.len());
}

/// Sanity: every committed body actually parses, and none of them carry
/// anything other than the placeholder — a literal secret pasted in here by
/// mistake would still be a string value, but this at least catches a
/// non-placeholder string slipping in during a future edit.
#[test]
fn sanitized_bodies_carry_only_placeholders() {
    for (event, body) in SANITIZED_BODIES {
        let v: Value = serde_json::from_str(body)
            .unwrap_or_else(|e| panic!("{event}: committed body is not valid JSON: {e}"));
        let obj = v.as_object().expect("body is a JSON object");
        for (key, value) in obj {
            // `hook_event_name` is the structural discriminant, not user
            // data — its real value is the point of the field.
            if key == "hook_event_name" {
                continue;
            }
            if let Some(s) = value.as_str() {
                assert_eq!(
                    s, "<redacted>",
                    "{event}.{key}: expected the placeholder, found a real-looking value"
                );
            }
        }
    }
}
