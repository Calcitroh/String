use super::{Version, hash_copy, invalid, powershell_directory};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use zip::ZipArchive;

const REPARSE: u32 = 0x400;
const OPEN_REPARSE: u32 = 0x0020_0000;
const DIRECTORY: u32 = 0x0200_0000;

// Attributes-only handles do not participate in Windows' delete-sharing checks.
pub(super) fn open_dir(path: &Path) -> io::Result<File> {
    let f = OpenOptions::new()
        .access_mode(0x81) // FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES
        .share_mode(1) // FILE_SHARE_READ
        .custom_flags(OPEN_REPARSE | DIRECTORY)
        .open(path)?;
    let m = f.metadata()?;
    if !m.is_dir() || m.file_attributes() & REPARSE != 0 {
        return Err(invalid("Updates require a directory without links"));
    }
    Ok(f)
}
// Resolve from the root down, opening each component without following links.
// The paths are also passed to Windows PowerShell without a verbatim prefix.
pub(super) fn pin_ancestors(path: &Path) -> io::Result<Vec<File>> {
    for part in path.components() {
        match part {
            std::path::Component::Normal(name) => {
                entry_path(
                    name.to_str()
                        .ok_or_else(|| invalid("Invalid update directory name"))?,
                )?;
            }
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {}
            _ => return Err(invalid("Updates require an absolute normalized directory")),
        }
    }
    let mut parents: Vec<_> = path.ancestors().skip(1).collect();
    parents.reverse();
    parents.into_iter().map(open_dir).collect()
}

fn pin_or_create_dir(path: &Path) -> io::Result<File> {
    match open_dir(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            match fs::create_dir(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
            open_dir(path)
        }
        result => result,
    }
}

#[derive(Debug)]
pub(super) struct PreparedRelease {
    pub path: PathBuf,
    pub script: PathBuf,
    files: Vec<File>,
    directories: HashMap<PathBuf, File>,
    folder: UpdateFolder,
}

impl PreparedRelease {
    pub fn guards(&self) -> impl Iterator<Item = &File> {
        self.files
            .iter()
            .chain(self.directories.values())
            .chain(&self.folder.parents)
            .chain([&self.folder.lock])
    }
}

#[derive(Debug)]
pub(super) struct UpdateFolder {
    pub path: PathBuf,
    parents: Vec<File>,
    lock: File,
}
impl UpdateFolder {
    pub fn open(data: &Path) -> io::Result<Self> {
        let absolute = std::path::absolute(data)?;
        let mut parents = pin_ancestors(&absolute)?;
        let data =
            powershell_directory(&absolute).ok_or_else(|| invalid("Unsupported update path"))?;
        let path = data.join("updates");
        parents.push(pin_or_create_dir(&data)?);
        parents.push(pin_or_create_dir(&path)?);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .custom_flags(OPEN_REPARSE)
            .open(path.join(".prepare.lock"))?;
        if lock.metadata()?.file_attributes() & REPARSE != 0 {
            return Err(invalid("The update lock is a link"));
        }
        Ok(Self {
            path,
            parents,
            lock,
        })
    }

