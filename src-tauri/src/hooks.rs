//! Hook installer: merge Session Signals' HTTP hooks into `~/.claude/settings.json`
//! without disturbing the user's other hooks, and remove only ours on
//! uninstall.
//!
//! Session Signals' hook entries are identified structurally — an `http` hook whose URL
//! points at our loopback `/hook` endpoint — so we never need to write a custom
//! marker into the user's settings, and uninstall is precise.

use serde_json::{json, Map, Value};
use std::path::PathBuf;

/// Hook events Session Signals wires up. These drive the state engine (see CLAUDE.md).
/// Grouped by the role each plays in the state machine (see `engine::apply`):
/// session lifecycle, work-start (→ Working), heartbeats (keep Working), and
/// terminal (→ Ready / NeedsYou). Verified against Claude Code 2.1.195; all
/// support `type:"http"` hooks except `SessionStart` (see note below).
pub const EVENTS: &[&str] = &[
    // Session lifecycle.
    "SessionStart",
    "SessionEnd",
    // Work-start → Working.
    "UserPromptSubmit",
    "UserPromptExpansion",
    "PreToolUse",
    "SubagentStart",
    "PreCompact",
    // Heartbeats → keep current Working state alive.
    "PostToolUse",
    "PostToolUseFailure",
    "PostToolBatch",
    // Terminal → Ready.
    "Stop",
    "StopFailure",
    "SubagentStop",
    "PostCompact",
    // Blocked on the user → NeedsYou (filtered by notification_type).
    "Notification",
];

/// The localhost endpoint Claude Code POSTs each hook to.
pub fn endpoint(port: u16) -> String {
    format!("http://127.0.0.1:{port}/hook")
}

/// Path to the user-level Claude Code settings file.
pub fn settings_path() -> Result<PathBuf, String> {
    let home = home_dir().ok_or_else(|| "could not determine home directory".to_string())?;
    Ok(home.join(".claude").join("settings.json"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// One hook group for a single event: an empty matcher with our http hook,
/// carrying the auth token as a header so the listener can tell our hooks apart
/// from any other local process posting to the port.
fn our_group(port: u16, token: &str) -> Value {
    json!({
        "matcher": "",
        "hooks": [
            {
                "type": "http",
                "url": endpoint(port),
                "headers": { crate::token::HEADER: token },
                // Generous timeout; our listener answers instantly anyway.
                "timeout": 10
            }
        ]
    })
}

/// The full `{ "hooks": { ... } }` block Session Signals installs — used both for the
/// copy-paste fallback and as the source of truth for the merge.
pub fn hook_block_value(port: u16, token: &str) -> Value {
    let mut hooks = Map::new();
    for ev in EVENTS {
        hooks.insert(ev.to_string(), Value::Array(vec![our_group(port, token)]));
    }
    json!({ "hooks": hooks })
}

/// Pretty-printed copy-paste string of the hook block.
pub fn hook_block_string(port: u16, token: &str) -> String {
    serde_json::to_string_pretty(&hook_block_value(port, token))
        .unwrap_or_else(|_| "{}".to_string())
}

/// Is this individual hook object one of Session Signals'? We recognize two shapes
/// structurally (no marker written into the user's file):
/// - an `http` hook posting to our loopback `/hook` endpoint, and
/// - the `command` capture hook, whose command contains the `beacon-capture`
///   marker (see `capture.rs`).
fn is_our_hook(hook: &Value) -> bool {
    match hook.get("type").and_then(Value::as_str) {
        Some("http") => match hook.get("url").and_then(Value::as_str) {
            Some(url) => {
                url.ends_with("/hook") && (url.contains("127.0.0.1") || url.contains("localhost"))
            }
            None => false,
        },
        Some("command") => hook
            .get("command")
            .and_then(Value::as_str)
            .map(|c| c.contains(crate::capture::MARKER))
            .unwrap_or(false),
        _ => false,
    }
}

/// The capture command hook group for `SessionStart` (Feature 2). `matcher` is
/// empty like our http groups; a slightly longer timeout covers the process-tree
/// walk + the loopback POST.
fn capture_group(command: &str) -> Value {
    json!({
        "matcher": "",
        "hooks": [
            {
                "type": "command",
                "command": command,
                "timeout": 5
            }
        ]
    })
}

/// Strip Session Signals' hooks out of one event's array of groups, in place. Drops a
/// group entirely only if it had *nothing but* our hooks. Returns whether the
/// array ended up empty (so the caller can remove the key).
fn strip_our_hooks(groups: &mut Vec<Value>) {
    for group in groups.iter_mut() {
        if let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) {
            hooks.retain(|h| !is_our_hook(h));
        }
    }
    // Remove groups whose hook list is now empty (i.e. were purely ours).
    groups.retain(|g| {
        g.get("hooks")
            .and_then(Value::as_array)
            .map(|h| !h.is_empty())
            .unwrap_or(true)
    });
}

/// Load settings.json as an object. Missing file → empty object. A present but
/// unparseable file is an error (we refuse to clobber it).
fn load_settings(path: &PathBuf) -> Result<Map<String, Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Map::new()),
        Ok(text) => serde_json::from_str::<Value>(&text)
            .map_err(|e| format!("settings.json is not valid JSON: {e}"))
            .and_then(|v| match v {
                Value::Object(m) => Ok(m),
                _ => Err("settings.json is not a JSON object".to_string()),
            }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(format!("could not read settings.json: {e}")),
    }
}

