//! Terminal capture: figure out which terminal *window* owns each session so a
//! row click can raise it (Feature 2).
//!
//! We can't get a window handle from a hook payload — only `session_id` + `cwd`.
//! So at install time Beacon writes a small per-OS script and registers it as a
//! `SessionStart` **command** hook. When a session starts, Claude Code runs the
//! script; it reads the hook JSON on stdin, walks the parent-process chain up to
//! the top-level terminal application, and POSTs `{terminal_pid, terminal_app}`
//! back to Beacon's listener (carrying the auth token) as a synthetic
//! `BeaconTerminal` event. The engine stores the pid on the session; `focus.rs`
//! later raises that pid's window.
//!
//! The script is regenerated whenever the port or token changes, so it always
//! targets the live listener. It carries the `beacon-capture` marker in its
//! filename so the hook installer can recognize (and cleanly remove) its command
//! hook structurally, exactly like the http hooks.

use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// Marker substring present in both the script filename and the command-hook
/// string, used by the installer to identify Beacon's capture hook.
pub const MARKER: &str = "beacon-capture";

#[cfg(windows)]
const SCRIPT_NAME: &str = "beacon-capture.ps1";
#[cfg(not(windows))]
const SCRIPT_NAME: &str = "beacon-capture.sh";

/// POSIX shell capture (macOS/Linux). On macOS a GUI app's parent is launchd
/// (pid 1), so walking up until the parent is pid ≤ 1 lands on the terminal app.
#[cfg(not(windows))]
const SCRIPT_TEMPLATE: &str = r#"#!/bin/sh
# Beacon terminal-capture hook (auto-generated — do not edit).
PORT=__PORT__
TOKEN=__TOKEN__
payload=$(cat)
sid=$(printf '%s' "$payload" | sed -n 's/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
[ -z "$sid" ] && exit 0
cwd=$(printf '%s' "$payload" | sed -n 's/.*"cwd"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
# Walk the parent-process chain to the top-level terminal app (pid). Along the
# way, capture the first *real* controlling tty — the hook process itself is
# detached (tty "??"), but its ancestors (claude, the shell) carry the tab's
# pty, e.g. "ttys003". That tty is the key that lets focus.rs select the exact
# tab/window, not just raise the app.
pid=$$
tty=""
while :; do
  info=$(ps -o ppid=,tty= -p "$pid" 2>/dev/null)
  ppid=$(printf '%s' "$info" | awk '{print $1}')
  t=$(printf '%s' "$info" | awk '{print $2}')
  if [ -z "$tty" ] && [ -n "$t" ] && [ "$t" != "?" ] && [ "$t" != "??" ]; then
    tty="/dev/$t"
  fi
  [ -z "$ppid" ] && break
  [ "$ppid" -le 1 ] && break
  pid=$ppid
done
app=$(ps -o comm= -p "$pid" 2>/dev/null | sed 's:.*/::')
curl -s -m 2 -X POST "http://127.0.0.1:$PORT/hook" \
  -H "Content-Type: application/json" \
  -H "X-Beacon-Token: $TOKEN" \
  -d "{\"hook_event_name\":\"BeaconTerminal\",\"session_id\":\"$sid\",\"cwd\":\"$cwd\",\"terminal_pid\":$pid,\"terminal_app\":\"$app\",\"terminal_tty\":\"$tty\"}" \
  >/dev/null 2>&1
exit 0
"#;

/// PowerShell capture (Windows). Walks parents until the parent is explorer.exe
/// (the shell that launches GUI apps) or vanishes — that topmost process is the
/// terminal app. App-level only: a specific Windows Terminal *tab* isn't
/// addressable portably.
#[cfg(windows)]
const SCRIPT_TEMPLATE: &str = r#"# Beacon terminal-capture hook (auto-generated - do not edit).
$ErrorActionPreference = 'SilentlyContinue'
$port = __PORT__
$token = '__TOKEN__'
$raw = [Console]::In.ReadToEnd()
try { $j = $raw | ConvertFrom-Json } catch { exit 0 }
$sid = $j.session_id
if (-not $sid) { exit 0 }
$cwd = $j.cwd
$cur = $PID
$appPid = $cur
$appName = ''
for ($i = 0; $i -lt 24; $i++) {
  $proc = Get-CimInstance Win32_Process -Filter "ProcessId=$cur"
  if (-not $proc) { break }
  $appPid = $cur
  $appName = $proc.Name
  $ppid = [int]$proc.ParentProcessId
  if ($ppid -le 0) { break }
  $parent = Get-CimInstance Win32_Process -Filter "ProcessId=$ppid"
  if (-not $parent -or $parent.Name -eq 'explorer.exe') { break }
  $cur = $ppid
}
$body = @{ hook_event_name = 'BeaconTerminal'; session_id = $sid; cwd = $cwd; terminal_pid = $appPid; terminal_app = $appName } | ConvertTo-Json -Compress
try {
  Invoke-RestMethod -Uri "http://127.0.0.1:$port/hook" -Method Post -ContentType 'application/json' -Headers @{ 'X-Beacon-Token' = $token } -Body $body -TimeoutSec 2 | Out-Null
} catch {}
exit 0
"#;

/// Absolute path of the capture script in Beacon's app-data dir.
fn script_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))?;
    Ok(dir.join(SCRIPT_NAME))
}

/// The `command` string for the SessionStart hook that runs the script.
#[cfg(windows)]
fn command_for(path: &std::path::Path) -> String {
    format!(
        "powershell -NoProfile -ExecutionPolicy Bypass -File \"{}\"",
        path.display()
    )
}

#[cfg(not(windows))]
fn command_for(path: &std::path::Path) -> String {
    // Single-quote the path so spaces (e.g. "Application Support") are safe.
    format!("sh '{}'", path.display())
}

/// (Re)write the capture script with the current port + token and return the
/// command-hook string to register for `SessionStart`. Best-effort: returns
/// `None` if the script can't be written (the rest of the install still
/// proceeds — capture is an enhancement, not a requirement).
pub fn write_script(app: &AppHandle, port: u16, token: &str) -> Option<String> {
    let path = script_path(app).ok()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let body = SCRIPT_TEMPLATE
        .replace("__PORT__", &port.to_string())
        .replace("__TOKEN__", token);
    std::fs::write(&path, body).ok()?;
    // Make it executable on unix (cosmetic — we invoke via `sh` regardless).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    Some(command_for(&path))
}
