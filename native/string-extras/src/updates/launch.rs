//! Transfer only the verified handles, without inheriting them in unrelated children.
use std::{
    ffi::{OsStr, OsString, c_void},
    fs::File,
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
    },
    path::{Path, PathBuf},
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

#[repr(C)]
#[derive(Default)]
struct StartupInfoEx {
    size: u32,
    reserved_names: [*mut u16; 3],
    window_fields: [u32; 8],
    show: u16,
    reserved_size: u16,
    reserved: *mut u8,
    stdio: [RawHandle; 3],
    attributes: *mut c_void,
}

#[repr(C)]
#[derive(Default)]
struct ProcessInfo {
    process: RawHandle,
    thread: RawHandle,
    ids: [u32; 2],
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
    fn InitializeProcThreadAttributeList(
        list: *mut c_void,
        count: u32,
        flags: u32,
        size: *mut usize,
    ) -> i32;
    fn UpdateProcThreadAttribute(
        list: *mut c_void,
        flags: u32,
        attribute: usize,
        value: *const RawHandle,
        size: usize,
        previous: *mut c_void,
        length: *mut usize,
    ) -> i32;
    fn DeleteProcThreadAttributeList(list: *mut c_void);
    fn GetSystemDirectoryW(path: *mut u16, size: u32) -> u32;
    fn CreateProcessW(
        app: *const u16,
        command: *mut u16,
        process_sa: *const c_void,
        thread_sa: *const c_void,
        inherit_handles: i32,
        flags: u32,
        env: *const u16,
        cwd: *const u16,
        startup: *mut StartupInfoEx,
        info: *mut ProcessInfo,
    ) -> i32;
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
    fn WaitForSingleObject(handle: RawHandle, millis: u32) -> u32;
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}

// Keep the parent's environment, replacing only our named entries. Let Windows
// PowerShell initialize its own module path even when invoked from PowerShell 7.
fn environment(paths: &[(&str, &Path)]) -> Vec<u16> {
    let mut entries: Vec<_> = std::env::vars_os()
        .filter(|(key, _)| {
            let key = key.to_string_lossy();
            !key.eq_ignore_ascii_case("PSModulePath")
                && !paths.iter().any(|(k, _)| key.eq_ignore_ascii_case(k))
        })
        .collect();
    entries.extend(
        paths
            .iter()
            .map(|(k, p)| (OsString::from(k), p.as_os_str().to_owned())),
    );
    entries.sort_by_cached_key(|(k, _)| {
        k.to_string_lossy()
            .to_uppercase()
            .encode_utf16()
            .collect::<Vec<_>>()
    });
    let mut block = Vec::new();
    for (key, value) in entries {
        block.extend(key.encode_wide());
        block.push(b'=' as u16);
        block.extend(wide(&value));
    }
    block.push(0);
    block
}

fn create_suspended(
    job: &OwnedHandle,
    paths: &[(&str, &Path)],
    flags: u32,
) -> io::Result<(OwnedHandle, OwnedHandle)> {
    use std::ptr::{null, null_mut};
    // All variable paths are environment values. The command line contains only fixed source.
    const COMMAND: &str = concat!(
        "powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \"",
        "Set-Location -LiteralPath $env:STRING_UPDATE_RELEASE -ErrorAction Stop; ",
        "[Environment]::CurrentDirectory = $env:STRING_UPDATE_RELEASE; ",
        "$Host.UI.RawUI.WindowTitle = 'Updating String'; ",
        "& $env:STRING_UPDATE_SCRIPT -To $env:STRING_UPDATE_TO; ",
        "if ($LASTEXITCODE) { Read-Host 'Press Enter to close' }\""
    );
    let mut system = vec![0u16; 32768];
    let n = unsafe { GetSystemDirectoryW(system.as_mut_ptr(), system.len() as u32) } as usize;
    if n == 0 || n >= system.len() {
        return Err(io::Error::last_os_error());
    }
    let dir = PathBuf::from(OsString::from_wide(&system[..n]));
    let exe = wide(
        dir.join(r"WindowsPowerShell\v1.0\powershell.exe")
            .as_os_str(),
    );
    let mut command = wide(COMMAND.as_ref());
    let env = environment(paths);
    let mut size = 0;
    unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size) };
    if size == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
    let list = storage.as_mut_ptr().cast();
    if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut size) } == 0 {
        return Err(io::Error::last_os_error());
    }
    const PROC_THREAD_ATTRIBUTE_JOB_LIST: usize = 0x2000d;
    let raw_job = job.as_raw_handle();
    let mut startup = StartupInfoEx {
        size: size_of::<StartupInfoEx>() as u32,
        attributes: list,
        ..Default::default()
    };
    let mut info = ProcessInfo::default();
    let ok = unsafe {
        UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_JOB_LIST,
            &raw_job,
            size_of_val(&raw_job),
            null_mut(),
            null_mut(),
        ) != 0
            && CreateProcessW(
                exe.as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                0,
                flags | 0x80000 | 0x400 | 4, // EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED
                env.as_ptr(),
                system.as_ptr(),
                &mut startup,
                &mut info,
            ) != 0
    };
    let error = io::Error::last_os_error();
    unsafe { DeleteProcThreadAttributeList(list) };
    if !ok {
        return Err(error);
    }
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(info.process),
            OwnedHandle::from_raw_handle(info.thread),
        )
    })
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

fn handoff_job() -> io::Result<OwnedHandle> {
    // If the player exits mid-transfer, Windows must release the duplicates
    // in the suspended child as well. Only the player owns this job handle.
    let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    job_limits(&job, 0x2000)?; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    Ok(job)
}

pub(super) fn spawn<'a>(
    paths: &[(&str, &Path)],
    flags: u32,
    guards: impl Iterator<Item = &'a File>,
) -> io::Result<OwnedHandle> {
    let job = handoff_job()?;
    let (child, thread) = create_suspended(&job, paths, flags)?;
    if let Err(error) = transfer(&job, child.as_raw_handle(), thread.as_raw_handle(), guards) {
        // Closing the job terminates the child. Wait for it to release its
        // duplicated guards before handing the preparation lock back to a retry.
        drop(job);
        unsafe { WaitForSingleObject(child.as_raw_handle(), u32::MAX) };
        return Err(error);
    }
    Ok(child)
}

fn transfer<'a>(
    job: &OwnedHandle,
    process: RawHandle,
    thread: RawHandle,
    guards: impl Iterator<Item = &'a File>,
) -> io::Result<()> {
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
    if unsafe { ResumeThread(thread) } == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    // The installer now owns the guards. Its own children launch normally.
    job_limits(job, 0x1000) // JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK
}

#[test]
fn closing_job_before_handle_transfer_reaps_suspended_child() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn IsProcessInJob(process: RawHandle, job: RawHandle, member: *mut i32) -> i32;
        fn TerminateProcess(process: RawHandle, code: u32) -> i32;
    }
    let job = handoff_job().unwrap();
    let (child, _thread) = create_suspended(&job, &[], 0x08000000).unwrap();
    let process = child.as_raw_handle();
    let mut member = 0;
    let queried = unsafe { IsProcessInJob(process, job.as_raw_handle(), &mut member) };
    drop(job);
    let waited = unsafe { WaitForSingleObject(process, 5000) };
    if waited != 0 {
        unsafe { TerminateProcess(process, 1) };
    }
    assert_ne!(queried, 0);
    assert_ne!(member, 0);
    assert_eq!(waited, 0);
}
