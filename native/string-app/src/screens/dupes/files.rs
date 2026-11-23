//! Read file identities on the worker that groups or checks the copies.
use super::{CopyToMerge, Moved, Problem};
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    io,
    mem::size_of,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
};
use string_core::library::Track;
use windows::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FileBasicInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Stamp {
    volume: u32,
    index: u64,
    size: u64,
    written: i64,
    changed: i64,
}
impl Stamp {
    fn open(path: &Path, guard: bool) -> io::Result<(Self, File)> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "A full song path is required",
            ));
        }
        let mut options = OpenOptions::new();
        if guard {
            options.read(true).share_mode(FILE_SHARE_READ.0);
        } else {
            options.access_mode(FILE_READ_ATTRIBUTES.0);
        }
        let file = options.open(path)?;
        let handle = HANDLE(file.as_raw_handle());
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        let mut basic = FILE_BASIC_INFO::default();
        // Both queries use the same open file, including when the path has just been replaced.
        unsafe {
            GetFileInformationByHandle(handle, &mut info)?;
            GetFileInformationByHandleEx(
                handle,
                FileBasicInfo,
                (&mut basic as *mut FILE_BASIC_INFO).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )?;
        }
        Ok((
            Self {
                volume: info.dwVolumeSerialNumber,
                index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
                size: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
                written: basic.LastWriteTime,
                changed: basic.ChangeTime,
            },
            file,
        ))
    }
    pub(super) fn seen(t: &Track) -> Option<Self> {
        let (stamp, _) = Self::open(Path::new(&t.path), false).ok()?;
        let unix_ms = (stamp.written - 116_444_736_000_000_000) / 10_000;
        (stamp.size == t.size && unix_ms == t.mtime).then_some(stamp)
    }
}

pub(super) struct Pair {
    pub copy: CopyToMerge,
    pub keep_path: String,
    pub keep: Option<Stamp>,
    pub drop: Option<Stamp>,
}
pub(super) struct Checked {
    pub plan: Vec<CopyToMerge>,
    // Hold the checked files against replacement or writing through dispatch. Release each
    // dropped file immediately before asking the Windows shell to recycle its path.
    guards: HashMap<String, File>,
}
impl Checked {
    pub(super) fn recycle(mut self, mut move_file: impl FnMut(&Path) -> io::Result<()>) -> Moved {
        let mut outcomes = Vec::with_capacity(self.plan.len());
        for copy in &self.plan {
            let path = Path::new(&copy.path);
            let result = (|| {
                // Windows must be able to rename the file into the bin. Keep a read handle
                // denying writes across the shell call, and keep all other paths locked.
                let _read = OpenOptions::new()
                    .read(true)
                    .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_DELETE.0)
                    .open(path)?;
                self.guards.remove(&copy.path);
                move_file(path)
            })();
            outcomes.push(result.map_err(|e| e.to_string()));
        }
        Moved {
            checked: self,
            outcomes,
        }
    }
}
pub(super) fn check(pairs: Vec<Pair>) -> super::FileCheck {
    let mut reread = HashSet::new();
    let mut guards = HashMap::new();
    let mut bad = vec![];
    let mut reason = String::new();
    let mut plan = Vec::with_capacity(pairs.len());
    for pair in pairs {
        for (path, expected) in [(&pair.keep_path, pair.keep), (&pair.copy.path, pair.drop)] {
            if guards.contains_key(path) {
                continue;
            }
            let why = match Stamp::open(Path::new(path), true) {
                Ok((actual, file)) if Some(actual) == expected => {
                    guards.insert(path.clone(), file);
                    continue;
                }
                Ok(_) if expected.is_none() => {
                    "could not be verified against the last library scan"
                }
                Ok(_) => "changed since it was listed",
                Err(e) if e.kind() == io::ErrorKind::NotFound => "is no longer there",
                Err(e) if matches!(e.raw_os_error(), Some(32 | 33)) => "is in use",
                Err(_) => "could not be opened",
            };
            if bad.is_empty() {
                let name = Path::new(path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                reason = format!("{name} {why}. Nothing was merged.");
            }
            reread.insert(path.clone());
            if bad.last() != Some(&pair.copy.drop_id) {
                bad.push(pair.copy.drop_id.clone());
            }
        }
        plan.push(pair.copy);
    }
    if bad.is_empty() {
        Ok(Checked { plan, guards })
    } else {
        Err((Problem { ids: bad, reason }, reread))
    }
}
