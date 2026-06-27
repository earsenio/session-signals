//! Localhost HTTP listener for Claude Code hooks.
//!
//! Binds `127.0.0.1:<port>` only and exposes a single `POST /hook` endpoint.
//! Claude Code's HTTP hooks POST the same JSON they would pass on stdin. We
//! respond immediately (hooks fire async; we never want to slow Claude down),
//! then hand the parsed event to a callback. Malformed or unknown payloads are
//! tolerated — they never crash the listener.

use crate::engine::HookEvent;
use std::net::{IpAddr, SocketAddr};
use tiny_http::{Header, Method, Request, Response, Server};

/// Start the listener on `127.0.0.1:port`. Spawns its own thread and returns
/// once the socket is bound (or an error if the port is taken).
///
/// - `POST /hook`  → parse the hook JSON and invoke `on_event`.
/// - `GET  /state` → return `state_json()`; a loopback-only health/readback
///   endpoint used to confirm the rollup without scraping the tray.
pub fn start<F, G>(port: u16, on_event: F, state_json: G) -> std::io::Result<()>
where
    F: Fn(HookEvent) + Send + 'static,
    G: Fn() -> String + Send + 'static,
{
    // Bind to loopback explicitly — never expose this to the network.
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let server = Server::http(addr)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::AddrInUse, e.to_string()))?;

    std::thread::Builder::new()
        .name("beacon-listener".into())
        .spawn(move || {
            for request in server.incoming_requests() {
                handle(request, &on_event, &state_json);
            }
        })?;

    Ok(())
}

fn handle<F, G>(mut request: Request, on_event: &F, state_json: &G)
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
            // Read the body (small JSON) before responding.
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            // Respond right away — never block Claude Code on our processing.
            let _ = request.respond(Response::from_string("ok").with_status_code(200));
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