fn write_settings(path: &PathBuf, settings: &Map<String, Value>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    // Keep a copy of the file as it was before this write. Note this is a
    // "previous version", not a pristine pre-install snapshot: every write
    // refreshes it, so it always holds the state from exactly one write ago.
    if path.exists() {
        let backup = path.with_extension("json.beacon.bak");
        let _ = std::fs::copy(path, backup);
    }
    let body = serde_json::to_string_pretty(&Value::Object(settings.clone()))
        .map_err(|e| format!("could not serialize settings: {e}"))?;
    // Write-then-rename so a crash mid-write can never leave settings.json
    // truncated or half-written — it's the user's entire Claude Code config.
    let tmp = path.with_extension("json.beacon.tmp");
    std::fs::write(&tmp, body + "\n").map_err(|e| format!("could not write settings.json: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("could not write settings.json: {e}"))
}

/// Are any of Session Signals' hooks currently present in settings.json? Port-agnostic
/// (our hooks are identified structurally), so this stays true across a port
/// change until an explicit uninstall.
pub fn is_installed() -> bool {
    let path = match settings_path() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let settings = match load_settings(&path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    settings
        .get("hooks")
        .and_then(Value::as_object)
        .map(|hooks| {
            hooks.values().any(|groups| {
                groups
                    .as_array()
                    .map(|gs| {
                        gs.iter().any(|g| {
                            g.get("hooks")
                                .and_then(Value::as_array)
                                .map(|hs| hs.iter().any(is_our_hook))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Do Session Signals' installed http hooks carry an auth-token header that doesn't match
/// `token` (missing, or a stale value)? When true, the token-enforcing listener
/// 401s every one of these hooks and silently tracks nothing — exactly the
/// breakage an upgrade to a token build causes when it leaves pre-token hooks in
/// place. Used at startup to decide whether to auto-repair.
///
/// Only our `http` hooks are checked: the capture `command` hook (also ours)
/// intentionally carries no header. Returns false when Session Signals has no http hooks
/// installed (the not-installed case is handled by the first-run flow) or when
/// every one already matches.
pub fn needs_token_repair(token: &str) -> bool {
    let path = match settings_path() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let settings = match load_settings(&path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let hooks = match settings.get("hooks").and_then(Value::as_object) {
        Some(h) => h,
        None => return false,
    };
    for groups in hooks.values() {
        let groups = match groups.as_array() {
            Some(g) => g,
            None => continue,
        };
        for g in groups {
            let hs = match g.get("hooks").and_then(Value::as_array) {
                Some(h) => h,
                None => continue,
            };
            for h in hs {
                // Only our http hooks carry the token header.
                if h.get("type").and_then(Value::as_str) == Some("http") && is_our_hook(h) {
                    let current = h
                        .get("headers")
                        .and_then(Value::as_object)
                        .and_then(|hdrs| hdrs.get(crate::token::HEADER))
                        .and_then(Value::as_str);
                    if current != Some(token) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Merge Session Signals' hooks into settings.json. Idempotent: any prior Session Signals hooks
/// are removed first, then our fresh block is added. Other hooks untouched.
pub fn install(port: u16, token: &str, capture_cmd: Option<&str>) -> Result<PathBuf, String> {
    let path = settings_path()?;
    let mut settings = load_settings(&path)?;

    let hooks = settings
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = hooks
        .as_object_mut()
        .ok_or_else(|| "settings.json \"hooks\" is not an object".to_string())?;

    for ev in EVENTS {
        let arr = hooks
            .entry(ev.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        let groups = arr
            .as_array_mut()
            .ok_or_else(|| format!("settings.json hooks.{ev} is not an array"))?;
        // Remove any stale Session Signals entries (http *and* the capture command), then
        // add the current ones.
        strip_our_hooks(groups);
        groups.push(our_group(port, token));
        // The terminal-capture command hook rides alongside the http hook on
        // SessionStart only (that's when we resolve the owning terminal).
        if ev == &"SessionStart" {
            if let Some(cmd) = capture_cmd {
                groups.push(capture_group(cmd));
            }
        }
    }

    write_settings(&path, &settings)?;
    Ok(path)
}

/// Remove only Session Signals' hook entries, leaving the rest of settings.json intact
/// and valid. Empty event arrays and an empty `hooks` object are cleaned up.
pub fn uninstall(_port: u16) -> Result<PathBuf, String> {
    let path = settings_path()?;
    let mut settings = load_settings(&path)?;

    if let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) {
        let event_keys: Vec<String> = hooks.keys().cloned().collect();
        for ev in event_keys {
            if let Some(groups) = hooks.get_mut(&ev).and_then(Value::as_array_mut) {
                strip_our_hooks(groups);
                if groups.is_empty() {
                    hooks.remove(&ev);
                }
            }
        }
        if hooks.is_empty() {
            settings.remove("hooks");
        }
    }

    write_settings(&path, &settings)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_preserves_foreign_hooks_and_uninstall_reverts() {
        // Start with a user file that already has an unrelated command hook.
        let mut settings: Map<String, Value> = serde_json::from_str(
            r#"{
              "model": "opus",
              "hooks": {
                "Stop": [
                  { "matcher": "", "hooks": [ { "type": "command", "command": "echo hi" } ] }
                ]
              }
            }"#,
        )
        .unwrap();

        // Simulate install's merge logic directly on the map.
        let hooks = settings.get_mut("hooks").unwrap().as_object_mut().unwrap();
        for ev in EVENTS {
            let arr = hooks
                .entry(ev.to_string())
                .or_insert_with(|| Value::Array(vec![]));
            let groups = arr.as_array_mut().unwrap();
            strip_our_hooks(groups);
            groups.push(our_group(4317, "tok"));
        }

        // The user's command hook on Stop survives alongside ours.
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert!(stop.iter().any(|g| g["hooks"][0]["type"] == "command"));
        assert!(stop.iter().any(|g| is_our_hook(&g["hooks"][0])));
        // Our hook carries the auth-token header.
        let ours = stop.iter().find(|g| is_our_hook(&g["hooks"][0])).unwrap();
        assert_eq!(ours["hooks"][0]["headers"][crate::token::HEADER], "tok");

        // Now uninstall: strip ours from every event.
        let hooks = settings.get_mut("hooks").unwrap().as_object_mut().unwrap();
        let keys: Vec<String> = hooks.keys().cloned().collect();
        for ev in keys {
            if let Some(groups) = hooks.get_mut(&ev).and_then(Value::as_array_mut) {
                strip_our_hooks(groups);
                if groups.is_empty() {
                    hooks.remove(&ev);
                }
            }
        }

        // Only the user's original Stop/command hook remains; model untouched.
        assert_eq!(settings["model"], "opus");
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1);
        assert_eq!(stop[0]["hooks"][0]["type"], "command");
        // Events that were purely ours got removed.
        assert!(settings["hooks"].get("SessionStart").is_none());
    }

    #[test]
    fn token_repair_detects_missing_and_mismatched_headers() {
        // A pre-token Session Signals http hook (no headers) — the exact stale shape an
        // upgrade leaves behind.
        let stale: Value = serde_json::from_str(
            r#"{ "type": "http", "url": "http://127.0.0.1:4317/hook", "timeout": 10 }"#,
        )
        .unwrap();
        assert!(is_our_hook(&stale));
        // No header at all → mismatch against any token.
        assert!(needs_repair_for(&stale, "tok"));

        // Wrong token value → mismatch.
        let wrong = our_group(4317, "old-token");
        let wrong_hook = &wrong["hooks"][0];
        assert!(needs_repair_for(wrong_hook, "new-token"));

        // Correct token → no repair.
        let good = our_group(4317, "tok");
        let good_hook = &good["hooks"][0];
        assert!(!needs_repair_for(good_hook, "tok"));

        // A foreign hook is never ours, so never triggers repair.
        let foreign: Value =
            serde_json::from_str(r#"{ "type": "command", "command": "echo hi" }"#).unwrap();
        assert!(!needs_repair_for(&foreign, "tok"));
    }

    /// Mirror of `needs_token_repair`'s per-hook check, for unit-testing single
    /// hook objects without writing a settings.json. (The public function walks a
    /// real file; this isolates the matching logic.)
    fn needs_repair_for(hook: &Value, token: &str) -> bool {
        if hook.get("type").and_then(Value::as_str) == Some("http") && is_our_hook(hook) {
            let current = hook
                .get("headers")
                .and_then(Value::as_object)
                .and_then(|h| h.get(crate::token::HEADER))
                .and_then(Value::as_str);
            return current != Some(token);
        }
        false
    }

    #[test]
    fn block_string_has_all_events() {
        let s = hook_block_string(4317, "secret-tok");
        for ev in EVENTS {
            assert!(s.contains(ev), "missing {ev}");
        }
        assert!(s.contains("127.0.0.1:4317/hook"));
        // The copy-paste fallback carries the token too.
        assert!(s.contains("secret-tok"));
        assert!(s.contains(crate::token::HEADER));
    }
}
