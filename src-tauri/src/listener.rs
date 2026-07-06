//! Localhost HTTP listener for Claude Code hooks.
//!
//! Binds `127.0.0.1:<port>` only and exposes a single `POST /hook` endpoint.
//! Claude Code's HTTP hooks POST the same JSON they would pass on stdin. We
//! respond immediately (hooks fire async; we never want to slow Claude down),
//! then hand the parsed event to a callback. Malformed or unknown payloads are
//! tolerated — they never crash the listener.

use crate::engine::HookEvent;
use crate::token;
use crate::LockExt;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Request, Response, Server};

/// Shared, live-updatable auth token. Held as an `Arc<Mutex<String>>` so a
/// "regenerate token" action can swap the secret in place — the running listener
/// reads the current value on each request, no restart needed.
pub type AuthToken = Arc<Mutex<String>>;

/// Start the listener on `127.0.0.1:port`. Spawns its own thread and returns the
/// `Arc<Server>` (or an error if the port is taken). Hold the handle to stop
/// the listener later via [`Server::unblock`] — this is how a live port change
/// tears down the old server before swapping in a new one.
///
/// - `POST /hook`  → require a matching `X-Beacon-Token`, then parse the hook
///   JSON and invoke `on_event`. A missing/wrong token is rejected with 401 and
///   changes no state.
/// - `GET  /state` → return `state_json()`; a loopback-only health/readback
///   endpoint used to confirm the rollup without scraping the tray. Read-only
///   and loopback-bound, so it isn't token-gated (it can't spoof state).
pub fn start<F, G>(
    port: u16,
    auth: AuthToken,
    on_event: F,
    state_json: G,
) -> std::io::Result<Arc<Server>>
where
    F: Fn(HookEvent) + Send + 'static,
    G: Fn() -> String + Send + 'static,
{
    // Bind to loopback explicitly — never expose this to the network.
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let server = Server::http(addr)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::AddrInUse, e.to_string()))?;
    let server = Arc::new(server);

    let worker = server.clone();
    std::thread::Builder::new()
        .name("beacon-listener".into())
        .spawn(move || {
            // `recv` unblocks (returns Err) when `unblock` is called on a swap,
            // ending the loop.
            while let Ok(request) = worker.recv() {
                handle(request, &auth, &on_event, &state_json);
            }
        })?;

    Ok(server)
}

/// Does this request carry the expected `X-Beacon-Token`? Constant in spirit:
/// we compare the full strings. A blank expected token (shouldn't happen) fails
/// closed.
fn token_ok(request: &Request, auth: &AuthToken) -> bool {
    let expected = auth.lock_safe().clone();
    if expected.is_empty() {
        return false;
    }
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(token::HEADER))
        .map(|h| h.value.as_str() == expected)
        .unwrap_or(false)
}

fn handle<F, G>(mut request: Request, auth: &AuthToken, on_event: &F, state_json: &G)
where
    F: Fn(HookEvent),
    G: Fn() -> String,
{
    // Defense in depth: even though we bind loopback only, reject any peer that
    // isn't a loopback address.
    if let Some(addr) = request.remote_addr().copied() {
        if !is_loopback(addr) {
            let _ = request.respond(Response::from_string("forbidden").with_status_code(403));
            return;
        }
    }

    let method = request.method().clone();
    let url = request.url().to_string();

    match (&method, url.as_str()) {
        (Method::Post, "/hook") => {
            // Token gate: a missing/wrong token is rejected and alters nothing.
            // We must still drain the body so the client's write completes.
            if !token_ok(&request, auth) {
                let mut sink = String::new();
                let _ = request.as_reader().read_to_string(&mut sink);
                let _ = request.respond(
                    Response::from_string("{\"error\":\"unauthorized\"}").with_status_code(401),
                );
                return;
            }
            // Read the body (small JSON) before responding.
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            // Respond right away — never block Claude Code on our processing.
            // Claude Code parses an HTTP hook's response body as JSON, so we
            // must return a JSON object (not plain text like "ok") or it logs
            // "HTTP hook must return JSON". An empty object is a no-op decision.
            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("valid header");
            let _ = request.respond(
                Response::from_string("{}")
                    .with_status_code(200)
                    .with_header(header),
            );
            // Parse leniently. Unknown fields ignored; bad JSON dropped.
            match serde_json::from_str::<HookEvent>(&body) {
                Ok(ev) if !ev.hook_event_name.is_empty() => on_event(ev),
                _ => { /* malformed or non-event payload — ignore */ }
            }
        }
        (Method::Get, "/state") => {
            let json = state_json();
            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("valid header");
            let _ = request.respond(Response::from_string(json).with_header(header));
        }
        _ => {
            let _ = request.respond(Response::from_string("not found").with_status_code(404));
        }
    }
}

