//! Explicit data folders, guarded reads, atomic replacement and daily backups.

use crate::{library::Library, state::State, stats::Listening};
use chrono::{Local, NaiveDate};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
pub(crate) mod text;
use serde_json::{Value, json};

pub(crate) fn object(value: Value) -> crate::Fields {
    match value {
        Value::Object(fields) => fields,
        _ => Default::default(),
    }
}
pub(crate) fn array(value: Value) -> Vec<Value> {
    match value {
        Value::Array(rows) => rows,
        _ => Vec::new(),
    }
}
pub(crate) fn take(fields: &mut crate::Fields, key: &str) -> Value {
    fields.remove(key).unwrap_or(Value::Null)
}

pub fn default_data_dir() -> Option<PathBuf> {
    env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("String"))
}

mod sealed {
    pub trait Sealed {}
}
pub trait Model: sealed::Sealed + Serialize + DeserializeOwned + Default {
    const NAME: &'static str;
    const CACHE: bool = false;
    fn valid(&self) -> bool;
}
impl sealed::Sealed for State {}
impl Model for State {
    const NAME: &'static str = "state";
    fn valid(&self) -> bool {
        self.v == 1
    }
}
impl sealed::Sealed for Listening {}
impl Model for Listening {
    const NAME: &'static str = "listening";
    fn valid(&self) -> bool {
        true
    }
}
impl sealed::Sealed for Library {}
impl Model for Library {
    const NAME: &'static str = "library";
    const CACHE: bool = true;
    fn valid(&self) -> bool {
        self.v == 1
    }
}

#[derive(Clone, Debug)]
pub struct DataFolder {
    root: PathBuf,
    backup: PathBuf,
}
pub struct Document<T> {
    folder: DataFolder,
    previous: Option<[u8; 32]>,
    pub value: T,
}
#[derive(Debug)]
pub struct SaveResult {
    pub bytes_changed: bool,
    pub backup_error: Option<io::Error>,
}
pub struct Profile {
    pub state: Document<State>,
    pub listening: Document<Listening>,
    pub library: Document<Library>,
}

fn read_existing(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn invalid(message: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn decode<T: Model>(bytes: &[u8]) -> io::Result<T> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let parsed = serde_json::from_slice::<Value>(&text::read_json(bytes)).map_err(invalid)?;
    let mut fields = object(parsed);
    // A numeric version for a different format must never be rewritten as version one.
    if fields
        .get("v")
        .and_then(Value::as_f64)
        .is_some_and(|v| v != 1.)
    {
        return Err(invalid("unsupported document version"));
    }
    fields.insert("v".into(), json!(1));
    let value: T = serde_json::from_value(Value::Object(fields)).map_err(invalid)?;
    if !value.valid() {
        return Err(invalid("unsupported document version"));
    }
    Ok(value)
}

// JSON.stringify writes whole seconds (including the 24 empty hour buckets) as integers.
struct Compact;
impl serde_json::ser::Formatter for Compact {
    fn write_f64<W: Write + ?Sized>(&mut self, w: &mut W, n: f64) -> io::Result<()> {
        if n.fract() == 0. && n.abs() <= 9_007_199_254_740_991. {
            return self.write_i64(w, n as i64);
        }
        serde_json::ser::CompactFormatter.write_f64(w, n)
    }
}

impl DataFolder {
    pub fn new(root: impl Into<PathBuf>, backup: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            backup: backup.into(),
        }
    }
    fn path<T: Model>(&self) -> PathBuf {
        self.root.join(format!("{}.json", T::NAME))
    }
    /// An I/O failure blocks every model. Only the rebuildable song cache may have broken JSON.
    pub fn read<T: Model>(&self) -> io::Result<Document<T>> {
        let path = self.path::<T>();
        let result = (|| {
            let bytes = read_existing(&path)?;
            let value = match bytes.as_deref() {
                Some(b) => match decode(b) {
                    Err(_) if T::CACHE => T::default(),
                    result => result?,
                },
                None => T::default(),
            };
            Ok(Document {
                folder: self.clone(),
                previous: bytes.as_deref().map(hash),
                value,
            })
        })();
        result.map_err(|e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", path.display())))
    }
    /// The first time String opens this profile on a day, a copy of its files as they are, and of today's daily
    /// backups, in `<backup>/opened/<date>/`; ten days are kept. The daily backup holds what String last saved that
    /// day, so a profile another version of String saved earlier the same day would otherwise have no copy from before
    /// this version's first save (and with a file missing, today's backup of it may be the only copy left).
    pub fn keep_opening_copy(&self) -> io::Result<()> {
        let date = Local::now().date_naive();
        let days = self.backup.join("opened");
        let today = days.join(date.to_string());
        if today.is_dir() {
            return Ok(());
        }
        if today.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} is in the way", today.display()),
            ));
        }
        let staging = days.join(".copying");
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir_all(&staging)?;
        for name in [State::NAME, Listening::NAME, Library::NAME] {
            if let Some(bytes) = read_existing(&self.root.join(format!("{name}.json")))? {
                replace(&staging, &staging.join(format!("{name}.json")), &bytes)?;
            }
            let daily = format!("{name}-{date}.json");
            if let Some(bytes) = read_existing(&self.backup.join(&daily))? {
                replace(&staging, &staging.join(daily), &bytes)?;
            }
        }
        fs::rename(&staging, &today)?;
        let mut kept = vec![];
        for entry in fs::read_dir(&days)? {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && entry.file_name().to_str().is_some_and(|s| {
                    s.len() == 10 && NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
                })
            {
                kept.push(entry.path());
            }
        }
        kept.sort_unstable_by(|a, b| b.cmp(a));
        for old in kept.into_iter().skip(10) {
            fs::remove_dir_all(old)?;
        }
        Ok(())
    }
    /// Load the whole profile before enabling saves in the application.
    pub fn open(&self) -> io::Result<Profile> {
        Ok(Profile {
            state: self.read()?,
            listening: self.read()?,
            library: self.read()?,
        })
    }
}

