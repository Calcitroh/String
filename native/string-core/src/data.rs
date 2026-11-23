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

#[cfg(test)]
mod profile_tests;

#[cfg(test)]
#[test]
fn final_inherited_daily_matching_live_must_survive_the_next_save() {
    let temp = tempfile::tempdir().unwrap();
    let backup = temp.path().join("backup");
    let folder = DataFolder::new(temp.path(), &backup);
    let live = folder.path::<State>();
    let date = Local::now().date_naive();
    let daily = backup.join(format!("state-{date}.json"));
    fs::write(&live, br#"{"route":"morning"}"#).unwrap();
    folder.keep_opening_copy_for(date).unwrap();
    let inherited = br#"{ "route": "afternoon" }"#;
    fs::write(&live, inherited).unwrap();
    fs::write(&daily, inherited).unwrap();
    let mut state = folder.read::<State>().unwrap();
    state.value.fields.insert("route".into(), "evening".into());
    assert!(state.save().unwrap().backup_error.is_none());
    assert_eq!(fs::read(&live).unwrap(), fs::read(&daily).unwrap());
    let kept = fs::read_dir(backup.join("opened").join(date.to_string())).unwrap();
    assert!(
        kept.into_iter()
            .any(|e| fs::read(e.unwrap().path()).unwrap() == inherited)
    );
}

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
    daily_written: Option<NaiveDate>,
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
fn decode<T: Model>(
    bytes: &[u8],
    check: impl FnOnce(&crate::Fields) -> io::Result<()>,
) -> io::Result<T> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let json = text::read_json(bytes);
    let parsed = match serde_json::from_slice::<Value>(&json) {
        Ok(value) => value,
        // A syntactically broken cache can be rebuilt. A well-formed document in another
        // format must never become a writable empty cache.
        Err(_)
            if T::CACHE
                && serde_json::from_slice::<&serde_json::value::RawValue>(&json).is_err() =>
        {
            return Ok(T::default());
        }
        Err(e) => return Err(invalid(e)),
    };
    let Value::Object(mut fields) = parsed else {
        return Err(invalid("expected a document object"));
    };
    if fields
        .get("v")
        .is_some_and(|v| v.as_f64() != Some(1.) && v.as_str() != Some("1"))
    {
        return Err(invalid("unsupported document version"));
    }
    check(&fields)?;
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
        self.read_checked(|_| Ok(()))
    }
    fn read_checked<T: Model>(
        &self,
        check: impl FnOnce(&crate::Fields) -> io::Result<()>,
    ) -> io::Result<Document<T>> {
        let path = self.path::<T>();
        let result = (|| {
            let bytes = read_existing(&path)?;
            let value = match bytes.as_deref() {
                Some(b) => decode(b, check)?,
                None => T::default(),
            };
            Ok(Document {
                folder: self.clone(),
                previous: bytes.as_deref().map(hash),
                daily_written: None,
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
        self.keep_opening_copy_for(Local::now().date_naive())
    }
    fn keep_opening_copy_for(&self, date: NaiveDate) -> io::Result<()> {
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
        fs::create_dir_all(&days)?;
        let staging = tempfile::Builder::new()
            .prefix(".copying-")
            .tempdir_in(&days)?;
        let dir = staging.path();
        for name in [State::NAME, Listening::NAME, Library::NAME] {
            if let Some(bytes) = read_existing(&self.root.join(format!("{name}.json")))? {
                replace(dir, &dir.join(format!("{name}.json")), &bytes)?;
            }
            let daily = format!("{name}-{date}.json");
            if let Some(bytes) = read_existing(&self.backup.join(&daily))? {
                replace(dir, &dir.join(daily), &bytes)?;
            }
        }
        if let Err(e) = fs::rename(dir, &today) {
            if today.is_dir() {
                return Ok(());
            }
            return Err(e);
        }
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
        self.load_profile(self.read()?, self.read()?)
    }
    /// A desktop scan uses the saved selections to prune cached rows. Inspect the original
    /// fields before State substitutes defaults or drops damaged list entries.
    pub fn open_for_scan(&self) -> io::Result<Profile> {
        let library = self.read::<Library>()?;
        let state = if library.value.tracks.is_empty() {
            self.read()?
        } else {
            let state = self.read_checked(validate_locations)?;
            if !state.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "state.json is missing. Restore it from backup before opening this saved library.",
                ));
            }
            state
        };
        self.load_profile(state, library)
    }
    fn load_profile(
        &self,
        state: Document<State>,
        library: Document<Library>,
    ) -> io::Result<Profile> {
        let mut listening = self.read::<Listening>()?;
        if let Some(status) = state.value.fields.get("phone") {
            listening.value.recover_legacy_cursor(status);
        }
        Ok(Profile {
            state,
            listening,
            library,
        })
    }
}