fn is_loopback(addr: SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Bound port of a started server (we start on port 0 → OS assigns).
    fn bound_port(server: &Server) -> u16 {
        match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            #[allow(unreachable_patterns)]
            _ => panic!("expected an IP listen address"),
        }
    }

    const TEST_TOKEN: &str = "test-token-abc123";

    fn auth(tok: &str) -> AuthToken {
        std::sync::Arc::new(Mutex::new(tok.to_string()))
    }

    /// POST a hook body with an explicit token header value (pass "" to omit).
    fn post_hook_with_token(port: u16, body: &str, tok: &str) {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let token_header = if tok.is_empty() {
            String::new()
        } else {
            format!("{}: {}\r\n", token::HEADER, tok)
        };
        let req = format!(
            "POST /hook HTTP/1.1\r\nHost: 127.0.0.1\r\n{token_header}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(req.as_bytes()).expect("write");
        stream.flush().ok();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).ok(); // drain so the server finishes the response
    }

    fn post_hook(port: u16, body: &str) {
        post_hook_with_token(port, body, TEST_TOKEN);
    }

    /// The mechanism behind a live port change: a port in use rejects a second
    /// bind (→ a clear "busy" error in set_config), and after `unblock` + drop
    /// the same port can be rebound (→ the swapped-in listener).
    #[test]
    fn busy_port_then_restart() {
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let server = start(
            0,
            auth(TEST_TOKEN),
            move |_ev| {
                c.fetch_add(1, Ordering::SeqCst);
            },
            || "{}".to_string(),
        )
        .expect("initial bind ok");
        let port = bound_port(&server);

        // A real hook reaches the callback.
        post_hook(
            port,
            r#"{"hook_event_name":"SessionStart","session_id":"x"}"#,
        );
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "callback should have run once"
        );

        // Binding the same port again is refused while it's in use.
        let busy = start(port, auth(TEST_TOKEN), |_e| {}, || "{}".to_string());
        assert!(busy.is_err(), "second bind on a live port must be busy");

        // Stop and release the port, then rebinding it succeeds.
        server.unblock();
        drop(server);
        std::thread::sleep(Duration::from_millis(150));
        let rebound = start(port, auth(TEST_TOKEN), |_e| {}, || "{}".to_string());
        assert!(
            rebound.is_ok(),
            "rebind after release should succeed: {:?}",
            rebound.err()
        );
    }

    /// A request with a missing or wrong token must be rejected and reach the
    /// callback zero times (it alters no state). The right token still works,
    /// and rotating the shared token in place takes effect with no restart.
    #[test]
    fn token_gate_rejects_then_accepts() {
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let tok = auth(TEST_TOKEN);
        let server = start(
            0,
            tok.clone(),
            move |_ev| {
                c.fetch_add(1, Ordering::SeqCst);
            },
            || "{}".to_string(),
        )
        .expect("bind ok");
        let port = bound_port(&server);
        let body = r#"{"hook_event_name":"SessionStart","session_id":"x"}"#;

        // No token → rejected, callback never runs.
        post_hook_with_token(port, body, "");
        // Wrong token → rejected too.
        post_hook_with_token(port, body, "nope");
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "bad token must alter nothing"
        );

        // Correct token → accepted.
        post_hook_with_token(port, body, TEST_TOKEN);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(count.load(Ordering::SeqCst), 1, "valid token is accepted");

        // Rotate the live token; the old one now fails and the new one works —
        // no listener restart involved.
        *tok.lock().unwrap() = "rotated-token".to_string();
        post_hook_with_token(port, body, TEST_TOKEN);
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "old token rejected after rotate"
        );
        post_hook_with_token(port, body, "rotated-token");
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(
            count.load(Ordering::SeqCst),
            2,
            "new token accepted after rotate"
        );

        server.unblock();
    }
}
