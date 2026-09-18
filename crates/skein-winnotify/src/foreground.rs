//! Bringing Skein's main window to the foreground from a
//! notification click (#155).
//!
//! Plain `SetForegroundWindow` is refused by Windows' foreground-lock
//! rule the moment the calling thread didn't cause the current
//! foreground window to lose focus — which is exactly the toast-click
//! case: `Activate`/`Activated` run inside Skein, but the *shell*
//! (explorer.exe / `ShellExperienceHost`) is what the user's click
//! actually landed on. `AttachThreadInput` borrows the current
//! foreground thread's input state so `SetForegroundWindow` is
//! allowed to succeed on Skein's behalf.

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, SW_RESTORE,
    SetForegroundWindow, ShowWindow,
};

/// RAII guard for `AttachThreadInput`: detaches on every exit path —
/// early return, later failure, or plain fall-through — so a bailout
/// partway through `bring_to_front` never leaves Skein's thread
/// permanently borrowing another process's input queue.
struct ThreadInputLink {
    from: u32,
    to: u32,
}

impl ThreadInputLink {
    fn attach(from: u32, to: u32) -> Option<Self> {
        // SAFETY: `from`/`to` are live thread ids obtained just above
        // this call site (`GetCurrentThreadId`,
        // `GetWindowThreadProcessId`); requesting attach (`true`)
        // here is undone by `Drop` requesting detach (`false`).
        let attached = unsafe { AttachThreadInput(from, to, true) };
        attached.as_bool().then_some(Self { from, to })
    }
}

impl Drop for ThreadInputLink {
    fn drop(&mut self) {
        // SAFETY: detaches exactly the link `attach` created above;
        // safe to call even if the target window/thread has since
        // gone away (Windows just reports failure, which we ignore —
        // there's nothing left to undo either way).
        let _ = unsafe { AttachThreadInput(self.from, self.to, false) };
    }
}

/// Best-effort foreground steal for `hwnd` (a raw `HWND` value, e.g.
/// from `tauri::WebviewWindow::hwnd()` cast with `.0 as isize`).
/// Restores the window first if minimized, then tries
/// `SetForegroundWindow` directly; if the foreground lock refuses
/// that, borrows the current foreground thread's input state and
/// retries. Returns whether `SetForegroundWindow` ultimately reported
/// success — a `false` still leaves the window restored/visible, just
/// not focused.
pub fn bring_to_front(hwnd: isize) -> bool {
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);

    // SAFETY: `hwnd` is caller-supplied and assumed to name a live
    // top-level window (the documented contract of this function);
    // `IsIconic`/`ShowWindow` read/write only that window's own state.
    let iconic = unsafe { IsIconic(hwnd) };
    if iconic.as_bool() {
        // SAFETY: see above.
        let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
    }

    // SAFETY: see above.
    if unsafe { SetForegroundWindow(hwnd) }.as_bool() {
        return true;
    }

    // SAFETY: no arguments to misuse; reads the current foreground
    // window only.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_invalid() {
        return false;
    }
    // SAFETY: reads the calling thread's own id.
    let current_thread = unsafe { GetCurrentThreadId() };
    // SAFETY: `foreground` was just returned by `GetForegroundWindow`
    // above and is still live (no window can be destroyed between
    // these two calls on this thread).
    let foreground_thread = unsafe { GetWindowThreadProcessId(foreground, None) };
    if foreground_thread == 0 {
        return false;
    }

    let Some(_link) = ThreadInputLink::attach(current_thread, foreground_thread) else {
        return false;
    };

    // SAFETY: `hwnd` is the same caller-supplied handle validated
    // (by assumption) above.
    let _ = unsafe { BringWindowToTop(hwnd) };
    // SAFETY: see above.
    let success = unsafe { SetForegroundWindow(hwnd) }.as_bool();
    // SAFETY: see above; the return value (previously focused window)
    // is intentionally discarded — nothing here needs to restore it.
    let _ = unsafe { SetFocus(Some(hwnd)) };

    success
    // `_link` drops here on every path above, always detaching.
}
