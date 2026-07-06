//! Listener auth token.
//!
//! A random secret shared between Session Signals and the hooks it installs. Every hook
//! POST must carry it as the `X-Beacon-Token` header; the listener rejects any
//! request that doesn't match (see `listener.rs`). This stops *other* local
//! processes from spoofing session state into Session Signals — loopback binding alone
//! doesn't, since any local program can reach `127.0.0.1`.
//!
//! The token is generated once on first run and persisted in the same
//! `beacon.json` store as the config (under a separate key, since it's a secret,
//! not a user-facing setting). `regenerate` mints a fresh one; the caller is
//! responsible for re-running the hook installer so `settings.json` and the live
//! listener stay in sync.

use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

const STORE_FILE: &str = "beacon.json";
const TOKEN_KEY: &str = "auth_token";

/// HTTP header the hooks send and the listener checks.
pub const HEADER: &str = "X-Beacon-Token";

/// Mint a new URL-safe token: 32 random bytes rendered as 64 hex chars. Hex is
/// inherently URL/header-safe (no quoting needed in JSON or HTTP). Fails closed:
/// if the OS CSPRNG is unavailable (effectively impossible on the desktop OSes
/// we ship to), we abort rather than mint a predictable secret.
pub fn generate() -> String {
    use std::fmt::Write;
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS CSPRNG unavailable — refusing to mint a weak token");
    let mut s = String::with_capacity(64);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Read the persisted token, generating + saving one on first run. Always
/// returns a usable token (a save failure is tolerated — we keep the in-memory
/// value so the running session still works).
pub fn load_or_create(app: &AppHandle) -> String {
    if let Ok(store) = app.store(STORE_FILE) {
        if let Some(v) = store.get(TOKEN_KEY) {
            if let Some(s) = v.as_str() {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
        let token = generate();
        store.set(TOKEN_KEY, serde_json::Value::String(token.clone()));
        let _ = store.save();
        return token;
    }
    // Store unavailable: still hand back a working token for this run.
    generate()
}

/// Mint and persist a fresh token, returning it. Used by the "regenerate" action.
pub fn regenerate(app: &AppHandle) -> Result<String, String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    let token = generate();
    store.set(TOKEN_KEY, serde_json::Value::String(token.clone()));
    store.save().map_err(|e| e.to_string())?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_64_hex_chars_and_unique() {
        let a = generate();
        let b = generate();
        assert_eq!(a.len(), 64, "32 bytes → 64 hex chars");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "two tokens must differ");
    }
}