fn replace(dir: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut temp = NamedTempFile::new_in(dir)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(target).map_err(|e| e.error)?;
    Ok(())
}
fn backup(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let prefix = format!("{name}-");
    replace(
        dir,
        &dir.join(format!("{prefix}{}.json", Local::now().date_naive())),
        bytes,
    )?;
    let mut dates = vec![];
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if let Some(s) = name
            .to_str()
            .and_then(|s| s.strip_prefix(&prefix))
            .and_then(|s| s.strip_suffix(".json"))
            && s.len() == 10
            && NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
            && entry.file_type()?.is_file()
        {
            dates.push(entry.path());
        }
    }
    dates.sort_unstable_by(|a, b| b.cmp(a));
    for old in dates.into_iter().skip(60) {
        fs::remove_file(old)?;
    }
    Ok(())
}
impl<T: Model> Document<T> {
    pub fn exists(&self) -> bool {
        self.previous.is_some()
    }
    pub fn json(&self) -> io::Result<Vec<u8>> {
        if !self.value.valid() {
            return Err(invalid("unsupported document version"));
        }
        let mut bytes = Vec::new();
        self.value
            .serialize(&mut serde_json::Serializer::with_formatter(
                &mut bytes, Compact,
            ))
            .map_err(invalid)?;
        Ok(text::write_json(bytes))
    }
    pub fn save(&mut self) -> io::Result<SaveResult> {
        let bytes = self.json()?;
        let dir = &self.folder.root;
        fs::create_dir_all(dir)?;
        // The stable sibling lock survives replacement and is released by the OS on exit.
        let lock: File = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(format!(".{}.lock", T::NAME)))?;
        lock.lock()?;
        let path = self.folder.path::<T>();
        if read_existing(&path)?.as_deref().map(hash) != self.previous {
            return Err(io::Error::other(format!(
                "{} changed since load; reload before saving",
                path.display()
            )));
        }
        let digest = hash(&bytes);
        let bytes_changed = self.previous != Some(digest);
        replace(dir, &path, &bytes)?;
        self.previous = Some(digest);
        let backup_error = backup(&self.folder.backup, T::NAME, &bytes).err();
        Ok(SaveResult {
            bytes_changed,
            backup_error,
        })
    }
}
