//! Ending a program and everything it started. On Linux and macOS the program is made the leader of its own process group at
//! spawn and the group gets the signals. On Windows it is put in a job object that ends all its members when told to, and when
//! the `Group` is dropped (which closes the job). A `Group` belongs to whoever holds the child, so a process id that Windows
//! reuses later never reaches another program's job.
use tokio::process::Command;

/// Called on the command before it is spawned.
pub fn configure(command: &mut Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    no_window(command);
}

/// As `no_window`, for a `std::process::Command`.
pub fn no_window_std(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Windows: a console program started by the desktop app (which has no console) would open a console window of its own each
/// time; it is started without one. Nothing on Linux and macOS.
pub fn no_window(command: &mut Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// What can end a started program and everything it started.
pub struct Group {
    #[cfg(unix)]
    pid: u32,
    #[cfg(windows)]
    job: usize,
}

/// Called right after the spawn with the child's id. Linux and macOS: always a group. Windows: puts the program in a job, and
/// None when that failed, in which case the caller ends the program and gives up the start rather than run something that
/// cannot be ended. A grandchild started before this returns is outside the job (a short window).
pub fn adopt(pid: u32) -> Option<Group> {
    #[cfg(unix)]
    {
        Some(Group { pid })
    }
    #[cfg(windows)]
    {
        win::adopt(pid).map(|job| Group { job })
    }
}

impl Group {
    /// Asks the group to end: SIGTERM on Linux and macOS. Windows has no such request for a job; the caller's later `kill` ends it.
    pub fn term(&self) {
        #[cfg(unix)]
        {
            // SAFETY: killpg with a process group id this host created (process_group(0) at spawn).
            unsafe {
                libc::killpg(self.pid as i32, libc::SIGTERM);
            }
        }
    }

    /// Ends the group now (SIGKILL; on Windows the job is terminated). A group that already ended makes this a no-op.
    pub fn kill(&self) {
        #[cfg(unix)]
        {
            // SAFETY: as above.
            unsafe {
                libc::killpg(self.pid as i32, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        win::kill(self.job);
    }
}

#[cfg(windows)]
impl Drop for Group {
    fn drop(&mut self) {
        win::close(self.job);
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

    /// A new job that ends its members when its last handle closes, holding the program `pid`; the handle as a number.
    pub fn adopt(pid: u32) -> Option<usize> {
        // SAFETY: plain Win32 calls; every handle opened here is closed here or by `close`.
        unsafe {
            let job: HANDLE = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(job, JobObjectExtendedLimitInformation, std::ptr::addr_of!(info).cast(), std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            let assigned = set != 0 && !process.is_null() && AssignProcessToJobObject(job, process) != 0;
            if !process.is_null() {
                CloseHandle(process);
            }
            if assigned {
                Some(job as usize)
            } else {
                CloseHandle(job);
                None
            }
        }
    }

    pub fn kill(job: usize) {
        // SAFETY: the handle came from `adopt` and is open until `close`.
        unsafe {
            TerminateJobObject(job as HANDLE, 1);
        }
    }

    pub fn close(job: usize) {
        // SAFETY: closed once, from the owner's Drop.
        unsafe {
            CloseHandle(job as HANDLE);
        }
    }
}
