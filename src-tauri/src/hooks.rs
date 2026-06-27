//! Hook installer: merge Beacon's HTTP hooks into `~/.claude/settings.json`
//! without disturbing the user's other hooks, and remove only ours on
//! uninstall.
//!
//! Beacon's hook entries are identified structurally — an `http` hook whose URL
//! points at our loopback `/hook` endpoint — so we never need to write a custom
//! marker into the user's settings, and uninstall is precise.

use serde_json::{json, Map, Value};
use std::path::PathBuf;

/// Hook events Beacon wires up. These drive the state engine (see CLAUDE.md).
pub const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PostToolUse",
    "Notification",
    "Stop",
    "SubagentStop",
    "SessionEnd",
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

/// One hook group for a single event: an empty matcher with our http hook.
fn our_group(port: u16) -> Value {
    json!({
        "matcher": "",
        "hooks": [
            {
                "type": "http",
                "url": endpoint(port),
                // Generous timeout; our listener answers instantly anyway.
                "timeout": 10
            }
        ]
    })
}

/// The full `{ "hooks": { ... } }` block Beacon installs — used both for the
/// copy-paste fallback and as the source of truth for the merge.
pub fn hook_block_value(port: u16) -> Value {
    let mut hooks = Map::new();
    for ev in EVENTS {
        hooks.insert(ev.to_string(), Value::Array(vec![our_group(port)]));
    }
    json!({ "hooks": hooks })
}

/// Pretty-printed copy-paste string of the hook block.
pub fn hook_block_string(port: u16) -> String {
    serde_json::to_string_pretty(&hook_block_value(port))
        .unwrap_or_else(|_| "{}".to_string())
}

/// Is this individual hook object one of Beacon's? (http hook → loopback /hook)
fn is_our_hook(hook: &Value) -> bool {
    if hook.get("type").and_then(Value::as_str) != Some("http") {
        return false;
    }
    match hook.get("url").and_then(Value::as_str) {
        Some(url) => {
            url.ends_with("/hook") && (url.contains("127.0.0.1") || url.contains("localhost"))
        }
        None => false,
    }
}

/// Strip Beacon's hooks out of one event's array of groups, in place. Drops a
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
    // Back up the existing file once before we touch it.
    if path.exists() {
        let backup = path.with_extension("json.beacon.bak");
        let _ = std::fs::copy(path, backup);
    }
    let body = serde_json::to_string_pretty(&Value::Object(settings.clone()))
        .map_err(|e| format!("could not serialize settings: {e}"))?;
    std::fs::write(path, body + "\n").map_err(|e| format!("could not write settings.json: {e}"))
}

/// Are any of Beacon's hooks currently present in settings.json? Port-agnostic
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

/// Merge Beacon's hooks into settings.json. Idempotent: any prior Beacon hooks
/// are removed first, then our fresh block is added. Other hooks untouched.
pub fn install(port: u16) -> Result<PathBuf, String> {
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
        // Remove any stale Beacon entries, then add the current one.
        strip_our_hooks(groups);
        groups.push(our_group(port));
    }

    write_settings(&path, &settings)?;
    Ok(path)
}

/// Remove only Beacon's hook entries, leaving the rest of settings.json intact
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
            groups.push(our_group(4317));
        }

        // The user's command hook on Stop survives alongside ours.
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert!(stop.iter().any(|g| g["hooks"][0]["type"] == "command"));
        assert!(stop.iter().any(|g| is_our_hook(&g["hooks"][0])));

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
    fn block_string_has_all_events() {
        let s = hook_block_string(4317);
        for ev in EVENTS {
            assert!(s.contains(ev), "missing {ev}");
        }
        assert!(s.contains("127.0.0.1:4317/hook"));
    }
}
