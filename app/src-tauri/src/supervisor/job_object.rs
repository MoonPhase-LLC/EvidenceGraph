//! Windows Job Object process guard (S1-04 requirement 7's "process guard
//! ... mechanism" for abnormal exit paths). Assigns the spawned child to a
//! job configured with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: if *this*
//! process (the Tauri host) terminates for any reason -- crash, a forced
//! kill, anything that skips normal Rust drop/cleanup code -- Windows
//! itself terminates every process still in the job, including the
//! child, without this process needing to still be running to do it.
//! This is independent of, and a backstop for,
//! `process::spawn`'s `kill_on_drop` and the explicit graceful-shutdown
//! path in `supervisor::shutdown` -- those cover the normal-exit cases;
//! this covers this process disappearing without running any of its own
//! cleanup code at all.

#![cfg(windows)]

use std::io;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

pub struct JobObject {
    handle: HANDLE,
}

// The raw job-object handle is not thread-affine (ordinary Win32 kernel
// object handles are safe to use from any thread); this type only exposes
// narrow, safe operations on it (`assign`), and closes it exactly once in
// `Drop`.
unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

impl JobObject {
    /// Creates an unnamed job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_
    /// CLOSE` set, so the job closing (this process exiting, cleanly or
    /// not) kills every process still assigned to it.
    pub fn create() -> io::Result<Self> {
        // SAFETY: `CreateJobObjectW` with null security attributes and no
        // name is the documented pattern for an anonymous, process-local
        // job object; the returned handle is checked for null (failure)
        // immediately below before any further use.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        // SAFETY: `handle` is a valid, just-created job object handle;
        // `info` is a correctly initialized, correctly sized structure
        // matching `JobObjectExtendedLimitInformation`'s documented
        // layout, and outlives this call.
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            let err = io::Error::last_os_error();
            // SAFETY: `handle` was just created above and is not used
            // again after this call.
            unsafe { CloseHandle(handle) };
            return Err(err);
        }

        Ok(Self { handle })
    }

    /// Assigns `child` to this job. Must be called after spawn (there is
    /// no way to pre-assign a not-yet-existing process) and before the
    /// child can meaningfully outlive this process.
    pub fn assign(&self, child: &tokio::process::Child) -> io::Result<()> {
        let raw_handle = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("child process has no raw handle"))?;
        // SAFETY: `self.handle` is a valid job object handle owned by
        // `self` for at least the duration of this call; `raw_handle` is
        // a valid process handle owned by `child` for at least the
        // duration of this call.
        let ok = unsafe { AssignProcessToJobObject(self.handle, raw_handle as HANDLE) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        // SAFETY: `self.handle` is a valid handle owned by `self` and is
        // not used again after this call. Note: closing this handle is
        // itself how the "kill everything in the job" behavior fires on a
        // *clean* exit path too (the job has no other open handle once
        // this one closes) -- `supervisor::shutdown`'s explicit graceful
        // shutdown already stops the child first, so this is a no-op
        // there, not a race with it.
        unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_succeeds() {
        JobObject::create().expect("job object creation should succeed on Windows");
    }

    #[tokio::test]
    async fn assign_a_real_child_process_succeeds() {
        let job = JobObject::create().unwrap();
        let mut child = tokio::process::Command::new("cmd.exe")
            .args(["/C", "exit 0"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("cmd.exe should be spawnable in this test environment");

        job.assign(&child)
            .expect("assigning a live child to the job should succeed");

        let _ = child.wait().await;
    }
}
