use super::{Version, invalid};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use zip::ZipArchive;

const REPARSE: u32 = 0x400;
const OPEN_REPARSE: u32 = 0x0020_0000;
const DIRECTORY: u32 = 0x0200_0000;

// Open the directory itself and deny delete sharing until its children are
// finished. A checked directory cannot be renamed out from under extraction.
fn pin_dir(path: &Path) -> io::Result<File> {
    let open = || {
        OpenOptions::new()
            .access_mode(0x80)
            .share_mode(1)
            .custom_flags(OPEN_REPARSE | DIRECTORY)
            .open(path)
    };
    let f = match open() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            match fs::create_dir(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
            open()?
        }
        r => r?,
    };
    let m = f.metadata()?;
    if !m.is_dir() || m.file_attributes() & REPARSE != 0 {
        return Err(invalid("Updates require a directory without links"));
    }
    Ok(f)
}
pub(super) struct UpdateFolder {
    pub path: PathBuf,
    _parents: Vec<File>,
    _lock: File,
}
impl UpdateFolder {
    pub fn open(data: &Path) -> io::Result<Self> {
        // The caller supplies the profile location. Pin the profile itself and
        // every directory we own below it, including the updates directory.
        let data = std::path::absolute(data)?;
        let path = data.join("updates");
        let parents = vec![pin_dir(&data)?, pin_dir(&path)?];
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
            _parents: parents,
            _lock: lock,
        })
    }

    pub fn unpack(&self, archive: File, version: Version) -> io::Result<PathBuf> {
        let stage = tempfile::Builder::new()
            .prefix(".stage-")
            .tempdir_in(&self.path)?;
        let mut pinned = HashMap::new();
        pinned.insert(PathBuf::new(), pin_dir(stage.path())?);
        let mut zip = ZipArchive::new(archive)?;
        let required = [
            r"app\String.exe",
            r"app\setup\install.ps1",
            r"app\setup\version.txt",
        ];
        let mut present = [false; 3];
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
                    pinned.insert(p.to_owned(), pin_dir(&stage.path().join(p))?);
                }
            }
            if !entry.is_dir() {
                for (i, name) in required.iter().enumerate() {
                    if rel.to_string_lossy().eq_ignore_ascii_case(name) {
                        present[i] = true;
                    }
                }
                let mut out = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .share_mode(0)
                    .custom_flags(OPEN_REPARSE)
                    .open(stage.path().join(rel))?;
                io::copy(&mut entry, &mut out)?;
            }
        }
        drop(zip);
        if present.contains(&false) {
            return Err(invalid("The signed release is missing installer files"));
        }
        let regular = |path: &str| -> io::Result<File> {
            let f = OpenOptions::new()
                .read(true)
                .share_mode(1)
                .custom_flags(OPEN_REPARSE)
                .open(stage.path().join(path))?;
            let m = f.metadata()?;
            if !m.is_file() || m.file_attributes() & REPARSE != 0 {
                return Err(invalid("Expected a regular installer file"));
            }
            Ok(f)
        };
        regular("app/String.exe")?;
        regular("app/setup/install.ps1")?;
        let mut inside = Vec::new();
        regular("app/setup/version.txt")?
            .take(4097)
            .read_to_end(&mut inside)?;
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
        drop(pinned);
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
        let ready = self.path.join(format!("String-{version}"));
        fs::rename(stage.path(), &ready)?;
        Ok(ready)
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
