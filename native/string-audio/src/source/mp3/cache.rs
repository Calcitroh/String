//! A level-two Windows oplock lets another handle read freely. A write or truncate
//! breaks it without waiting for our acknowledgement. Only reuse an index while the
//! kernel still grants that lease, including on volumes with coarse timestamps.
use super::{Index, scan};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use symphonia::core::errors::Result;
use symphonia::core::formats::Track;

type Handle = *mut c_void;

#[derive(Default, PartialEq, Eq, Copy, Clone)]
#[repr(C)]
struct FileId {
    volume: u64,
    id: [u8; 16],
}

// OVERLAPPED with its byte-offset union initialized to zero.
#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offsets: [u32; 2],
    event: Handle,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFileInformationByHandleEx(file: Handle, class: i32, out: *mut FileId, size: u32) -> i32;
    fn ReOpenFile(file: Handle, access: u32, share: u32, flags: u32) -> Handle;
    fn CreateEventW(
        attributes: *const c_void,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> Handle;
    fn DeviceIoControl(
        file: Handle,
        code: u32,
        input: *const c_void,
        input_len: u32,
        output: *mut c_void,
        output_len: u32,
        returned: *mut u32,
        request: *mut Overlapped,
    ) -> i32;
    fn WaitForSingleObject(event: Handle, timeout_ms: u32) -> u32;
    fn CancelIoEx(file: Handle, request: *mut Overlapped) -> i32;
    fn GetOverlappedResult(
        file: Handle,
        request: *mut Overlapped,
        bytes: *mut u32,
        wait: i32,
    ) -> i32;
}

fn file_id(file: &File) -> Option<FileId> {
    let mut id = FileId::default();
    // FileIdInfo returns a volume serial and a 128-bit ID for this open object.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            18,
            &mut id,
            size_of::<FileId>() as u32,
        )
    };
    (ok != 0 && id.id != [0; 16]).then_some(id)
}

struct ReadLease {
    file: OwnedHandle,
    event: OwnedHandle,
    request: Box<Overlapped>,
}

impl ReadLease {
    fn acquire(file: &File) -> Option<Self> {
        const GENERIC_READ: u32 = 0x80000000;
        const SHARE_READ_WRITE_DELETE: u32 = 7;
        const FILE_FLAG_OVERLAPPED: u32 = 0x40000000;
        const FSCTL_REQUEST_OPLOCK_LEVEL_2: u32 = 0x90004;
        const ERROR_IO_PENDING: i32 = 997;
        // ReOpenFile refers to the same object even if its path was replaced. Keep
        // asynchronous I/O on a separate handle from the synchronous decoder.
        unsafe {
            let raw = ReOpenFile(
                file.as_raw_handle(),
                GENERIC_READ,
                SHARE_READ_WRITE_DELETE,
                FILE_FLAG_OVERLAPPED,
            );
            if raw == -1isize as Handle {
                return None;
            }
            let file = OwnedHandle::from_raw_handle(raw);
            let raw = CreateEventW(ptr::null(), 1, 0, ptr::null());
            if raw.is_null() {
                return None;
            }
            let event = OwnedHandle::from_raw_handle(raw);
            let mut request = Box::new(Overlapped {
                internal: 0,
                internal_high: 0,
                offsets: [0; 2],
                event: event.as_raw_handle(),
            });
            let ok = DeviceIoControl(
                file.as_raw_handle(),
                FSCTL_REQUEST_OPLOCK_LEVEL_2,
                ptr::null(),
                0,
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                request.as_mut(),
            );
            if ok == 0 && io::Error::last_os_error().raw_os_error() == Some(ERROR_IO_PENDING) {
                Some(Self {
                    file,
                    event,
                    request,
                })
            } else {
                None
            }
        }
    }

    fn intact(&self) -> bool {
        const WAIT_TIMEOUT: u32 = 258;
        unsafe { WaitForSingleObject(self.event.as_raw_handle(), 0) == WAIT_TIMEOUT }
    }
}

impl Drop for ReadLease {
    fn drop(&mut self) {
        // The kernel owns the OVERLAPPED until the break or cancellation completes.
        unsafe {
            CancelIoEx(self.file.as_raw_handle(), self.request.as_mut());
            let mut bytes = 0;
            GetOverlappedResult(
                self.file.as_raw_handle(),
                self.request.as_mut(),
                &mut bytes,
                1,
            );
        }
    }
}

thread_local! {
    static RECENT: RefCell<VecDeque<(FileId, ReadLease, Index)>> = const { RefCell::new(VecDeque::new()) };
}

pub(super) fn index(file: &mut File, track: &Track, rate: u32) -> Result<Index> {
    let id = file_id(file);
    let cached = id.and_then(|id| {
        RECENT.with_borrow(|items| {
            items
                .iter()
                .find(|(key, lease, _)| *key == id && lease.intact())
                .map(|(_, _, index)| index.clone())
        })
    });
    if let Some(index) = cached {
        return Ok(index);
    }

    let lease = id.and_then(|id| ReadLease::acquire(file).map(|lease| (id, lease)));
    let (index, walked) = scan(file, file.metadata()?.len(), track, rate)?;
    if walked
        && let Some((id, lease)) = lease
        && lease.intact()
    {
        RECENT.with_borrow_mut(|items| {
            items.retain(|(key, old, _)| *key != id && old.intact());
            items.truncate(3);
            items.push_front((id, lease, index.clone()));
        });
    }
    Ok(index)
}
