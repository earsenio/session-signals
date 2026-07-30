//! Terminal capture: figure out which terminal *window* owns each session so a
//! row click can raise it (Feature 2), and resolve the session's git identity.
//!
//! We can't get a window handle from a hook payload — only `session_id` + `cwd`.
//! So at install time Session Signals writes a small per-OS script and registers
//! it as a **command** hook on `SessionStart` and `Stop`. Claude Code runs the
//! script; it reads the hook JSON on stdin, walks the parent-process chain up to
//! the top-level terminal application, and POSTs `{terminal_pid, terminal_app}`
//! back to Session Signals' listener (carrying the auth token) as a synthetic
//! `BeaconTerminal` event. The engine stores the pid on the session; `focus.rs`
//! later raises that pid's window.
//!
//! **Why the git facts are resolved here and not in the engine.** The row label
//! is `repo (branch)`, which used to be read straight off `<cwd>/.git/HEAD` by
//! `engine.rs`. That made the *app* touch arbitrary user directories, and macOS
//! gates those per protected category (Desktop / Documents / Downloads / network
//! volumes are each a separate TCC grant) — so every session opened under a
//! not-yet-granted folder popped a "would like to access files in your … folder"
//! prompt. This script instead runs inside the user's own shell, in the session's
//! cwd, under the *terminal's* existing grants, and ships the answer in the
//! payload. The app now reads nothing outside its own data dir, so no folder
//! prompt can ever fire. Keep it that way.
//!
//! The script is regenerated whenever the port or token changes, so it always
//! targets the live listener. It carries the `beacon-capture` marker in its
//! filename so the hook installer can recognize (and cleanly remove) its command
//! hook structurally, exactly like the http hooks.

use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// Marker substring present in both the script filename and the command-hook
/// string, used by the installer to identify Session Signals' capture hook.
pub const MARKER: &str = "beacon-capture";

/// Argument that puts the script in per-turn mode: git identity only, no
/// parent-process walk, and the report may not create a session row.
const TURN_ARG: &str = "turn";

/// The event that runs the script in per-turn mode. Kept next to [`TURN_ARG`] so
/// the wiring and the mode contract stay together.
const TURN_EVENT: &str = "Stop";

/// Events whose `command` hook runs the capture script.
///
/// - `SessionStart` — full capture: terminal handle *and* git identity.
/// - `Stop` — git identity only, so a mid-session `git checkout` reaches the
///   widget within one turn. Chosen over `UserPromptSubmit` because that hook
///   sits between the user pressing enter and Claude starting, which is exactly
///   where CLAUDE.md's "hooks must never slow Claude Code down" guardrail bites;
///   `Stop` fires when the session is idle by definition.
///
/// Windows omits `Stop` — see the PowerShell template's note.
#[cfg(not(windows))]
pub const CAPTURE_EVENTS: &[&str] = &["SessionStart", TURN_EVENT];
#[cfg(windows)]
pub const CAPTURE_EVENTS: &[&str] = &["SessionStart"];

/// The command-hook string to register for `event`, given the base command
/// returned by [`write_script`]. Only the per-turn event gets the mode argument.
pub fn command_for_event(base_cmd: &str, event: &str) -> String {
    if event == TURN_EVENT {
        format!("{base_cmd} {TURN_ARG}")
    } else {
        base_cmd.to_string()
    }
}

#[cfg(windows)]
const SCRIPT_NAME: &str = "beacon-capture.ps1";
#[cfg(not(windows))]
const SCRIPT_NAME: &str = "beacon-capture.sh";