    pub fn unpack(self, archive: File, version: Version) -> io::Result<PreparedRelease> {
        // Extract under the final name so publishing never requires releasing the guards.
        // A retry keeps the previous preparation intact until the new one passes validation.
        let name = format!("String-{version}");
        let stage = tempfile::Builder::new()
            .prefix(&name)
            .rand_bytes(0)
            .tempdir_in(&self.path)
            .or_else(|e| match e.kind() {
                io::ErrorKind::AlreadyExists => tempfile::Builder::new()
                    .prefix(&format!("{name}-"))
                    .tempdir_in(&self.path),
                _ => Err(e),
            })?;
        let mut pinned = HashMap::new();
        let mut files = Vec::new();
        pinned.insert(PathBuf::new(), open_dir(stage.path())?);
        let mut zip = ZipArchive::new(archive)?;
        let required = [
            r"app\String.exe",
            r"app\setup\install.ps1",
            r"app\setup\version.txt",
        ];
        let mut present = [None, None, None];
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.is_symlink()
                || entry.external_attributes() & REPARSE != 0
                || entry
                    .unix_mode()
                    .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o040000 | 0o100000))
            {
                return Err(invalid("A release entry is a link or special file"));
            }
            let rel = entry_path(&entry.name()?)?;
            let directory = if entry.is_dir() {
                rel.as_path()
            } else {
                rel.parent().unwrap()
            };
            for p in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
                if !pinned.contains_key(p) {
                    let dir = pin_or_create_dir(&stage.path().join(p))?;
                    // Setup opens these directories with fixed spellings. A case
                    // variant must resolve to the same guarded directory.
                    for fixed in ["app", r"app\setup"] {
                        if spelling_differs(p, fixed) {
                            same_file(&dir, &open_dir(&stage.path().join(fixed))?)?;
                        }
                    }
                    pinned.insert(p.to_owned(), dir);
                }
            }
            if !entry.is_dir() {
                let path = stage.path().join(&rel);
                let mut out = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .share_mode(0)
                    .custom_flags(OPEN_REPARSE)
                    .open(&path)?;
                let expected = hash_copy(&mut entry, &mut out)?;
                drop(out);
                // Readers such as PowerShell deny existing writers. Reopen read-only,
                // then authenticate the bytes under the same guard we hand to the child.
                let mut guard = OpenOptions::new()
                    .read(true)
                    .share_mode(1)
                    .custom_flags(OPEN_REPARSE)
                    .open(&path)?;
                let meta = guard.metadata()?;
                if !meta.is_file()
                    || meta.file_attributes() & REPARSE != 0
                    || hash_copy(&mut guard, io::sink())? != expected
                {
                    return Err(invalid("An extracted release file failed verification"));
                }
                // Fixed lookups made by the installer must reach the guarded
                // file even on a volume with case-sensitive directories.
                for fixed in [
                    required[0],
                    required[2],
                    r"app\setup\files.txt",
                    r"app\setup\uninstall.ps1",
                    r"app\setup\imports.txt",
                ] {
                    if spelling_differs(&rel, fixed) {
                        let alias = OpenOptions::new()
                            .read(true)
                            .share_mode(1)
                            .custom_flags(OPEN_REPARSE)
                            .open(stage.path().join(fixed))?;
                        same_file(&guard, &alias)?;
                    }
                }
                for (slot, fixed) in present.iter_mut().zip(required) {
                    if rel.to_string_lossy().eq_ignore_ascii_case(fixed)
                        && slot.replace((path.clone(), files.len())).is_some()
                    {
                        return Err(invalid("Ambiguous installer file in the signed release"));
                    }
                }
                files.push(guard);
            }
        }
        drop(zip);
        let [Some(_), Some((script, _)), Some((_, version_index))] = present else {
            return Err(invalid("The signed release is missing installer files"));
        };
        let version_file = &mut files[version_index];
        version_file.rewind()?;
        let mut inside = Vec::new();
        version_file.take(4097).read_to_end(&mut inside)?;
        if inside.len() > 4096
            || crate::text::host_text(&inside)
                .trim()
                .parse::<Version>()
                .ok()
                != Some(version)
        {
            return Err(invalid(
                "The version inside the signed release differs from the release name",
            ));
        }
        // Only clear old preparations after the new signed payload is ready.
        for item in fs::read_dir(&self.path)? {
            let item = item?;
            if item.path() == stage.path() || item.file_name() == ".prepare.lock" {
                continue;
            }
            let p = item.path();
            let metadata = fs::symlink_metadata(&p)?;
            if metadata.file_attributes() & REPARSE != 0 {
                if metadata.file_attributes() & 0x10 != 0 {
                    fs::remove_dir(p)?;
                } else {
                    fs::remove_file(p)?;
                }
            } else if metadata.is_dir() {
                fs::remove_dir_all(p)?;
            } else {
                fs::remove_file(p)?;
            }
        }
        Ok(PreparedRelease {
            path: stage.keep(),
            script,
            files,
            directories: pinned,
            folder: self,
        })
    }
}
fn spelling_differs(path: &Path, fixed: &str) -> bool {
    let name = path.as_os_str().to_string_lossy();
    name != fixed && name.eq_ignore_ascii_case(fixed)
}

fn same_file(guard: &File, alias: &File) -> io::Result<()> {
    if super::file_id(guard).is_some_and(|id| super::file_id(alias) == Some(id)) {
        Ok(())
    } else {
        Err(invalid(
            "Installer path does not resolve to the signed entry",
        ))
    }
}

pub(super) fn entry_path(name: &str) -> io::Result<PathBuf> {
    let mut path = PathBuf::new();
    let body = name.strip_suffix(['\\', '/']).unwrap_or(name);
    for part in body.split(['\\', '/']) {
        let stem = part
            .split('.')
            .next()
            .unwrap()
            .trim_end()
            .to_ascii_uppercase();
        let device = matches!(
            stem.as_str(),
            "AUX" | "CON" | "PRN" | "NUL" | "CONIN$" | "CONOUT$"
        ) || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|n| {
                matches!(
                    n,
                    "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            });
        if part.is_empty()
            || part.ends_with(['.', ' '])
            || device
            || part.chars().any(|c| c < ' ' || "<>:\"|?*".contains(c))
        {
            return Err(invalid("The archive contains an invalid Windows path"));
        }
        path.push(part);
    }
    Ok(path)
}
