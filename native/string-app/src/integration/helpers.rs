//! Helper programs: add-ons that are programs of their own, with their own window. An add-on folder
//! (`<data>\addons\<id>\addon.json`) that names a `program` and a `button` gets a small round button in the sidebar's
//! Playlists header; clicking it starts the program, which adds songs by running `String.exe --add <paths>`.
//! `native\HELPERS.md` says how to write one.
use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fmt, fs, io,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

/// What `--string-version` says.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The glyph a button gets when `button.glyph` names none of String's.
pub const FALLBACK_GLYPH: &str = "plus";
/// An `addon.json` bigger than this is taken as broken.
const MAX_MANIFEST: u64 = 1 << 20;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const BACKUP_SEMANTICS: u32 = 0x0200_0000;

/// The glyph names String draws (`ui\glyphs.json`).
static GLYPHS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    serde_json::from_str::<HashMap<String, serde::de::IgnoredAny>>(include_str!(
        "../ui/glyphs.json"
    ))
    .map(|m| m.into_keys().collect())
    .unwrap_or_default()
});

/// One helper, as its `addon.json` describes it, its program checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Helper {
    /// The add-on's folder name.
    pub id: String,
    pub name: String,
    /// `button.title`: the button's tooltip.
    pub title: String,
    /// `button.glyph`, or [`FALLBACK_GLYPH`] when String has no glyph by that name.
    pub glyph: String,
    pub folder: PathBuf,
    /// `<folder>\<program>`: an `.exe` directly inside the add-on's folder, no link on the way.
    pub program: PathBuf,
}

/// Why an add-on folder that names a program gets no button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Refusal {
    /// `addon.json` can't be read, isn't a JSON object, or `program` isn't a string.
    Broken,
    /// The add-ons folder, the add-on's folder, its `addon.json` or its program is a link (symbolic link, junction).
    Link,
    /// `program` names something other than a file directly inside the add-on's own folder.
    Outside,
    /// `program` isn't an `.exe`.
    NotExe,
    /// `program` isn't there.
    Missing,
    /// A `program` with no `button` object.
    NoButton,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Refusal::Broken => "addon.json can't be read, or its program isn't a file name",
            Refusal::Link => "a link (symbolic link or junction) on the way to its program",
            Refusal::Outside => "its program must be a file directly inside its own folder",
            Refusal::NotExe => "its program must be an .exe",
            Refusal::Missing => "its program isn't there",
            Refusal::NoButton => "it has a program but no button",
        })
    }
}

/// What [`list`] found: the helpers, by folder name, and the add-ons it turned down, with why.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub helpers: Vec<Helper>,
    pub refused: Vec<(String, Refusal)>,
}

/// The helpers in `<data>\addons\`, in folder-name order. Folders that aren't add-ons (no `addon.json`, a name that
/// isn't an add-on id) and add-ons without a `program` (String 1.x's) are passed over; ones that name a program
/// wrongly are listed in `refused`.
pub fn list(data: &Path) -> Listing {
    let mut out = Listing::default();
    let addons = data.join("addons");
    match fs::symlink_metadata(&addons) {
        Ok(m) if is_link(&m) => {
            out.refused.push(("addons".into(), Refusal::Link));
            return out;
        }
        Ok(m) if m.is_dir() => {}
        _ => return out,
    }
    let Ok(entries) = fs::read_dir(&addons) else {
        return out;
    };
    let mut folders: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
        .filter(|(id, _)| is_id(id))
        .collect();
    folders.sort_by_key(|(id, _)| id.to_ascii_lowercase());
    for (id, folder) in folders {
        match read(&id, &folder) {
            Ok(Some(h)) => out.helpers.push(h),
            Ok(None) => {}
            Err(why) => out.refused.push((id, why)),
        }
    }
    out
}

/// An add-on id, as String 1.x takes it: a lowercase letter or digit, then up to 40 more of those or dashes.
pub fn is_id(id: &str) -> bool {
    let b = id.as_bytes();
    let ok = |c: &u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    !b.is_empty() && b.len() <= 41 && ok(&b[0]) && b[1..].iter().all(|c| ok(c) || *c == b'-')
}

