//! Read file identities on the worker that groups or checks the copies.
use super::{CopyToMerge, Problem};
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    io,
    mem::size_of,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
};
use string_core::library::{Track, native_path};
use windows::Win32::{
    Foundation::HANDLE,
    Globalization::{CSTR_EQUAL, CompareStringOrdinal},
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_BASIC_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAGS_AND_ATTRIBUTES,
        FILE_GENERIC_READ, FILE_ID_INFO, FILE_SHARE_DELETE, FILE_SHARE_MODE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FileBasicInfo, FileIdInfo, GETFINALPATHNAMEBYHANDLE_FLAGS,
        GetFileAttributesW, GetFileInformationByHandle, GetFileInformationByHandleEx,
        GetFinalPathNameByHandleW, ReOpenFile, VOLUME_NAME_DOS, VOLUME_NAME_GUID,
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Stamp {
    volume: u64,
    index: [u8; 16],
    size: u64,
    written: i64,
    changed: i64,
}
impl Stamp {
    fn open(path: &Path, parents: &mut HashMap<PathBuf, File>) -> io::Result<(Self, File)> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "A full song path is required",
            ));
        }
        guard_parent(path, parents)?;
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)?;
        // Windows resolved each component under its own directory's case rules.
        // guard_parent rejected reparse points. Also refuse substituted drive names.
        let actual = dos_path(&final_name(&file, VOLUME_NAME_DOS)?);
        let submitted = dos_path(&std::path::absolute(path)?);
        if actual != submitted {
            let wide = |p: &Path| p.as_os_str().encode_wide().collect::<Vec<_>>();
            if unsafe { CompareStringOrdinal(&wide(&actual), &wide(&submitted), true) }
                != CSTR_EQUAL
            {
                return Err(io::Error::other("The song path is redirected"));
            }
        }
        Ok((Self::read(&file)?, file))
    }
    fn read(file: &File) -> io::Result<Self> {
        let handle = HANDLE(file.as_raw_handle());
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        let mut basic = FILE_BASIC_INFO::default();
        let mut id = FILE_ID_INFO::default();
        // Query the same open object for all fields. ReFS IDs need all 128 bits.
        unsafe {
            GetFileInformationByHandle(handle, &mut info)?;
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                (&mut id as *mut FILE_ID_INFO).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )?;
            if id.FileId.Identifier == [0; 16] {
                return Err(io::Error::other(
                    "This volume did not identify the song file",
                ));
            }
            if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
                return Err(io::Error::other("This file is a link or reparse point"));
            }
            GetFileInformationByHandleEx(
                handle,
                FileBasicInfo,
                (&mut basic as *mut FILE_BASIC_INFO).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )?;
        }
        Ok(Self {
            volume: id.VolumeSerialNumber,
            index: id.FileId.Identifier,
            size: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
            written: basic.LastWriteTime,
            changed: basic.ChangeTime,
        })
    }
    pub(super) fn seen(t: &Track) -> Option<Self> {
        let path = native_path(&t.path);
        let mut parents = HashMap::new();
        let (stamp, _guard) = Self::open(&path, &mut parents).ok()?;
        let unix_ms = (stamp.written - 116_444_736_000_000_000) / 10_000;
        if stamp.size != t.size || unix_ms != t.mtime || t.read_error != 0 {
            return None;
        }
        // A restored file can have the very same size and timestamp as the old recording.
        // Use the scanner's parser while the file is locked, including its raw tags under edits.
        let tags = string_core::tags::read(&path).ok()?.tags;
        (tags.title == t.tags.title
            && tags.artist == t.tags.artist
            && tags.album == t.tags.album
            && tags.format == t.tags.format
            && (tags.duration - t.tags.duration).abs() < 0.000_001)
            .then_some(stamp)
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
    // Retain the keeper and every pending file until the result has been applied.
    guards: HashMap<String, (Stamp, File)>,
    moved_guard: Option<File>,
    _parents: HashMap<PathBuf, File>,
}
impl Checked {
    pub(super) fn recycle(
        &mut self,
        index: usize,
        move_file: impl FnOnce(&Path) -> io::Result<()>,
    ) -> io::Result<()> {
        let bin = super::bin::Bin::open()?;
        let (_, file) = self.guarded(&self.plan[index].drop_id)?;
        let canonical = final_name(file, VOLUME_NAME_DOS)?;
        let original = dos_path(&canonical);
        let source = if self.plan[index].path.starts_with(r"\\?\") {
            &canonical
        } else {
            &original
        };
        // Give Restore the exact on-disk spelling while the source is still locked.
        self.move_one(
            index,
            |_| move_file(source),
            |moved| {
                recycled_location(moved)
                    && bin
                        .restore_path(moved)
                        .is_ok_and(|recorded| recorded == original)
            },
        )
    }
    fn guarded(&self, id: &str) -> io::Result<&(Stamp, File)> {
        self.guards
            .get(id)
            .ok_or_else(|| io::Error::other("The song is no longer guarded"))
    }
    pub(super) fn move_one(
        &mut self,
        index: usize,
        move_file: impl FnOnce(&Path) -> io::Result<()>,
        in_bin: impl FnOnce(&Path) -> bool,
    ) -> io::Result<()> {
        self.moved_guard = None; // the previous move has been applied
        let path = native_path(&self.plan[index].path);
        let copy = &self.plan[index];
        let (keep_stamp, keeper) = self.guarded(&copy.keep_id)?;
        let (drop_stamp, drop_file) = self.guarded(&copy.drop_id)?;
        let read = reopen(drop_file, FILE_SHARE_READ | FILE_SHARE_DELETE)?;
        // The shell records the submitted path for Restore. A staging name would be restored
        // verbatim. Follow the original object through the call, independently of that path.
        if recycled_location(&final_name(&read, VOLUME_NAME_GUID)?) {
            return Err(io::Error::other("The song is already in the Recycle Bin"));
        }
        if Stamp::read(keeper)? != *keep_stamp || Stamp::read(&read)? != *drop_stamp {
            return Err(io::Error::other(
                "A file in this pair changed before the move",
            ));
        }
        let (_, guard) = self.guards.remove(&copy.drop_id).unwrap();
        drop(guard);
        move_file(&path)?;
        // Keep this object in place until the session commits its history transfer.
        if let Ok(moved) = reopen(&read, FILE_SHARE_READ)
            && final_name(&moved, VOLUME_NAME_DOS).is_ok_and(|p| in_bin(&p))
        {
            self.moved_guard = Some(moved);
            return Ok(());
        }
        Err(io::Error::other(format!(
            "Windows could not confirm which file moved. In the Recycle Bin, look for “{}” with “{}” under Original Location. If the wrong file moved, choose Restore. String kept its history in place.",
            path.file_name().unwrap_or_default().to_string_lossy(),
            path.parent().unwrap_or(&path).display()
        )))
    }
}

