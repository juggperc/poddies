//! Process containment.
//!
//! On Windows the worker is assigned to a Job Object configured with:
//!
//! * `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` — when the host exits (or simply
//!   drops the handle), the worker dies. No orphaned plugin processes.
//! * `JOB_OBJECT_LIMIT_PROCESS_MEMORY` — a runaway plugin is stopped by the OS
//!   instead of by the user's fan.
//! * `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` — a hard ceiling on how many processes
//!   the worker may have alive at once, so a plugin cannot fork-bomb the
//!   machine. It is not 1: a Python plugin legitimately needs the worker plus
//!   an interpreter (and the Windows `py` launcher adds another hop).
//!
//! What this is *not*: a security boundary. A native plugin runs with the
//! user's privileges and can read what the user can read. This contains
//! crashes, hangs and resource abuse — the realistic failure modes for a
//! plugin — and that is the honest, documented limit of the model.

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    /// Owns a Job Object. Dropping it kills the worker.
    pub struct Sandbox {
        job: HANDLE,
    }

    /// Worker + interpreter + the `py` launcher hop + one slot of slack.
    const ACTIVE_PROCESS_LIMIT: u32 = 4;

    // SAFETY: a HANDLE is a plain kernel handle with no thread affinity.
    unsafe impl Send for Sandbox {}
    unsafe impl Sync for Sandbox {}

    impl Sandbox {
        pub fn attach(child: &Child, memory_limit_bytes: usize) -> io::Result<Self> {
            // SAFETY: all pointers passed are valid for the duration of the call.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return Err(io::Error::last_os_error());
                }

                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                    | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                    | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
                limits.BasicLimitInformation.ActiveProcessLimit = ACTIVE_PROCESS_LIMIT;
                limits.ProcessMemoryLimit = memory_limit_bytes;

                let ok = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const std::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    let error = io::Error::last_os_error();
                    CloseHandle(job);
                    return Err(error);
                }

                let process = child.as_raw_handle() as HANDLE;
                if AssignProcessToJobObject(job, process) == 0 {
                    let error = io::Error::last_os_error();
                    CloseHandle(job);
                    return Err(error);
                }

                Ok(Self { job })
            }
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            // Closing the job handle triggers KILL_ON_JOB_CLOSE.
            unsafe { CloseHandle(self.job) };
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io;
    use std::process::Child;

    /// No-op on non-Windows. cgroups/rlimits would go here for a Linux port.
    pub struct Sandbox;

    impl Sandbox {
        pub fn attach(_child: &Child, _memory_limit_bytes: usize) -> io::Result<Self> {
            Ok(Self)
        }
    }
}

pub use imp::Sandbox;