fn validate_locations(fields: &crate::Fields) -> io::Result<()> {
    if !fields.contains_key("roots") && !fields.contains_key("files") {
        return Err(invalid(
            "The library locations are missing. Restore state.json from backup before scanning.",
        ));
    }
    let valid_path = |v: &Value| v.as_str().is_some_and(|p| !p.is_empty());
    for key in ["roots", "files"] {
        if let Some(list) = fields.get(key)
            && !list
                .as_array()
                .is_some_and(|paths| paths.iter().all(valid_path))
        {
            return Err(invalid(format!(
                "The '{key}' list is damaged. Restore state.json from backup before scanning."
            )));
        }
    }
    Ok(())
}

fn stage(dir: &Path, bytes: &[u8]) -> io::Result<NamedTempFile> {
    let mut temp = NamedTempFile::new_in(dir)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    Ok(temp)
}
fn replace(dir: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    stage(dir, bytes)?.persist(target).map_err(|e| e.error)?;
    Ok(())
}
fn backup(dir: &Path, name: &str, date: NaiveDate, bytes: &[u8]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let prefix = format!("{name}-");
    replace(dir, &dir.join(format!("{prefix}{date}.json")), bytes)?;
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
    fn protect_inherited_daily(&self, date: NaiveDate) -> io::Result<()> {
        if self.daily_written == Some(date) {
            return Ok(());
        }
        let name = format!("{}-{date}.json", T::NAME);
        let Some(bytes) = read_existing(&self.folder.backup.join(&name))? else {
            return Ok(());
        };
        self.folder.keep_opening_copy_for(date)?;
        let opened = self.folder.backup.join("opened").join(date.to_string());
        if read_existing(&opened.join(name))?.as_deref() == Some(&bytes)
            || read_existing(&opened.join(format!("{}.json", T::NAME)))?.as_deref() == Some(&bytes)
        {
            return Ok(());
        }
        // Another writer may have made a different daily backup since the morning's
        // snapshot. Both the live file and today's backup are about to be replaced.
        let hex: String = hash(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        let kept = opened.join(format!("{}-daily-{hex}.json", T::NAME));
        match stage(&opened, &bytes)?.persist_noclobber(&kept) {
            Ok(_) => Ok(()),
            Err(e)
                if e.error.kind() == io::ErrorKind::AlreadyExists && fs::read(kept)? == bytes =>
            {
                Ok(())
            }
            Err(e) => Err(e.error),
        }
    }
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
        let date = Local::now().date_naive();
        // Before this writer first replaces today's backup, secure the copy it inherited.
        // Retry even after a live save succeeded: the daily file may still be the only good copy.
        let protect_daily = self.protect_inherited_daily(date);
        replace(dir, &path, &bytes)?;
        self.previous = Some(digest);
        let backup_error = protect_daily
            .and_then(|()| backup(&self.folder.backup, T::NAME, date, &bytes))
            .err();
        if backup_error.is_none() {
            self.daily_written = Some(date);
        }
        Ok(SaveResult {
            bytes_changed,
            backup_error,
        })
    }
}