/// POSIX shell capture (macOS/Linux). On macOS a GUI app's parent is launchd
/// (pid 1), so walking up until the parent is pid ≤ 1 lands on the terminal app.
#[cfg(not(windows))]
const SCRIPT_TEMPLATE: &str = r#"#!/bin/sh
# Session Signals terminal-capture hook (auto-generated — do not edit).
PORT=__PORT__
TOKEN=__TOKEN__
# Escape a value for embedding in the hand-built JSON body below.
jesc() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }
payload=$(cat)
sid=$(printf '%s' "$payload" | sed -n 's/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
[ -z "$sid" ] && exit 0
cwd=$(printf '%s' "$payload" | sed -n 's/.*"cwd"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
# Resolve the session's git identity here, in the user's shell, instead of in
# the app — see the module docs. Silent, best-effort: no git, or a cwd that
# isn't a repo, just leaves the fields empty and the app falls back to
# basename(cwd).
branch=""; repo=""; wt="false"
if [ -n "$cwd" ] && command -v git >/dev/null 2>&1; then
  branch=$(git -C "$cwd" rev-parse --abbrev-ref HEAD 2>/dev/null)
  [ "$branch" = "HEAD" ] && branch=""   # detached
  # --path-format=absolute (git >= 2.31) is required, not cosmetic: without it
  # a cwd *below* the repo root returns --git-dir absolute but --git-common-dir
  # relative ("../.git"), so the inequality test below would call every
  # subdirectory a worktree.
  gp=$(git -C "$cwd" rev-parse --path-format=absolute \
         --show-toplevel --git-dir --git-common-dir 2>/dev/null)
  if [ -n "$gp" ]; then
    top=$(printf  '%s\n' "$gp" | sed -n 1p)
    gdir=$(printf '%s\n' "$gp" | sed -n 2p)
    gcom=$(printf '%s\n' "$gp" | sed -n 3p)
    if [ "$gdir" != "$gcom" ]; then
      # Linked worktree: name the row after the *main* repo, not the worktree
      # folder. A submodule has the two dirs equal, so it stays unflagged.
      wt="true"
      repo=$(basename "$(dirname "$gcom")")
    else
      repo=$(basename "$top")
    fi
  fi
fi
# The terminal handle is fixed for the life of a session, so only the
# SessionStart invocation walks for it. The per-turn invocation ($1 = "turn")
# skips this block entirely — it is by far the most expensive part of the
# script (5-25 `ps` execs) and re-resolving it every turn would buy nothing.
term=""
if [ "$1" != "turn" ]; then
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
  term=",\"terminal_pid\":$pid,\"terminal_app\":\"$(jesc "$app")\",\"terminal_tty\":\"$(jesc "$tty")\""
fi
# capture_mode tells the engine whether this report may create a session row.
# "full" (SessionStart) may — it can legitimately beat the http hook. "turn"
# (Stop) may not: it could land after SessionEnd and resurrect a dead row.
# capture_version marks the git fields as authoritative, so a fresh report
# replaces them wholesale (a checkout that detaches HEAD must clear the branch,
# not leave a stale one behind).
mode="full"
[ "$1" = "turn" ] && mode="turn"
curl -s -m 2 -X POST "http://127.0.0.1:$PORT/hook" \
  -H "Content-Type: application/json" \
  -H "X-Beacon-Token: $TOKEN" \
  -d "{\"hook_event_name\":\"BeaconTerminal\",\"capture_version\":2,\"capture_mode\":\"$mode\",\"session_id\":\"$(jesc "$sid")\",\"cwd\":\"$(jesc "$cwd")\",\"git_base\":\"$(jesc "$repo")\",\"git_branch\":\"$(jesc "$branch")\",\"git_worktree\":$wt$term}" \
  >/dev/null 2>&1
exit 0
"#;

/// PowerShell capture (Windows). Walks parents until the parent is explorer.exe
/// (the shell that launches GUI apps) or vanishes — that topmost process is the
/// terminal app. App-level only: a specific Windows Terminal *tab* isn't
/// addressable portably.
///
/// Only ever invoked in "full" mode: [`CAPTURE_EVENTS`] omits `Stop` on Windows
/// because a PowerShell cold start is ~100-300 ms, far too much to spend at every
/// turn boundary. Branch changes are therefore picked up at session start only.
#[cfg(windows)]
const SCRIPT_TEMPLATE: &str = r#"# Session Signals terminal-capture hook (auto-generated - do not edit).
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
# Git identity, resolved here rather than in the app so the app never opens a
# path under the session's cwd (see the module docs).
$branch = ''; $repo = ''; $wt = $false
if ($cwd) {
  $branch = (& git -C $cwd rev-parse --abbrev-ref HEAD 2>$null | Select-Object -First 1)
  if ($branch -eq 'HEAD') { $branch = '' }
  # --path-format=absolute (git >= 2.31) is required so the worktree test below
  # compares like with like; without it --git-common-dir can come back relative.
  $gp = @(& git -C $cwd rev-parse --path-format=absolute --show-toplevel --git-dir --git-common-dir 2>$null)
  if ($gp.Count -ge 3) {
    if ($gp[1] -ne $gp[2]) {
      # Linked worktree: name the row after the main repo, not the worktree dir.
      $wt = $true
      $repo = Split-Path -Leaf (Split-Path -Parent $gp[2])
    } else {
      $repo = Split-Path -Leaf $gp[0]
    }
  }
}
if (-not $branch) { $branch = '' }
if (-not $repo) { $repo = '' }
$body = @{ hook_event_name = 'BeaconTerminal'; capture_version = 2; capture_mode = 'full'; session_id = $sid; cwd = $cwd; git_base = $repo; git_branch = $branch; git_worktree = $wt; terminal_pid = $appPid; terminal_app = $appName } | ConvertTo-Json -Compress
try {
  Invoke-RestMethod -Uri "http://127.0.0.1:$port/hook" -Method Post -ContentType 'application/json' -Headers @{ 'X-Beacon-Token' = $token } -Body $body -TimeoutSec 2 | Out-Null
} catch {}
exit 0
"#;

/// Absolute path of the capture script in Session Signals' app-data dir.
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
    // The script embeds the auth token, so it must be owner-only. On unix,
    // create it 0o700 from the first byte (the exec bit is cosmetic — we invoke
    // via `sh` regardless), then re-apply in case the file already existed with
    // looser permissions from an earlier version.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o700)
            .open(&path)
            .ok()?;
        file.write_all(body.as_bytes()).ok()?;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    std::fs::write(&path, body).ok()?;
    Some(command_for(&path))
}

/// Tests for the POSIX template. The script is the only thing that touches a
/// session's working directory now, so its git derivation is load-bearing for
/// every row label — exercise it for real (a rendered script, real git
/// fixtures, a stubbed `curl` that captures the body) rather than trusting the
/// string by inspection.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn rendered() -> String {
        SCRIPT_TEMPLATE
            .replace("__PORT__", "4317")
            .replace("__TOKEN__", "testtoken")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "beacon-capture-test-{}-{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git(repo: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("git must be installed to run these tests");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Build a real repo with one commit on branch `main`.
    fn repo_fixture(root: &Path) {
        std::fs::create_dir_all(root).unwrap();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.email", "t@example.com"]);
        git(root, &["config", "user.name", "T"]);
        std::fs::write(root.join("f.txt"), "x").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "init"]);
    }

    /// Run the rendered script with `cwd` in its stdin payload and a stubbed
    /// `curl` on PATH, and return the JSON body it tried to POST.
    fn run_capture(dir: &Path, cwd: &str, mode_arg: Option<&str>) -> serde_json::Value {
        use std::os::unix::fs::PermissionsExt;

        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let out_file = dir.join("body.json");
        // Stub curl: the body is the argument right after `-d`.
        let stub = format!(
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"-d\" ]; then\n    printf '%s' \"$2\" > '{}'\n    exit 0\n  fi\n  shift\ndone\nexit 0\n",
            out_file.display()
        );
        let curl = bin.join("curl");
        std::fs::write(&curl, stub).unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();

        let script = dir.join("capture.sh");
        std::fs::write(&script, rendered()).unwrap();

        let payload = serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": "sess-1",
            "cwd": cwd,
        })
        .to_string();

        let path_env = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut cmd = Command::new("sh");
        cmd.arg(&script);
        if let Some(m) = mode_arg {
            cmd.arg(m);
        }
        let mut child = cmd
            .env("PATH", path_env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(payload.as_bytes())
                .unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "script failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let body = std::fs::read_to_string(&out_file).expect("script never called curl");
        serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("script emitted invalid JSON ({e}): {body}"))
    }

    /// A template typo would silently disable capture at runtime (the hook's
    /// output is discarded), so fail the build instead.
    #[test]
    fn template_is_valid_shell() {
        let dir = scratch("syntax");
        let script = dir.join("capture.sh");
        std::fs::write(&script, rendered()).unwrap();
        let out = Command::new("sh").arg("-n").arg(&script).output().unwrap();
        assert!(
            out.status.success(),
            "sh -n rejected the template: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A normal clone reports the repo root's name and its branch — including
    /// from a subdirectory cwd, which is the case that regressed when
    /// `--path-format=absolute` was missing (`--git-dir` came back absolute
    /// while `--git-common-dir` stayed relative, so every subdirectory looked
    /// like a worktree).
    #[test]
    fn reports_repo_and_branch_from_root_and_subdir() {
        let dir = scratch("clone");
        let repo = dir.join("myrepo");
        repo_fixture(&repo);

        let v = run_capture(&dir, repo.to_str().unwrap(), None);
        assert_eq!(v["git_base"], "myrepo");
        assert_eq!(v["git_branch"], "main");
        assert_eq!(v["git_worktree"], false);
        assert_eq!(v["capture_version"], 2);
        assert_eq!(v["capture_mode"], "full");

        let sub = repo.join("src-tauri");
        std::fs::create_dir_all(&sub).unwrap();
        let v = run_capture(&dir, sub.to_str().unwrap(), None);
        assert_eq!(v["git_base"], "myrepo");
        assert_eq!(v["git_branch"], "main");
        assert_eq!(
            v["git_worktree"], false,
            "a subdirectory of a normal clone is not a worktree"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A linked worktree is flagged and labelled with the MAIN repo's name, not
    /// the worktree directory's — matching the behaviour the engine used to
    /// derive from `commondir`.
    #[test]
    fn reports_main_repo_name_for_linked_worktree() {
        let dir = scratch("worktree");
        let repo = dir.join("myrepo");
        repo_fixture(&repo);
        let wt = dir.join("wt-dir");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature-x",
                wt.to_str().unwrap(),
            ],
        );

        let v = run_capture(&dir, wt.to_str().unwrap(), None);
        assert_eq!(v["git_base"], "myrepo", "worktree rows name the main repo");
        assert_eq!(v["git_branch"], "feature-x");
        assert_eq!(v["git_worktree"], true);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Detached HEAD has no branch to show; a non-repo cwd has neither. Both
    /// must report empty rather than guessing, so the engine falls back to
    /// `basename(cwd)`.
    #[test]
    fn detached_head_and_non_repo_report_empty() {
        let dir = scratch("empty");

        let repo = dir.join("myrepo");
        repo_fixture(&repo);
        git(&repo, &["checkout", "-q", "--detach"]);
        let v = run_capture(&dir, repo.to_str().unwrap(), None);
        assert_eq!(v["git_base"], "myrepo");
        assert_eq!(v["git_branch"], "", "detached HEAD reports no branch");

        let plain = dir.join("not-a-repo");
        std::fs::create_dir_all(&plain).unwrap();
        let v = run_capture(&dir, plain.to_str().unwrap(), None);
        assert_eq!(v["git_base"], "");
        assert_eq!(v["git_branch"], "");
        assert_eq!(v["git_worktree"], false);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Per-turn mode skips the parent-process walk (the expensive part) and
    /// marks itself so the engine won't let it create a session row.
    #[test]
    fn turn_mode_sends_git_only() {
        let dir = scratch("turn");
        let repo = dir.join("myrepo");
        repo_fixture(&repo);

        let v = run_capture(&dir, repo.to_str().unwrap(), Some(TURN_ARG));
        assert_eq!(v["capture_mode"], "turn");
        assert_eq!(v["git_branch"], "main");
        assert!(
            v.get("terminal_pid").is_none(),
            "turn mode must not walk for a terminal handle"
        );
        assert!(v.get("terminal_tty").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cwd containing a quote or backslash used to be spliced raw into the
    /// hand-built body, producing invalid JSON that the listener dropped — so
    /// such a session silently lost capture *entirely*. `jesc` fixes that: the
    /// body always parses.
    ///
    /// Fidelity of the path itself is a separate, pre-existing limitation: the
    /// `sed` that lifts `cwd` out of the raw stdin JSON stops at the first `"`
    /// and doesn't unescape `\"`/`\\`, so a pathological path arrives truncated.
    /// That's why the `BeaconTerminal` arm in `engine.rs` never overwrites a cwd
    /// it already has — the real hooks deliver it through a proper JSON parser.
    /// Fixing the extraction needs portable escape-aware parsing in POSIX sh;
    /// not worth it for a case that essentially doesn't occur on Unix.
    #[test]
    fn quotes_and_backslashes_in_cwd_stay_valid_json() {
        let dir = scratch("escaping");
        let odd = dir.join(r#"we"ird\path"#);
        std::fs::create_dir_all(&odd).unwrap();

        // run_capture parses the body as JSON, so reaching this line at all is
        // the assertion that matters: the report is still well-formed and the
        // session is still captured.
        let v = run_capture(&dir, odd.to_str().unwrap(), None);
        assert_eq!(v["session_id"], "sess-1");
        assert_eq!(v["capture_mode"], "full");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The per-turn event is the only one that gets the mode argument.
    #[test]
    fn only_the_turn_event_gets_the_mode_argument() {
        assert_eq!(
            command_for_event("sh '/x/y.sh'", "SessionStart"),
            "sh '/x/y.sh'"
        );
        assert_eq!(
            command_for_event("sh '/x/y.sh'", TURN_EVENT),
            "sh '/x/y.sh' turn"
        );
        assert!(CAPTURE_EVENTS.contains(&"SessionStart"));
    }
}
