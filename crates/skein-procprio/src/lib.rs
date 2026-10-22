//! Lower the CPU priority of an already-spawned child process by pid.
//!
//! Skein spawns harness PTY children (shells, agent CLIs) that go on to run
//! cargo/rustc builds, which can starve the UI. Lowering the child right
//! after spawn makes its descendants inherit the lower class: on Windows a
//! `CreateProcessW` child of a below-normal process defaults to the
//! creator's class; on Unix niceness is inherited across fork.
//!
//! The rule is **never raise**: a process already at or below the target is
//! left alone ([`Outcome::AlreadyLow`]), so a user who started something
//! deliberately idle keeps it.
//!
//! Race the caller has to live with: the call happens after the child
//! exists, so any grandchild created before it keeps the original priority.

use std::io;

/// Niceness a lowered Unix process is set to.
pub const UNIX_NICE: i32 = 10;

/// What [`lower_priority`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The priority was lowered by this call.
    Lowered,
    /// The process was already at or below the target; nothing changed.
    AlreadyLow,
    /// This platform has no implementation.
    Unsupported,
}

/// Lower the priority of process `pid` (never raises it).
///
/// # Errors
/// Returns the OS error when the process cannot be opened, queried or set.
pub fn lower_priority(pid: u32) -> io::Result<Outcome> {
    imp::lower_priority(pid)
}

/// Whether `pid` is currently at or below the lowered priority.
///
/// # Errors
/// Returns the OS error when the process cannot be opened or queried.
pub fn is_lowered(pid: u32) -> io::Result<bool> {
    imp::is_lowered(pid)
}

/// Pure decision for Windows: lower unless already `BELOW_NORMAL` or `IDLE`.
#[cfg(any(windows, test))]
fn windows_needs_lowering(class: u32) -> bool {
    const IDLE: u32 = 0x40;
    const BELOW_NORMAL: u32 = 0x4000;
    !(class == IDLE || class == BELOW_NORMAL)
}

/// Pure decision for Unix: lower unless already at or above the target nice.
#[cfg(any(unix, test))]
fn unix_needs_lowering(nice: i32) -> bool {
    nice < UNIX_NICE
}

#[cfg(windows)]
mod imp {
    use super::{Outcome, io, windows_needs_lowering};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{
        BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass, OpenProcess,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, SetPriorityClass,
    };

    /// Closes the handle on drop so every path releases it.
    struct Owned(HANDLE);

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful OpenProcess, is owned
            // solely by this guard and is closed exactly once, here.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn open(pid: u32) -> io::Result<Owned> {
        // SAFETY: plain value arguments; the returned handle is wrapped in a
        // guard that closes it.
        let handle = unsafe {
            OpenProcess(
                PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            )
        }
        .map_err(io::Error::other)?;
        Ok(Owned(handle))
    }

    fn class_of(h: &Owned) -> io::Result<u32> {
        // SAFETY: `h` holds a live process handle with query access.
        let class = unsafe { GetPriorityClass(h.0) };
        if class == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(class)
    }

    pub fn lower_priority(pid: u32) -> io::Result<Outcome> {
        let h = open(pid)?;
        if !windows_needs_lowering(class_of(&h)?) {
            return Ok(Outcome::AlreadyLow);
        }
        // SAFETY: `h` holds a live process handle with set-information access.
        unsafe { SetPriorityClass(h.0, BELOW_NORMAL_PRIORITY_CLASS) }.map_err(io::Error::other)?;
        Ok(Outcome::Lowered)
    }

    pub fn is_lowered(pid: u32) -> io::Result<bool> {
        let h = open(pid)?;
        Ok(!windows_needs_lowering(class_of(&h)?))
    }
}

#[cfg(unix)]
mod imp {
    use super::{Outcome, UNIX_NICE, io, unix_needs_lowering};
    use rustix::process::{Pid, getpriority_process, setpriority_process};

    fn pid_of(pid: u32) -> io::Result<Pid> {
        i32::try_from(pid)
            .ok()
            .and_then(Pid::from_raw)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid pid"))
    }

    // Linux: PRIO_PROCESS on a pid acts on that thread id. Right after exec
    // the child is single-threaded, and later threads and children inherit it.
    pub fn lower_priority(pid: u32) -> io::Result<Outcome> {
        let pid = Some(pid_of(pid)?);
        if !unix_needs_lowering(getpriority_process(pid)?) {
            return Ok(Outcome::AlreadyLow);
        }
        setpriority_process(pid, UNIX_NICE)?;
        Ok(Outcome::Lowered)
    }

    pub fn is_lowered(pid: u32) -> io::Result<bool> {
        Ok(!unix_needs_lowering(getpriority_process(Some(pid_of(
            pid,
        )?))?))
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    use super::{Outcome, io};

    pub fn lower_priority(_pid: u32) -> io::Result<Outcome> {
        Ok(Outcome::Unsupported)
    }

    pub fn is_lowered(_pid: u32) -> io::Result<bool> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests;