fn reopen(file: &File, sharing: FILE_SHARE_MODE) -> io::Result<File> {
    let handle = unsafe {
        ReOpenFile(
            HANDLE(file.as_raw_handle()),
            FILE_GENERIC_READ.0,
            sharing,
            FILE_FLAGS_AND_ATTRIBUTES(0),
        )
    }?;
    Ok(unsafe { File::from_raw_handle(handle.0) })
}

// Keep the containing folder in place even while the shell may rename the leaf.
fn guard_parent(path: &Path, parents: &mut HashMap<PathBuf, File>) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("The song needs a parent folder"))?;
    if dir.parent().is_some() && !parents.contains_key(dir) {
        let file = OpenOptions::new()
            .access_mode(
                (windows::Win32::Storage::FileSystem::FILE_TRAVERSE
                    | windows::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES)
                    .0,
            )
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
            .custom_flags((FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS).0)
            .open(dir)?;
        if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(io::Error::other(
                "A folder in the song path is a link or reparse point",
            ));
        }
        // Ordinary ancestors cannot be renamed over a delete-denying descendant.
        // Windows can return their entry attributes even in a traverse-only tree.
        for ancestor in dir.ancestors().skip(1).take_while(|p| p.parent().is_some()) {
            let name = windows::core::HSTRING::from(ancestor.as_os_str());
            let attrs = unsafe { GetFileAttributesW(&name) };
            if attrs == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            if attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
                return Err(io::Error::other(
                    "A folder in the song path is a link or reparse point",
                ));
            }
        }
        parents.insert(dir.into(), file);
    }
    Ok(())
}

