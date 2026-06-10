//! Windows Job Object backend.
//!
//! Creates a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` so
//! all child processes are terminated when the handle is dropped.
//! Resource limits (memory, CPU) are applied via
//! `SetInformationJobObject`. The child process is assigned to the
//! job after spawn via `AssignProcessToJobObject`.

use crate::policy::SandboxPolicy;
use crate::traits::{Sandbox, SandboxBackend, SandboxResult};

/// Windows Job Object sandbox.
///
/// Holds the Job Object handle alive for the lifetime of the sandbox.
/// When dropped, `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` ensures all
/// processes in the job are terminated.
pub struct WindowsJobSandbox {
    job_handle: windows_sys::Win32::Foundation::HANDLE,
}

// The job handle is a raw pointer managed by us (closed in Drop).
// SAFETY: The handle is only used through &self methods and closed
// in Drop, which is single-threaded by design.
#[allow(unsafe_code)]
unsafe impl Send for WindowsJobSandbox {}
#[allow(unsafe_code)]
unsafe impl Sync for WindowsJobSandbox {}

impl WindowsJobSandbox {
    /// Create a new Job Object with kill-on-close semantics.
    ///
    /// Returns `None` if the OS call fails (caller should fall back to
    /// `NoopSandbox`).
    pub fn try_new() -> Option<Self> {
        let handle = job_helpers::create_job_object()?;
        Some(Self { job_handle: handle })
    }

    /// Create a new sandbox, falling back to a dummy handle on failure.
    ///
    /// Prefer [`try_new`] in production; this exists for ergonomics in
    /// tests and non-critical paths.
    #[must_use]
    pub fn new() -> Self {
        Self::try_new().unwrap_or(Self {
            job_handle: windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE,
        })
    }

    fn is_valid(&self) -> bool {
        self.job_handle != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE
            && !self.job_handle.is_null()
    }
}

impl Default for WindowsJobSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WindowsJobSandbox {
    fn drop(&mut self) {
        if self.is_valid() {
            job_helpers::close_handle(self.job_handle);
        }
    }
}

impl Sandbox for WindowsJobSandbox {
    fn backend(&self) -> SandboxBackend {
        SandboxBackend::WindowsJobObject
    }

    fn is_available(&self) -> bool {
        self.is_valid()
    }

    fn apply(
        &self,
        policy: &SandboxPolicy,
        _cmd: &mut tokio::process::Command,
    ) -> crab_core::Result<SandboxResult> {
        if !self.is_valid() {
            return Ok(SandboxResult {
                applied: false,
                description: "Windows Job Object creation failed; no sandbox".into(),
                backend: SandboxBackend::WindowsJobObject,
            });
        }

        // Apply resource limits to the job object.
        job_helpers::set_limits(self.job_handle, policy)?;

        // Assign the current process to the job. The child inherits
        // the job association when spawned from this process.
        job_helpers::assign_current_process(self.job_handle)?;

        Ok(SandboxResult {
            applied: true,
            description: format!("Windows Job Object: kill-on-close, {}", policy.summary()),
            backend: SandboxBackend::WindowsJobObject,
        })
    }
}

/// Safe wrappers around Windows Job Object FFI calls.
///
/// All `unsafe` blocks are confined to this module.
#[allow(unsafe_code)]
mod job_helpers {
    use std::mem;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    use crate::policy::SandboxPolicy;

    /// Create a Job Object. Returns `None` on failure.
    pub(super) fn create_job_object() -> Option<HANDLE> {
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            tracing::warn!("CreateJobObjectW failed");
            None
        } else {
            Some(handle)
        }
    }

    /// Close a handle, logging on failure.
    pub(super) fn close_handle(handle: HANDLE) {
        let ok = unsafe { CloseHandle(handle) };
        if ok == 0 {
            tracing::warn!("CloseHandle failed: {}", std::io::Error::last_os_error());
        }
    }

    /// Apply extended limit information to the job object.
    pub(super) fn set_limits(handle: HANDLE, policy: &SandboxPolicy) -> crab_core::Result<()> {
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
        let mut limit_flags: u32 = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        if policy.max_memory_bytes > 0 {
            limit_flags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
            limits.JobMemoryLimit = policy.max_memory_bytes as usize;
        }

        if !policy.allow_subprocess {
            limit_flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            limits.BasicLimitInformation.ActiveProcessLimit = 1;
        }

        limits.BasicLimitInformation.LimitFlags = limit_flags;

        // CPU time limit: PerProcessUserTimeLimit is in 100-ns units.
        if policy.max_cpu_seconds > 0 {
            limits.BasicLimitInformation.PerProcessUserTimeLimit =
                (policy.max_cpu_seconds * 10_000_000).cast_signed();
        }

        let size = mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size,
            )
        };
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            tracing::warn!("SetInformationJobObject failed: {err}");
            return Err(err.into());
        }
        Ok(())
    }

    /// Assign the current process to the job object.
    pub(super) fn assign_current_process(job: HANDLE) -> crab_core::Result<()> {
        let process = unsafe { GetCurrentProcess() };
        let ok = unsafe { AssignProcessToJobObject(job, process) };
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            tracing::warn!("AssignProcessToJobObject failed: {err}");
            return Err(err.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_available_returns_true() {
        let sandbox = WindowsJobSandbox::new();
        assert!(sandbox.is_available());
        assert_eq!(sandbox.backend(), SandboxBackend::WindowsJobObject);
    }

    #[tokio::test]
    async fn apply_sets_limits_and_returns_applied() {
        let sandbox = WindowsJobSandbox::new();
        if !sandbox.is_available() {
            return;
        }
        let policy = SandboxPolicy::deny_all()
            .with_max_memory(128 * 1024 * 1024)
            .with_max_cpu(60);
        let mut cmd = tokio::process::Command::new("cmd");
        let result = sandbox.apply(&policy, &mut cmd).unwrap();
        assert_eq!(result.backend, SandboxBackend::WindowsJobObject);
        assert!(result.applied);
    }

    #[test]
    fn drop_closes_handle_without_panic() {
        let sandbox = WindowsJobSandbox::new();
        drop(sandbox);
    }

    #[test]
    fn try_new_returns_some_on_windows() {
        let result = WindowsJobSandbox::try_new();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn apply_deny_subprocess_limits_active_process() {
        let sandbox = WindowsJobSandbox::new();
        if !sandbox.is_available() {
            return;
        }
        let policy = SandboxPolicy::deny_all().with_subprocess(false);
        let mut cmd = tokio::process::Command::new("cmd");
        let result = sandbox.apply(&policy, &mut cmd).unwrap();
        assert!(result.applied);
        assert!(result.description.contains("subprocess:no"));
    }
}
