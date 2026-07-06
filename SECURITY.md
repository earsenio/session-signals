# Security Policy

Session Signals is a fully-local desktop app: no telemetry, no outbound
network calls, ever. It nonetheless has a few local surfaces worth an explicit
threat model, because it runs a loopback HTTP listener and edits
`~/.claude/settings.json`.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

- Preferred: use GitHub's **private vulnerability reporting** —
  [Report a vulnerability](https://github.com/earsenio/session-signals/security/advisories/new)
  (Security → Advisories → "Report a vulnerability"). This keeps the report
  private until a fix is available.

Please include: affected version, your OS, reproduction steps, and impact. We'll
acknowledge receipt, investigate, and coordinate a fix and disclosure timeline
with you. There is no bug-bounty program; we're grateful for responsible
disclosure regardless.

## Supported versions

Session Signals is pre-1.0. Security fixes land on the latest release line; please verify
against the newest [release](https://github.com/earsenio/session-signals/releases)
before reporting.

## Threat model

### 1. The `127.0.0.1:<port>` HTTP listener (default 4317)

- **Implementation:** binds loopback only (`SocketAddr::from(([127,0,0,1], port))`).
  Defense-in-depth additionally rejects any non-loopback peer with `403`.
  `POST /hook` requires a matching `X-Beacon-Token` header or returns `401` and
  changes no state. `GET /state` is read-only and loopback-bound.
- **Residual risk:** any **local** process or user on the machine can reach the
  port. The token mitigates spoofing of `/hook`. `GET /state` is intentionally
  un-gated (read-only, can't spoof state) but exposes session labels (cwd
  basenames + git branch) to any local reader — relevant on shared/multi-user
  machines.
- **Note:** the token comparison is a plain string compare, not constant-time.
  Local-only reach plus a low-value secret makes timing attacks impractical;
  hardening to a constant-time compare is optional.

### 2. The listener auth token

- **Implementation:** 32 bytes from the OS CSPRNG → 64 hex chars, generated on
  first run, stored in the app-data store, and re-stamped into every installed
  hook on install, on port change, and on regeneration. Hooks left with a stale
  or missing token after an upgrade are auto-repaired.
- **Residual risk:** the token lives in `~/.claude/settings.json` and the
  app-data store in **plaintext**, readable by the user. That is appropriate for
  a loopback shared secret, not a high-value credential.

### 3. Writing to `~/.claude/settings.json`

- **Implementation:** a **non-destructive merge** — Session Signals' hooks are identified
  *structurally* (an HTTP hook to the loopback `/hook`, or the `command` capture
  hook carrying the `beacon-capture` marker), never by clobbering unrelated keys.
  A **one-time backup** is written to `settings.json.beacon.bak` before any
  change. Uninstall strips only Session Signals' entries and prunes emptied arrays. A
  present-but-unparseable settings file is refused, not overwritten.
- **Residual risk:** Session Signals also installs a **`command` hook**
  (`beacon-capture.sh` / `.ps1`) that runs on `SessionStart`. It is auto-generated
  into app-data and registered to run; it walks the process tree to find the
  owning terminal (for click-to-focus) and POSTs only
  `{terminal_pid, terminal_app, tty}` to the loopback listener. It is removed on
  uninstall. This is the most surprising thing Session Signals does to a security
  reviewer, so it is called out explicitly.

### 4. Webview content security

The UI loads no remote content; there is no HTTP client in the codebase. A
restrictive CSP is enforced as defense-in-depth (see `app.security.csp` in
`src-tauri/tauri.conf.json`).

## Summary of guarantees

- No telemetry; no outbound network calls, ever.
- Listener is loopback-only and rejects non-loopback peers.
- State-changing requests are token-gated.
- `settings.json` edits are non-destructive and backed up; uninstall is clean.