fn is_link(m: &fs::Metadata) -> bool {
    m.file_type().is_symlink() || m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// One add-on folder: `Ok(None)` when it isn't a helper (not an add-on, or one without a program).
fn read(id: &str, folder: &Path) -> Result<Option<Helper>, Refusal> {
    let Ok(meta) = fs::symlink_metadata(folder) else {
        return Ok(None);
    };
    if is_link(&meta) {
        return Err(Refusal::Link);
    }
    if !meta.is_dir() {
        return Ok(None);
    }
    let manifest = folder.join("addon.json");
    let Ok(meta) = fs::symlink_metadata(&manifest) else {
        return Ok(None);
    };
    if is_link(&meta) {
        return Err(Refusal::Link);
    }
    if !meta.is_file() || meta.len() > MAX_MANIFEST {
        return Err(Refusal::Broken);
    }
    let json: serde_json::Value = fs::read(&manifest)
        .ok()
        .and_then(|b| serde_json::from_slice(b.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&b)).ok())
        .ok_or(Refusal::Broken)?;
    let j = json.as_object().ok_or(Refusal::Broken)?;
    let Some(program) = j.get("program") else {
        return Ok(None);
    };
    let program = program.as_str().ok_or(Refusal::Broken)?;
    let program = check_program(folder, program)?;
    let button = j
        .get("button")
        .and_then(|b| b.as_object())
        .ok_or(Refusal::NoButton)?;
    let text = |o: &serde_json::Map<String, serde_json::Value>, k: &str| {
        o.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let name = text(j, "name").unwrap_or_else(|| id.to_owned());
    let glyph = text(button, "glyph")
        .filter(|g| GLYPHS.contains(g))
        .unwrap_or_else(|| FALLBACK_GLYPH.into());
    Ok(Some(Helper {
        id: id.to_owned(),
        title: text(button, "title").unwrap_or_else(|| name.clone()),
        name,
        glyph,
        folder: folder.to_owned(),
        program,
    }))
}

/// `program` as `addon.json` gives it: a plain `.exe` file name, the file directly inside `folder`, not a link.
pub fn check_program(folder: &Path, program: &str) -> Result<PathBuf, Refusal> {
    let plain = !program.is_empty()
        && program != "."
        && program != ".."
        && !program.ends_with([' ', '.'])
        && !program.chars().any(|c| {
            c.is_control() || matches!(c, '\\' | '/' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
        && Path::new(program).file_name() == Some(OsStr::new(program));
    if !plain {
        return Err(Refusal::Outside);
    }
    let lower = program.to_ascii_lowercase();
    if lower.len() <= 4 || !lower.ends_with(".exe") {
        return Err(Refusal::NotExe);
    }
    let path = folder.join(program);
    let meta = fs::symlink_metadata(&path).map_err(|_| Refusal::Missing)?;
    if is_link(&meta) {
        return Err(Refusal::Link);
    }
    if !meta.is_file() {
        return Err(Refusal::Outside);
    }
    // and once every link is resolved, still in this very folder
    let real = fs::canonicalize(&path).map_err(|_| Refusal::Missing)?;
    let home = fs::canonicalize(folder).map_err(|_| Refusal::Missing)?;
    if real.parent() != Some(home.as_path()) {
        return Err(Refusal::Outside);
    }
    Ok(path)
}

/// Open the entry itself. Its type comes from the retained handle. Sharing only reads protects
/// the name and contents (including reparse data) until process creation finishes.
fn pin_entry(path: &Path, directory: bool) -> io::Result<fs::File> {
    let handle = fs::OpenOptions::new()
        // An attributes-only handle does not participate in Windows share checks.
        .access_mode(0x81) // FILE_READ_ATTRIBUTES | FILE_READ_DATA (FILE_LIST_DIRECTORY on a directory)
        .share_mode(1) // FILE_SHARE_READ
        .custom_flags(OPEN_REPARSE_POINT | BACKUP_SEMANTICS)
        .open(path)?;
    let m = handle.metadata()?;
    if is_link(&m) || (directory && !m.is_dir()) || (!directory && !m.is_file()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unexpected helper file type",
        ));
    }
    Ok(handle)
}

struct LaunchFiles {
    helper: Helper,
    _pins: Vec<fs::File>,
}

fn prepare_launch(data: &Path, id: &str) -> io::Result<LaunchFiles> {
    let folder = data.canonicalize()?.join("addons").join(id);
    let mut dirs: Vec<_> = folder
        .ancestors()
        .filter(|p| p.parent().is_some())
        .collect();
    dirs.reverse();
    let mut pins = Vec::with_capacity(dirs.len() + 2);
    for dir in dirs {
        pins.push(pin_entry(dir, true)?);
    }
    pins.push(pin_entry(&folder.join("addon.json"), false)?);
    let helper = read(id, &folder)
        .map_err(|why| io::Error::new(io::ErrorKind::PermissionDenied, why.to_string()))?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no longer a helper"))?;
    pins.push(pin_entry(&helper.program, false)?);
    Ok(LaunchFiles {
        helper,
        _pins: pins,
    })
}

/// What a click on a helper's button did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Started {
    Started,
    /// The copy String started last still runs: none started. True when its window was brought forward.
    AlreadyOpen(bool),
    /// A self-test: helpers are listed and drawn, never started.
    SelfTest,
}

struct Run {
    pid: u32,
    alive: Arc<AtomicBool>,
}

/// The helpers String lists, and the ones it started.
pub struct Helpers {
    data: PathBuf,
    smoke: bool,
    list: Vec<Helper>,
    running: HashMap<String, Run>,
    reported: HashSet<String>,
    ctx: Option<egui::Context>,
    focused: bool,
    /// The helper ids whose buttons the last frame drew, in order (for the self-test).
    pub drawn: Vec<String>,
    /// The self-test's verdict on starting: `Some(true)` once every listed helper was turned down as it should be.
    pub smoke_start: Option<bool>,
}

impl Helpers {
    pub fn new(data: &Path, smoke: bool) -> Self {
        Self {
            data: data.to_owned(),
            smoke,
            list: vec![],
            running: HashMap::new(),
            reported: HashSet::new(),
            ctx: None,
            focused: false,
            drawn: vec![],
            smoke_start: None,
        }
    }

    /// Wakes the window when a helper String started closes, so its button stops showing it open.
    pub fn attach(&mut self, ctx: &egui::Context) {
        self.ctx = Some(ctx.clone());
    }

    /// Lists the helpers again. Returns the add-ons turned down that weren't reported before, for the error log.
    pub fn refresh(&mut self) -> Vec<String> {
        let found = list(&self.data);
        self.list = found.helpers;
        found
            .refused
            .into_iter()
            .map(|(id, why)| format!("{id}: {why}"))
            .filter(|m| self.reported.insert(m.clone()))
            .collect()
    }

    /// Lists again when String's window gains the focus (a helper may have been put in or taken out meanwhile).
    pub fn focus(&mut self, focused: bool) -> Vec<String> {
        let gained = focused && !self.focused;
        self.focused = focused;
        if gained { self.refresh() } else { vec![] }
    }

    pub fn list(&self) -> &[Helper] {
        &self.list
    }

    /// True while the copy String started last still runs.
    pub fn is_open(&self, id: &str) -> bool {
        self.running
            .get(id)
            .is_some_and(|r| r.alive.load(Ordering::Acquire))
    }

    /// True while any helper String started still runs.
    pub fn any_open(&self) -> bool {
        self.running
            .values()
            .any(|r| r.alive.load(Ordering::Acquire))
    }

    /// Starts the helper: `<program> --string-data <data> --string-exe <String.exe> --string-version <x.y.z>`, in its
    /// own folder, not waited for. Not again while the copy started last still runs (its window comes forward
    /// instead), and never in a self-test.
    pub fn start(&mut self, id: &str) -> io::Result<Started> {
        if !self.list.iter().any(|h| h.id == id) {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no such helper"));
        }
        if self.smoke {
            return Ok(Started::SelfTest);
        }
        if let Some(run) = self.running.get(id)
            && run.alive.load(Ordering::Acquire)
        {
            return Ok(Started::AlreadyOpen(bring_forward(run.pid)));
        }
        let launch = prepare_launch(&self.data, id)?;
        let h = &launch.helper;
        let exe = std::env::current_exe()?;
        let data = std::path::absolute(&self.data)?;
        let mut child = Command::new(&h.program)
            .arg("--string-data")
            .arg(&data)
            .arg("--string-exe")
            .arg(&exe)
            .arg("--string-version")
            .arg(VERSION)
            .current_dir(&h.folder)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        drop(launch);
        let alive = Arc::new(AtomicBool::new(true));
        let pid = child.id();
        let (flag, ctx) = (alive.clone(), self.ctx.clone());
        let watched = thread::Builder::new()
            .name("string-helper".into())
            .spawn(move || {
                let _ = child.wait();
                flag.store(false, Ordering::Release);
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
        if watched.is_err() {
            alive.store(false, Ordering::Release);
        }
        self.running.insert(id.to_owned(), Run { pid, alive });
        Ok(Started::Started)
    }
}

/// Brings the process's main window forward (restored if minimised). False when it has none showing.
fn bring_forward(pid: u32) -> bool {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    use windows::core::BOOL;
    struct Find {
        pid: u32,
        found: Option<HWND>,
    }
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
        let f = unsafe { &mut *(lp.0 as *mut Find) };
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
        let main = owner == f.pid
            && unsafe { IsWindowVisible(hwnd) }.as_bool()
            && unsafe { GetWindow(hwnd, GW_OWNER) }.is_err();
        if main {
            f.found = Some(hwnd);
        }
        BOOL::from(!main)
    }
    let mut f = Find { pid, found: None };
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut f as *mut Find as isize)) };
    let Some(hwnd) = f.found else {
        return false;
    };
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd).as_bool()
    }
}

