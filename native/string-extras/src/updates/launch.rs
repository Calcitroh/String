//! Transfer only the verified handles, without inheriting them in unrelated children.
use std::{
    ffi::c_void,
    fs::File,
    io,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
        process::{ChildExt, CommandExt},
    },
    process::{Child, Command},
};

#[repr(C)]
#[derive(Default)]
struct BasicLimits {
    process_time: i64,
    job_time: i64,
    flags: u32,
    min_working_set: usize,
    max_working_set: usize,
    active_processes: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}

#[repr(C)]
#[derive(Default)]
struct ExtendedLimits {
    basic: BasicLimits,
    io_counters: [u64; 6],
    memory_limits: [usize; 4],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(security: *const c_void, name: *const u16) -> RawHandle;
    fn SetInformationJobObject(
        job: RawHandle,
        class: i32,
        info: *const ExtendedLimits,
        size: u32,
    ) -> i32;
    fn AssignProcessToJobObject(job: RawHandle, process: RawHandle) -> i32;
    fn GetCurrentProcess() -> RawHandle;
    fn DuplicateHandle(
        source: RawHandle,
        handle: RawHandle,
        target: RawHandle,
        copy: *mut RawHandle,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn ResumeThread(thread: RawHandle) -> u32;
}

fn job_limits(job: &OwnedHandle, flags: u32) -> io::Result<()> {
    let mut info = ExtendedLimits::default();
    info.basic.flags = flags;
    // JobObjectExtendedLimitInformation takes the entire initialized Win32 structure.
    if unsafe { SetInformationJobObject(job.as_raw_handle(), 9, &info, size_of_val(&info) as u32) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn spawn<'a>(
    command: &mut Command,
    flags: u32,
    guards: impl Iterator<Item = &'a File>,
) -> io::Result<Child> {
    // If the player exits mid-transfer, Windows must release the duplicates
    // in the suspended child as well. Only the player owns this job handle.
    let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    job_limits(&job, 0x2000)?; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE

    let mut child = command.creation_flags(flags | 4).spawn()?; // CREATE_SUSPENDED
    let handoff = (|| {
        let process = child.as_raw_handle();
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        for file in guards {
            let mut copy = std::ptr::null_mut();
            // The duplicate lives in this specific child and cannot be inherited.
            if unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    file.as_raw_handle(),
                    process,
                    &mut copy,
                    0,
                    0,
                    2,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        if unsafe { ResumeThread(child.main_thread_handle().as_raw_handle()) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        // The installer now owns the guards. Its own children launch normally.
        job_limits(&job, 0x1000) // JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK
    })();
    if let Err(error) = handoff {
        let _ = child.kill();
        drop(job);
        let _ = child.wait();
        return Err(error);
    }
    Ok(child)
}
