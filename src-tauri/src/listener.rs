//! Localhost HTTP listener for Claude Code hooks.
//!
//! Binds `127.0.0.1:<port>` only and exposes a single `POST /hook` endpoint.
//! Claude Code's HTTP hooks POST the same JSON they would pass on stdin. We
//! respond immediately (hooks fire async; we never want to slow Claude down),
//! then hand the parsed event to a callback. Malformed or unknown payloads are
//! tolerated — they never crash the listener.

use crate::engine::HookEvent;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tiny_http::{Header, Method, Request, Response, Server};

/// Start the listener on `127.0.0.1:port`. Spawns its own thread and returns the
/// `Arc<Server>` (or an error if the port is taken). Hold the handle to stop
/// the listener later via [`Server::unblock`] — this is how a live port change
/// tears down the old server before swapping in a new one.
///
/// - `POST /hook`  → parse the hook JSON and invoke `on_event`.
/// - `GET  /state` → return `state_json()`; a loopback-only health/readback
///   endpoint used to confirm the rollup without scraping the tray.
pub fn start<F, G>(port: u16, on_event: F, state_json: G) -> std::io::Result<Arc<Server>>
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
            // `recv` unblocks (returns Err) when `unblock` is called on a swap.
            loop {
                match worker.recv() {
                    Ok(request) => handle(request, &on_event, &state_json),
                    Err(_) => break,
                }
            }
        })?;

    Ok(server)
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

    fn post_hook(port: u16, body: &str) {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let req = format!(
            "POST /hook HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(req.as_bytes()).expect("write");
        stream.flush().ok();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).ok(); // drain so the server finishes the response
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
            move |_ev| {
                c.fetch_add(1, Ordering::SeqCst);
            },
            || "{}".to_string(),
        )
        .expect("initial bind ok");
        let port = bound_port(&server);

        // A real hook reaches the callback.
        post_hook(port, r#"{"hook_event_name":"SessionStart","session_id":"x"}"#);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(count.load(Ordering::SeqCst), 1, "callback should have run once");

        // Binding the same port again is refused while it's in use.
        let busy = start(port, |_e| {}, || "{}".to_string());
        assert!(busy.is_err(), "second bind on a live port must be busy");

        // Stop and release the port, then rebinding it succeeds.
        server.unblock();
        drop(server);
        std::thread::sleep(Duration::from_millis(150));
        let rebound = start(port, |_e| {}, || "{}".to_string());
        assert!(rebound.is_ok(), "rebind after release should succeed: {:?}", rebound.err());
    }
}