/// The self-test's case: the helpers in the test profile are listed, their buttons drawn, and a click starts nothing;
/// paths given with `--add` at the start are in the library's inputs.
pub mod smoke {
    use super::Started;
    use crate::smoke::Case;

    pub const SIDEBAR: Case = Case {
        name: "helpers",
        prepare: |v| {
            for msg in v.platform.helpers.refresh() {
                v.platform.log.write("add-ons", msg);
            }
            let ids: Vec<String> = v
                .platform
                .helpers
                .list()
                .iter()
                .map(|h| h.id.clone())
                .collect();
            let turned_down = ids
                .iter()
                .all(|id| matches!(v.platform.helpers.start(id), Ok(Started::SelfTest)));
            v.platform.helpers.smoke_start = Some(turned_down && !v.platform.helpers.any_open());
        },
        ready: |v| {
            let h = &v.platform.helpers;
            let drawn = h.list().iter().all(|x| h.drawn.contains(&x.id));
            let inputs = |key: &str| {
                v.s.field(key)
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str().map(str::to_lowercase))
                    .collect::<Vec<_>>()
            };
            let (roots, files) = (inputs("roots"), inputs("files"));
            let added = v.platform.added_at_start.iter().all(|p| {
                let p = p.to_string_lossy().to_lowercase();
                roots.contains(&p) || files.contains(&p)
            });
            h.smoke_start == Some(true) && drawn && added
        },
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn base() -> tempfile::TempDir {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-runs/helper-checks");
        fs::create_dir_all(&base).unwrap();
        tempfile::tempdir_in(base).unwrap()
    }

