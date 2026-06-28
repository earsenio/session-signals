//! Best-effort OS window focus, keyed by a process id.
//!
//! Two operations, each with a per-platform implementation and a no-op fallback:
//!
//! - [`raise_pid`]   — bring the top-level window owned by `pid` to the front.
//! - [`frontmost_pid`] — the pid of whichever app currently owns the foreground.
//!
//! Both are *app/window level*, never tab level: focusing a terminal multiplexer
//! (tmux) pane or an IDE-integrated terminal tab is not portable and is out of
//! scope (see the Phase 5 brief). Everything degrades gracefully — an
//! unresolvable pid yields `false` / `None`, never a panic.

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

#[cfg(target_os = "windows")]
pub fn raise_pid(pid: i32) -> bool {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
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
pub fn raise_pid(_pid: i32) -> bool {
    false
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn frontmost_pid() -> Option<i32> {
    None
}