pub(super) fn dos_path(path: &Path) -> PathBuf {
    let mut parts = path.components();
    let Some(Component::Prefix(prefix)) = parts.next() else {
        return path.into();
    };
    let mut out = match prefix.kind() {
        Prefix::VerbatimDisk(letter) => PathBuf::from(format!("{}:\\", char::from(letter))),
        Prefix::VerbatimUNC(server, share) => {
            let mut unc = std::ffi::OsString::from(r"\\");
            unc.push(server);
            unc.push(r"\");
            unc.push(share);
            PathBuf::from(unc)
        }
        _ => return path.into(),
    };
    out.extend(parts.filter(|part| *part != Component::RootDir));
    out
}

fn final_name(file: &File, flags: GETFINALPATHNAMEBYHANDLE_FLAGS) -> io::Result<PathBuf> {
    let mut name = vec![0; 512];
    loop {
        let len =
            unsafe { GetFinalPathNameByHandleW(HANDLE(file.as_raw_handle()), &mut name, flags) }
                as usize;
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        if len < name.len() {
            return Ok(std::ffi::OsString::from_wide(&name[..len]).into());
        }
        name.resize(len + 1, 0);
    }
}

pub(super) fn recycled_location(path: &Path) -> bool {
    let Some(user_bin) = path.parent() else {
        return false;
    };
    let Some(bin) = user_bin.parent() else {
        return false;
    };
    path.file_name()
        .is_some_and(|s| s.to_string_lossy().starts_with("$R"))
        && bin
            .file_name()
            .is_some_and(|s| s.eq_ignore_ascii_case("$Recycle.Bin"))
        && bin.parent().is_some_and(|root| root.parent().is_none())
}
pub(super) fn check(pairs: Vec<Pair>) -> super::FileCheck {
    let mut reread = HashSet::new();
    let mut guards = HashMap::new();
    let mut parents = HashMap::new();
    let mut bad = vec![];
    let mut reason = String::new();
    let mut plan = Vec::with_capacity(pairs.len());
    for pair in pairs {
        for (id, path, expected) in [
            (&pair.copy.keep_id, &pair.keep_path, pair.keep),
            (&pair.copy.drop_id, &pair.copy.path, pair.drop),
        ] {
            if guards
                .get(id)
                .is_some_and(|(stamp, _)| Some(*stamp) == expected)
            {
                continue;
            }
            let path_ref = native_path(path);
            let detail;
            let why = match Stamp::open(&path_ref, &mut parents) {
                Ok((actual, file)) if Some(actual) == expected => {
                    guards.insert(id.clone(), (actual, file));
                    continue;
                }
                Ok(_) if expected.is_none() => {
                    "could not be verified against the last library scan"
                }
                Ok(_) => "changed since it was listed",
                Err(e) if e.kind() == io::ErrorKind::NotFound => "is no longer there",
                Err(e) if matches!(e.raw_os_error(), Some(32 | 33)) => "is in use",
                Err(e) => {
                    detail = format!("could not be verified ({e})");
                    &detail
                }
            };
            if bad.is_empty() {
                let name = path_ref.file_name().unwrap_or_default().to_string_lossy();
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
        Ok(Checked {
            plan,
            guards,
            moved_guard: None,
            _parents: parents,
        })
    } else {
        Err((Problem { ids: bad, reason }, reread))
    }
}