    /// `<data>\addons\<id>\` with this addon.json and, when given, an empty file by that name.
    fn addon(data: &Path, id: &str, json: &str, file: Option<&str>) -> PathBuf {
        let dir = data.join("addons").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("addon.json"), json).unwrap();
        if let Some(f) = file {
            fs::write(dir.join(f), b"MZ").unwrap();
        }
        dir
    }

    fn manifest(program: &str) -> String {
        serde_json::json!({
            "id": "x", "name": "Example", "version": "1.0.0", "program": program,
            "button": { "title": "Example: does a thing", "glyph": "down" }
        })
        .to_string()
    }

    #[test]
    fn a_helper_is_listed_with_its_button() {
        let t = base();
        let dir = addon(
            t.path(),
            "example",
            &manifest("example-helper.exe"),
            Some("example-helper.exe"),
        );
        let l = list(t.path());
        assert_eq!(l.refused, vec![]);
        assert_eq!(
            l.helpers,
            vec![Helper {
                id: "example".into(),
                name: "Example".into(),
                title: "Example: does a thing".into(),
                glyph: "down".into(),
                folder: dir.clone(),
                program: dir.join("example-helper.exe"),
            }]
        );
    }

    #[test]
    fn a_utf8_bom_manifest_survives_discovery_and_launch_validation() {
        let t = base();
        let json = format!("\u{feff}{}", manifest("song.exe"));
        addon(t.path(), "sample", &json, Some("song.exe"));
        assert_eq!(list(t.path()).helpers[0].id, "sample");
        assert_eq!(
            prepare_launch(t.path(), "sample").unwrap().helper.id,
            "sample"
        );
    }

    #[test]
    fn launch_keeps_deny_delete_handles_until_creation_finishes() {
        let t = base();
        let dir = addon(t.path(), "sample", &manifest("song.exe"), Some("song.exe"));
        let launch = prepare_launch(t.path(), "sample").unwrap();
        let delete_access = |path: &Path| {
            fs::OpenOptions::new()
                .access_mode(0x0001_0000) // request DELETE access without changing anything
                .share_mode(7)
                .custom_flags(BACKUP_SEMANTICS)
                .open(path)
        };
        for path in [
            t.path().to_owned(),
            t.path().join("addons"),
            dir.clone(),
            dir.join("addon.json"),
            dir.join("song.exe"),
        ] {
            assert_eq!(delete_access(&path).unwrap_err().raw_os_error(), Some(32));
        }
        drop(launch);
        assert!(delete_access(&dir.join("song.exe")).is_ok());
    }

    #[test]
    fn an_unknown_glyph_falls_back_to_plus_and_a_missing_title_to_the_name() {
        let t = base();
        addon(
            t.path(),
            "a",
            r#"{"name":"A","program":"a.EXE","button":{"glyph":"no-such-glyph"}}"#,
            Some("a.EXE"),
        );
        let h = &list(t.path()).helpers[0];
        assert_eq!((h.glyph.as_str(), h.title.as_str()), ("plus", "A"));
    }

    #[test]
    fn add_ons_without_a_program_and_folders_that_are_not_add_ons_are_passed_over() {
        let t = base();
        addon(
            t.path(),
            "web-only",
            r#"{"name":"W","scripts":["w.js"]}"#,
            None,
        );
        addon(t.path(), "Not_An_Id", &manifest("x.exe"), Some("x.exe"));
        fs::create_dir_all(t.path().join("addons").join("no-manifest")).unwrap();
        fs::write(t.path().join("addons").join("stray.txt"), b"").unwrap();
        assert_eq!(list(t.path()), Listing::default());
        // and no addons folder at all
        assert_eq!(list(&t.path().join("nowhere")), Listing::default());
    }

    #[test]
    fn a_broken_addon_json_is_skipped_and_the_rest_still_listed() {
        let t = base();
        addon(t.path(), "broken", "{ not json", None);
        addon(t.path(), "array", "[1,2]", None);
        addon(t.path(), "number", r#"{"program": 5, "button": {}}"#, None);
        addon(t.path(), "good", &manifest("good.exe"), Some("good.exe"));
        let l = list(t.path());
        assert_eq!(
            l.helpers.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            ["good"]
        );
        assert_eq!(
            l.refused,
            vec![
                ("array".into(), Refusal::Broken),
                ("broken".into(), Refusal::Broken),
                ("number".into(), Refusal::Broken)
            ]
        );
    }

    #[test]
    fn a_program_outside_its_own_folder_is_refused() {
        let t = base();
        fs::write(t.path().join("addons-outside.exe"), b"MZ").unwrap();
        let other = addon(t.path(), "other", "{}", Some("other.exe"));
        let sub = addon(t.path(), "sub", &manifest(r"bin\sub.exe"), None);
        fs::create_dir_all(sub.join("bin")).unwrap();
        fs::write(sub.join("bin").join("sub.exe"), b"MZ").unwrap();
        for (id, program) in [
            ("up", r"..\..\addons-outside.exe"),
            ("sibling", r"..\other\other.exe"),
            ("slash", "../other/other.exe"),
            ("absolute", &*other.join("other.exe").to_string_lossy()),
            ("drive", "C:other.exe"),
            ("stream", "x.txt:hidden.exe"),
            ("dots", ".."),
            ("trailing", "x.exe."),
        ] {
            addon(t.path(), id, &manifest(program), None);
        }
        // a folder named like a program
        let dir = addon(t.path(), "folder", &manifest("dir.exe"), None);
        fs::create_dir_all(dir.join("dir.exe")).unwrap();
        let l = list(t.path());
        assert_eq!(l.helpers, vec![]);
        let mut refused = l.refused;
        refused.sort();
        let mut want: Vec<(String, Refusal)> = [
            "absolute", "dots", "drive", "folder", "sibling", "slash", "stream", "sub", "trailing",
            "up",
        ]
        .iter()
        .map(|id| (id.to_string(), Refusal::Outside))
        .collect();
        want.sort();
        assert_eq!(refused, want);
    }

    #[test]
    fn a_program_that_is_not_an_exe_or_not_there_is_refused() {
        let t = base();
        addon(t.path(), "script", &manifest("run.bat"), Some("run.bat"));
        addon(t.path(), "noext", &manifest("helper"), Some("helper"));
        addon(t.path(), "bare", &manifest(".exe"), Some(".exe"));
        addon(t.path(), "gone", &manifest("gone.exe"), None);
        addon(
            t.path(),
            "nobutton",
            r#"{"program":"nb.exe"}"#,
            Some("nb.exe"),
        );
        let l = list(t.path());
        assert_eq!(l.helpers, vec![]);
        assert_eq!(
            l.refused,
            vec![
                ("bare".into(), Refusal::NotExe),
                ("gone".into(), Refusal::Missing),
                ("nobutton".into(), Refusal::NoButton),
                ("noext".into(), Refusal::NotExe),
                ("script".into(), Refusal::NotExe),
            ]
        );
    }

    /// Links need Developer Mode or an administrator for symbolic links; junctions need neither (mklink /J).
    fn junction(link: &Path, target: &Path) -> bool {
        Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    #[test]
    fn a_program_behind_a_link_is_refused() {
        let t = base();
        // the add-on's folder is a junction to a real add-on folder elsewhere
        let elsewhere = t.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("addon.json"), manifest("real.exe")).unwrap();
        fs::write(elsewhere.join("real.exe"), b"MZ").unwrap();
        fs::create_dir_all(t.path().join("addons")).unwrap();
        assert!(junction(
            &t.path().join("addons").join("linked"),
            &elsewhere
        ));
        // a symbolic link to a program, where Windows lets this account make one
        let sym = addon(t.path(), "symlink", &manifest("sym.exe"), None);
        let made =
            std::os::windows::fs::symlink_file(elsewhere.join("real.exe"), sym.join("sym.exe"))
                .is_ok();
        let l = list(t.path());
        assert_eq!(l.helpers, vec![]);
        let mut want = vec![("linked".to_string(), Refusal::Link)];
        if made {
            want.push(("symlink".into(), Refusal::Link));
        } else {
            want.push(("symlink".into(), Refusal::Missing));
        }
        assert_eq!(l.refused, want);
        // and the whole addons folder behind a junction lists nothing
        let t2 = base();
        assert!(junction(
            &t2.path().join("addons"),
            &t.path().join("addons")
        ));
        assert_eq!(
            list(t2.path()).refused,
            vec![("addons".to_string(), Refusal::Link)]
        );
    }

    #[test]
    fn ids_are_string_1x_add_on_ids() {
        for ok in ["a", "0", "my-helper", "x1-2", &"a".repeat(41)] {
            assert!(is_id(ok), "{ok}");
        }
        for bad in ["", "-a", "A", "a_b", "a.b", "ü", &"a".repeat(42)] {
            assert!(!is_id(bad), "{bad}");
        }
    }

    #[test]
    fn added_paths_keep_absolute_ones_that_exist_once() {
        let t = base();
        let song = t.path().join("song.mp3");
        fs::write(&song, b"").unwrap();
        let upper = PathBuf::from(song.to_string_lossy().to_uppercase());
        let got = crate::add_work::classify(vec![
            song.clone(),
            PathBuf::from("song.mp3"),
            t.path().join("missing.mp3"),
            t.path().to_path_buf(),
            upper,
        ]);
        assert_eq!(
            got.into_iter()
                .map(|p| (p.path, p.directory))
                .collect::<Vec<_>>(),
            [(song, false), (t.path().to_path_buf(), true)]
        );
    }

    #[test]
    fn a_self_test_never_starts_a_helper_and_refresh_reports_each_refusal_once() {
        let t = base();
        addon(t.path(), "good", &manifest("good.exe"), Some("good.exe"));
        addon(t.path(), "bad", &manifest("bad.bat"), Some("bad.bat"));
        let mut h = Helpers::new(t.path(), true);
        assert_eq!(h.refresh(), vec![format!("bad: {}", Refusal::NotExe)]);
        assert_eq!(h.refresh(), Vec::<String>::new());
        assert_eq!(h.start("good").unwrap(), Started::SelfTest);
        assert!(!h.any_open());
        assert!(h.start("bad").is_err());
        // a gain of focus lists again
        fs::remove_dir_all(t.path().join("addons").join("good")).unwrap();
        assert!(h.focus(true).is_empty());
        assert!(h.list().is_empty());
    }

    /// A tiny helper, built with this toolchain's rustc: it writes where it runs and its arguments to `seen.txt` in
    /// its working directory, then stays a moment. No window: it's a Windows-subsystem program that draws none.
    fn echo_helper(dir: &Path) -> bool {
        let src = dir.join("echo.rs");
        fs::write(
            &src,
            r#"#![windows_subsystem = "windows"]
fn main() {
    let cwd = std::env::current_dir().unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::fs::write(cwd.join("seen.tmp"), format!("{}\n{}", cwd.display(), args.join("\n"))).unwrap();
    std::fs::rename(cwd.join("seen.tmp"), cwd.join("seen.txt")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
}
"#,
        )
        .unwrap();
        let rustc = Path::new(env!("CARGO")).with_file_name("rustc.exe");
        Command::new(if rustc.exists() {
            rustc.as_os_str()
        } else {
            OsStr::new("rustc")
        })
        .arg("--edition=2021")
        .arg("-o")
        .arg(dir.join("echo.exe"))
        .arg(&src)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    }

    fn wait_until(what: impl Fn() -> bool) -> bool {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < end {
            if what() {
                return true;
            }
            thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn a_click_starts_the_program_in_its_folder_once_at_a_time() {
        let t = base();
        let dir = addon(t.path(), "echo", &manifest("echo.exe"), None);
        assert!(
            echo_helper(&dir),
            "couldn't build the test helper with rustc"
        );
        let mut h = Helpers::new(t.path(), false);
        assert!(h.refresh().is_empty());
        assert_eq!(h.start("echo").unwrap(), Started::Started);
        // still running: not started again (it has no window to bring forward)
        assert_eq!(h.start("echo").unwrap(), Started::AlreadyOpen(false));
        assert!(h.is_open("echo"));
        let seen = dir.join("seen.txt");
        assert!(wait_until(|| seen.exists()));
        let text = fs::read_to_string(&seen).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let exe = std::env::current_exe().unwrap();
        let data = std::path::absolute(t.path()).unwrap();
        assert_eq!(
            fs::canonicalize(lines[0]).unwrap(),
            fs::canonicalize(&dir).unwrap()
        );
        assert_eq!(
            lines[1..],
            [
                "--string-data",
                &*data.to_string_lossy(),
                "--string-exe",
                &*exe.to_string_lossy(),
                "--string-version",
                VERSION,
            ]
        );
        // once it has closed, a click starts it again
        assert!(wait_until(|| !h.is_open("echo")));
        fs::remove_file(&seen).unwrap();
        assert_eq!(h.start("echo").unwrap(), Started::Started);
        assert!(wait_until(|| seen.exists()));
        assert!(wait_until(|| !h.any_open()));
    }

    #[test]
    fn a_program_gone_since_the_listing_is_not_started() {
        let t = base();
        let dir = addon(t.path(), "swap", &manifest("swap.exe"), Some("swap.exe"));
        let mut h = Helpers::new(t.path(), false);
        h.refresh();
        assert_eq!(h.list().len(), 1);
        fs::remove_file(dir.join("swap.exe")).unwrap();
        let e = h.start("swap").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        assert!(!h.any_open());
    }
}
