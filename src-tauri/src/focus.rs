//! Best-effort OS window focus.
//!
//! - [`raise`]       — bring the window *and the exact tab* owning a session to
//!   the front, given its `(pid, tty, app)` target.
//! - [`raise_pid`]   — bring the top-level window owned by `pid` to the front
//!   (app-level only); the fallback `raise` uses when tab selection isn't
//!   possible.
//! - [`frontmost_pid`] — the pid of whichever app currently owns the foreground.
//!
//! Tab precision: on macOS, scriptable terminals (Terminal.app, iTerm2) expose a
//! per-tab `tty`, so if Beacon captured the session's tty we select that exact
//! tab via AppleScript rather than merely raising the app — the difference
//! between landing on the right session and landing on whatever tab happened to
//! be active. Unknown terminals, a tmux pane, an IDE-integrated terminal, and
//! all of Windows fall back to the app-level raise. Everything degrades
//! gracefully — an unresolvable target yields `false`, never a panic.

/// Everything Beacon captured about where a session lives, enough to focus it.
/// `tty`/`app` are best-effort; `pid` (the terminal app) is the floor.
pub struct FocusTarget {
    pub pid: i32,
    /// Controlling tty, e.g. "/dev/ttys003". Enables tab-precise focus.
    pub tty: Option<String>,
    /// Terminal app name (e.g. "Terminal", "iTerm2"), picks the focus strategy.
    pub app: Option<String>,
}

/// Focus the exact tab/window owning a session. Tries tab-precise AppleScript
/// for known terminals, then falls back to an app-level raise by pid.
#[cfg(target_os = "macos")]
pub fn raise(t: &FocusTarget) -> bool {
    if let Some(tty) = t.tty.as_deref().filter(|s| !s.is_empty()) {
        let app = t.app.as_deref().unwrap_or("");
        // `comm` basenames: Terminal.app → "Terminal", iTerm2 → "iTerm2".
        if app.eq_ignore_ascii_case("Terminal") {
            if raise_terminal_app_tab(tty) {
                return true;
            }
        } else if app.eq_ignore_ascii_case("iTerm2") || app.eq_ignore_ascii_case("iTerm") {
            if raise_iterm_tab(tty) {
                return true;
            }
        }
    }
    raise_pid(t.pid)
}

/// Run an AppleScript and report success — true only if osascript exited 0 *and*
/// the script printed our sentinel (so "compiled but matched nothing" reads as a
/// miss, letting the caller fall back rather than claim a phantom success).
#[cfg(target_os = "macos")]
fn osascript_ok(script: &str) -> bool {
    std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("BEACON_OK"))
        .unwrap_or(false)
}

/// Terminal.app: find the tab whose `tty` matches, select it, raise its window.
/// `tty` is a fixed `/dev/ttysNNN` string (no quoting hazard).
#[cfg(target_os = "macos")]
fn raise_terminal_app_tab(tty: &str) -> bool {
    let script = format!(
        r#"tell application "Terminal"
  repeat with w in windows
    repeat with t in tabs of w
      if (tty of t) is "{tty}" then
        set selected of t to true
        set frontmost of w to true
        activate
        return "BEACON_OK"
      end if
    end repeat
  end repeat
end tell
return "no""#
    );
    osascript_ok(&script)
}

/// iTerm2: a tab holds sessions, each with a `tty`. Select the matching session,
/// its tab, and window, then activate.
#[cfg(target_os = "macos")]
fn raise_iterm_tab(tty: &str) -> bool {
    let script = format!(
        r#"tell application "iTerm2"
  repeat with w in windows
    repeat with t in tabs of w
      repeat with s in sessions of t
        if (tty of s) is "{tty}" then
          tell w to select
          tell t to select
          tell s to select
          activate
          return "BEACON_OK"
        end if
      end repeat
    end repeat
  end repeat
end tell
return "no""#
    );
    osascript_ok(&script)
}

/// Raise the top-level window owned by `pid`. Returns whether a window was found
/// and a raise was attempted.
#[cfg(target_os = "macos")]
pub fn raise_pid(pid: i32) -> bool {
    // Two-step, because macOS 14+ (Sonoma) tightened cross-app activation:
    //
    // 1. NSRunningApplication.activate — cheap, needs no special permission, and
    //    is enough when Beacon itself is active. But from a background process it
    //    often *won't* pull another app forward (the deprecated
    //    `IgnoringOtherApps` flag is now a no-op), so on its own it's unreliable.
    // 2. System Events via AppleScript (`set frontmost … to true`), which raises
    //    by pid reliably. The user grants Automation permission once on first use.
    //
    // We attempt both and report success from whichever path worked.
    let native = {
        use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
        match NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            Some(app) => {
                app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
                true
            }
            None => false,
        }
    };
    let script = format!(
        "tell application \"System Events\" to set frontmost of (first process whose unix id is {pid}) to true"
    );
    match std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
    {
        // AppleScript succeeded → the window is frontmost.
        Ok(o) if o.status.success() => true,
        // Blocked (no Automation permission) or unavailable → fall back to
        // whether the native activate at least found a running app.
        _ => native,
    }
}

/// The pid of the frontmost application, if any.
#[cfg(target_os = "macos")]
pub fn frontmost_pid() -> Option<i32> {
    use objc2_app_kit::NSWorkspace;
    let ws = NSWorkspace::sharedWorkspace();
    ws.frontmostApplication().map(|app| app.processIdentifier())
}

// --- Windows ---------------------------------------------------------------

/// Windows Terminal tabs aren't addressable portably, so we raise the terminal
/// app's window by pid (tty/app on the target are unused here).
#[cfg(target_os = "windows")]
pub fn raise(t: &FocusTarget) -> bool {
    raise_pid(t.pid)
}

#[cfg(target_os = "windows")]
pub fn raise_pid(pid: i32) -> bool {
    // `BOOL` lives in `windows::core` as of the windows crate 0.58+; the rest
    // remain under `Win32::Foundation`.
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, TRUE};
    use windows::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        SetForegroundWindow, ShowWindow, ASFW_ANY, SW_RESTORE,
    };

    struct Search {
        target: u32,
        found: Option<HWND>,
    }

    extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe {
            let search = &mut *(lparam.0 as *mut Search);
            // Only consider visible top-level windows.
            if !IsWindowVisible(hwnd).as_bool() {
                return TRUE;
            }
            let mut wnd_pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut wnd_pid));
            if wnd_pid == search.target {
                search.found = Some(hwnd);
                return BOOL(0); // FALSE → stop enumerating
            }
            TRUE
        }
    }

    if pid <= 0 {
        return false;
    }
    let mut search = Search {
        target: pid as u32,
        found: None,
    };
    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(&mut search as *mut Search as isize),
        );
        match search.found {
            Some(hwnd) => {
                // Standard foreground-permission nudge + un-minimize, then raise.
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                SetForegroundWindow(hwnd).as_bool()
            }
            None => false,
        }
    }
}

#[cfg(target_os = "windows")]
pub fn frontmost_pid() -> Option<i32> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            None
        } else {
            Some(pid as i32)
        }
    }
}

// --- Fallback (other platforms) -------------------------------------------

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn raise(t: &FocusTarget) -> bool {
    raise_pid(t.pid)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn raise_pid(_pid: i32) -> bool {
    false
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn frontmost_pid() -> Option<i32> {
    None
}
