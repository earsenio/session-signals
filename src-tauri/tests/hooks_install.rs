//! Integration test for the real filesystem install/uninstall path, run against
//! a sandboxed HOME so the developer's actual ~/.claude/settings.json is never
//! touched. Verifies the non-destructive merge and the precise uninstall.

use beacon_lib::hooks;
use serde_json::Value;

/// Point HOME at a throwaway dir and seed a settings.json with a foreign hook
/// plus an unrelated top-level key.
fn sandbox(test_name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("beacon-test-{test_name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".claude")).unwrap();
    std::env::set_var("HOME", &dir);

    let seed = r#"{
      "model": "opus",
      "hooks": {
        "Stop": [
          { "matcher": "", "hooks": [ { "type": "command", "command": "echo bye" } ] }
        ]
      }
    }"#;
    std::fs::write(dir.join(".claude").join("settings.json"), seed).unwrap();
    dir
}

#[test]
fn install_then_uninstall_is_non_destructive() {
    let dir = sandbox("install");
    let settings = dir.join(".claude").join("settings.json");

    // Install Session Signals' hooks, including the terminal-capture command hook.
    let capture = "sh '/tmp/beacon-capture.sh'";
    hooks::install(4317, "tok-xyz", Some(capture)).expect("install ok");
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).expect("valid JSON");

    // Foreign key + foreign command hook survive.
    assert_eq!(v["model"], "opus");
    let stop = v["hooks"]["Stop"].as_array().unwrap();
    assert!(stop.iter().any(|g| g["hooks"][0]["type"] == "command"));
    // Our http hook was added to Stop and all other events.
    assert!(stop
        .iter()
        .any(|g| g["hooks"][0]["url"] == "http://127.0.0.1:4317/hook"));
    for ev in hooks::EVENTS {
        assert!(
            v["hooks"].get(ev).is_some(),
            "event {ev} missing after install"
        );
    }

    // The auth-token header is written into our hook.
    assert_eq!(
        v["hooks"]["Stop"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["hooks"][0]["url"] == "http://127.0.0.1:4317/hook")
            .unwrap()["hooks"][0]["headers"]["X-Beacon-Token"],
        "tok-xyz"
    );

    // The capture command hook rides on SessionStart alongside the http hook.
    let ss = v["hooks"]["SessionStart"].as_array().unwrap();
    assert!(
        ss.iter().any(|g| g["hooks"][0]["type"] == "command"
            && g["hooks"][0]["command"]
                .as_str()
                .map(|c| c.contains("beacon-capture"))
                .unwrap_or(false)),
        "capture command hook missing on SessionStart"
    );

    // Re-install must be idempotent (no duplicate Session Signals entries on Stop).
    hooks::install(4317, "tok-xyz", Some(capture)).expect("reinstall ok");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let beacon_on_stop = v["hooks"]["Stop"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|g| g["hooks"][0]["url"] == "http://127.0.0.1:4317/hook")
        .count();
    assert_eq!(beacon_on_stop, 1, "reinstall duplicated Session Signals hook");

    // Uninstall removes only ours.
    hooks::uninstall(4317).expect("uninstall ok");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v["model"], "opus");
    let stop = v["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 1);
    assert_eq!(stop[0]["hooks"][0]["type"], "command");
    // Events that were purely Session Signals' are gone.
    assert!(v["hooks"].get("SessionStart").is_none());

    let _ = std::fs::remove_dir_all(&dir);
}
