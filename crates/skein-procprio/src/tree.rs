//! Kill a spawned child's whole process tree, not just the child.
//!
//! Killing a PTY's direct child leaves its grandchildren running. A
//! [`ProcessTree`] is attached to the child right after spawn and can later
//! end everything under it.
//!
//! # Limits
//!
//! - **Windows:** anything the child starts before [`ProcessTree::attach`]
//!   returns is outside the job, the same limit [`crate::lower_priority`] has.
//!   The job sets `BREAKAWAY_OK` (not `SILENT_BREAKAWAY_OK`): a descendant
//!   that explicitly passes `CREATE_BREAKAWAY_FROM_JOB` escapes the kill, and
//!   everything that never asks stays in. Without `BREAKAWAY_OK` such a
//!   process fails to start with access denied, which breaks tools in ways
//!   nothing explains.
//! - **Unix:** the tree is the child's process group (it must be a group
//!   leader, which portable-pty arranges with `setsid`). A descendant that
//!   moved itself to another process group or session (`setsid`, a
//!   job-control shell's background job) is not reached. A process-group id
//!   cannot be reused while the group has a member, so signalling is safe
//!   while any member lives. The escalation thread's final `SIGKILL` can
//!   follow its last liveness poll by up to 100 ms; a group that emptied and
//!   whose id was reused as a new group in that gap would be hit, which is
//!   accepted as not happening in practice (pid allocation does not hand a
//!   just-freed pid straight back). The caller must still not call
//!   [`ProcessTree::kill`] once the leader has been reaped, because then
//!   nothing says the group has members.

use std::io;

/// How long a Unix group gets to exit after `SIGHUP` before `SIGKILL`.
pub const UNIX_KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// A handle on a child's process tree.
///
/// On Windows this owns a job object created with kill-on-close: dropping a
/// `ProcessTree` kills the tree, and so does the owning process exiting. The
/// job allows explicit breakaway (`CREATE_BREAKAWAY_FROM_JOB`) but not silent
/// breakaway, so only a process that asks to leave the job escapes.
/// On Unix dropping does nothing.
#[derive(Debug)]
pub struct ProcessTree {
    inner: imp::Inner,
}

impl ProcessTree {
    /// Attach to the direct child `pid`. Call right after spawn.
    ///
    /// Anything the child starts before this returns is outside the tree on
    /// Windows (the same timing limit as [`crate::lower_priority`]).
    ///
    /// # Errors
    /// The OS error when the job cannot be set up, or `InvalidInput` on Unix
    /// when `pid` is not a process-group leader of its own group.
    /// `Unsupported` on other platforms.
    pub fn attach(pid: u32) -> io::Result<Self> {
        Ok(Self {
            inner: imp::attach(pid)?,
        })
    }

    /// Ends every process in the tree. Never blocks beyond a few syscalls;
    /// idempotent.
    ///
    /// # Errors
    /// The OS error when the signal or termination fails.
    pub fn kill(&self) -> io::Result<()> {
        imp::kill(&self.inner)
    }
}

#[cfg(windows)]
mod imp {
    use super::io;
    use std::ffi::c_void;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

    /// Closes the handle on drop so every path releases it.
    #[derive(Debug)]
    struct Owned(HANDLE);

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful Create/OpenProcess,
            // is owned solely by this guard and is closed exactly once, here.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    #[derive(Debug)]
    pub struct Inner(Owned);

    // SAFETY: a job handle is a kernel object usable from any thread, and
    // this type only calls TerminateJobObject and CloseHandle on it.
    unsafe impl Send for Inner {}
    // SAFETY: as above; those calls are safe to make concurrently.
    unsafe impl Sync for Inner {}

    pub fn attach(pid: u32) -> io::Result<Inner> {
        // SAFETY: null attributes and name are allowed; the returned handle
        // is wrapped in a guard that closes it.
        let job = Owned(unsafe { CreateJobObjectW(None, None) }.map_err(io::Error::from)?);

        // Kill-on-close plus BREAKAWAY_OK, deliberately not
        // SILENT_BREAKAWAY_OK. Without BREAKAWAY_OK a descendant that passes
        // CREATE_BREAKAWAY_FROM_JOB fails to start with access denied, which
        // breaks tools in ways nothing explains (Bun treats that as a compat
        // break and sets BREAKAWAY_OK on its own no-orphans job; VS Code's CLI
        // probes for it). With it, only a process that explicitly asks to
        // leave the job escapes the kill; everything else, which never asks
        // (libuv/Node never pass the flag), stays in.
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        let info_len = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
            .map_err(io::Error::other)?;
        // SAFETY: `job` is a live job handle; the pointer and length describe
        // `info`, which outlives the call.
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast::<c_void>(),
                info_len,
            )
        }
        .map_err(io::Error::from)?;

        // SAFETY: plain value arguments; the handle is wrapped in a guard.
        let process = Owned(
            unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) }
                .map_err(io::Error::from)?,
        );
        // Nested jobs (Windows 8+) let this work when the caller is itself in
        // a job. The pid cannot be reused while portable-pty holds the
        // child's handle.
        // SAFETY: both handles are live and owned by guards.
        unsafe { AssignProcessToJobObject(job.0, process.0) }.map_err(io::Error::from)?;
        drop(process);
        Ok(Inner(job))
    }

    pub fn kill(inner: &Inner) -> io::Result<()> {
        // SAFETY: the job handle is live while `inner` exists.
        unsafe { TerminateJobObject((inner.0).0, 1) }.map_err(io::Error::from)
    }
}

#[cfg(unix)]
mod imp {
    use super::{UNIX_KILL_GRACE, io};
    use rustix::io::Errno;
    use rustix::process::{
        Pid, Signal, getpgid, getpgrp, kill_process_group, test_kill_process_group,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    #[derive(Debug)]
    pub struct Inner {
        pgid: Pid,
        escalating: Arc<AtomicBool>,
    }

    pub fn attach(pid: u32) -> io::Result<Inner> {
        let invalid = |msg: &'static str| io::Error::new(io::ErrorKind::InvalidInput, msg);
        let pid = i32::try_from(pid)
            .ok()
            .filter(|p| *p > 1)
            .and_then(Pid::from_raw)
            .ok_or_else(|| invalid("invalid pid"))?;
        if getpgid(Some(pid))? != pid {
            return Err(invalid("child is not a process-group leader"));
        }
        if pid == getpgrp() {
            return Err(invalid("child shares this process's group"));
        }
        Ok(Inner {
            pgid: pid,
            escalating: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn kill(inner: &Inner) -> io::Result<()> {
        match kill_process_group(inner.pgid, Signal::HUP) {
            Ok(()) => {}
            // Group already empty.
            Err(Errno::SRCH) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        if inner.escalating.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let pgid = inner.pgid;
        let spawned = std::thread::Builder::new()
            .name("skein-tree-kill".into())
            .spawn(move || {
                let deadline = Instant::now() + UNIX_KILL_GRACE;
                while Instant::now() < deadline {
                    if test_kill_process_group(pgid).is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let _ = kill_process_group(pgid, Signal::KILL);
            });
        if let Err(e) = spawned {
            // No escalation thread: go straight to SIGKILL.
            let _ = kill_process_group(inner.pgid, Signal::KILL);
            return Err(e);
        }
        Ok(())
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    use super::io;

    #[derive(Debug)]
    pub struct Inner;

    pub fn attach(_pid: u32) -> io::Result<Inner> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub fn kill(_inner: &Inner) -> io::Result<()> {
        Ok(())
    }
}
