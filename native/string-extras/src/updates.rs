//! Check a published release and prepare its signed installer for the app.
mod launch;
#[cfg(test)]
mod tests;
mod unpack;
use base64::{Engine, engine::general_purpose::STANDARD};
use p256::{
    ecdsa::{Signature, VerifyingKey, signature::hazmat::PrehashVerifier},
    pkcs8::DecodePublicKey,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fmt, fs,
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, Instant},
};
use ureq::{Body, http::Response};

const FEED: &str = "https://api.github.com/repos/Calcitroh/String/releases/latest";
const KEY: &str = "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEc1dW08TlzhlcEqhsyFH7VHRAZn4I0I4O4NCTobsdY45jozA5DKGqz7GqVs5f4rtkBzstC78qXU0PM0RTwK3c3Q==";
const MAX_RELEASE: u64 = 600 << 20;
const METADATA: Duration = Duration::from_secs(600);
const RELEASE: Duration = Duration::from_secs(7200);
const SILENCE: Duration = Duration::from_secs(60);

fn invalid(s: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s.into())
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct Version(u32, u32, u32);
impl FromStr for Version {
    type Err = io::Error;
    fn from_str(s: &str) -> io::Result<Self> {
        let parts = s
            .trim()
            .split('.')
            .map(|s| {
                s.trim()
                    .parse::<i32>()
                    .ok()
                    .and_then(|n| u32::try_from(n).ok())
            })
            .collect::<Option<Vec<_>>>();
        let Some(parts) = parts.filter(|v| (2..=4).contains(&v.len())) else {
            return Err(invalid("Expected a release version number"));
        };
        Ok(Self(parts[0], parts[1], parts.get(2).copied().unwrap_or(0)))
    }
}
impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}
#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub version: Version,
    pub notes: String,
    pub size: u64,
}
struct Release {
    info: ReleaseInfo,
    zip: String,
    sig: String,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Feed {
    draft: bool,
    prerelease: bool,
    tag_name: String,
    body: Option<String>,
    assets: Option<Vec<Asset>>,
}
#[derive(Debug)]
pub struct Check {
    pub current: Version,
    pub installed: bool,
    pub latest: Option<ReleaseInfo>,
    pub newer: bool,
}
#[derive(Debug, Copy, Clone)]
pub enum Progress {
    Receiving { got: u64, total: u64 },
    Verifying,
}
#[derive(Debug)]
pub struct InstallPlan {
    pub version: Version,
    release: unpack::PreparedRelease,
    to: PathBuf,
    _install_dir: fs::File,
    install_parents: Vec<fs::File>,
}
impl InstallPlan {
    /// Hand the verified files and their directory guards to the installer process.
    pub fn spawn(&self) -> io::Result<std::os::windows::io::OwnedHandle> {
        self.spawn_command(&[], 0x10) // CREATE_NEW_CONSOLE
    }

    fn spawn_command(
        &self,
        extra: &[(&str, &Path)],
        flags: u32,
    ) -> io::Result<std::os::windows::io::OwnedHandle> {
        let mut paths = vec![
            ("STRING_UPDATE_RELEASE", self.release.path.as_path()),
            ("STRING_UPDATE_SCRIPT", self.release.script.as_path()),
            ("STRING_UPDATE_TO", self.to.as_path()),
        ];
        paths.extend_from_slice(extra);
        launch::spawn(
            &paths,
            flags,
            self.release.guards().chain(&self.install_parents),
        )
    }
}
/// What a drive letter points at, as Windows' DOS device names report it: a SUBST letter (`\??\C:\…`) or a network
/// mapping can be pointed elsewhere while an update runs, and the installer takes the install folder by its path.
fn remappable(device: &str) -> bool {
    device.starts_with(r"\??\")
        || device.contains("LanmanRedirector")
        || device.contains(r"\Device\Mup")
        || device.contains("WebDavRedirector")
}

/// Automatic updates need String on a letter Windows gives a local volume; anywhere else the person installs the new
/// version with its installer.
fn fixed_drive(path: &Path) -> io::Result<()> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn QueryDosDeviceW(name: *const u16, target: *mut u16, max: u32) -> u32;
    }
    let refuse = || {
        invalid(
            "String is installed on a SUBST or network drive, so it doesn't update itself: install the new version \
             with its installer",
        )
    };
    let letter = match path.components().next() {
        Some(std::path::Component::Prefix(p)) => match p.kind() {
            std::path::Prefix::Disk(l) | std::path::Prefix::VerbatimDisk(l) => l as char,
            _ => return Err(refuse()),
        },
        _ => return Err(refuse()),
    };
    let name: Vec<u16> = format!("{letter}:").encode_utf16().chain([0]).collect();
    let mut target = vec![0u16; 4096];
    let n = unsafe { QueryDosDeviceW(name.as_ptr(), target.as_mut_ptr(), target.len() as u32) };
    if n == 0 {
        return Err(io::Error::last_os_error());
    }
    let end = target.iter().position(|&c| c == 0).unwrap_or(n as usize);
    if remappable(&String::from_utf16_lossy(&target[..end])) {
        return Err(refuse());
    }
    Ok(())
}

pub struct Updater {
    install: PathBuf,
    current: Version,
    silence: Duration,
    feed: String,
    key: String,
    #[cfg(any(test, feature = "test-feed"))]
    fixture: Option<String>,
}

// FILE_ID_INFO identifies a file on its volume, including on case-sensitive NTFS.
#[repr(C)]
#[derive(Default, PartialEq)]
struct FileId {
    volume: u64,
    id: [u8; 16],
}
fn file_id(file: &fs::File) -> Option<FileId> {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            file: *mut std::ffi::c_void,
            class: i32,
            info: *mut FileId,
            size: u32,
        ) -> i32;
    }
    let mut info = FileId::default();
    // The live file handle and writable buffer cover the entire FileIdInfo call.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            18,
            &mut info,
            size_of::<FileId>() as u32,
        )
    };
    (ok != 0 && info.id != [0; 16]).then_some(info)
}

fn powershell_directory(path: &Path) -> Option<PathBuf> {
    use std::path::{Component, Prefix};
    let Component::Prefix(prefix) = path.components().next()? else {
        return None;
    };
    match prefix.kind() {
        Prefix::VerbatimDisk(_) => Some(path.to_str()?.strip_prefix(r"\\?\")?.into()),
        Prefix::VerbatimUNC(_, _) => Some(format!(r"\\{}", path.to_str()?.get(8..)?).into()),
        Prefix::Disk(_) | Prefix::UNC(_, _) => Some(path.into()),
        _ => None,
    }
}

impl Updater {
    pub fn new(install: &Path, current: &str) -> io::Result<Self> {
        Ok(Self {
            install: std::path::absolute(install)?,
            current: current.parse()?,
            silence: SILENCE,
            feed: FEED.into(),
            key: KEY.into(),
            #[cfg(any(test, feature = "test-feed"))]
            fixture: None,
        })
    }
    /// Test builds can select a loopback repository and its fixture's public key.
    /// The repository URL ends just before /releases/latest.
    #[cfg(any(test, feature = "test-feed"))]
    pub fn for_test_feed(
        install: &Path,
        current: &str,
        repository_url: &str,
        key: &str,
    ) -> io::Result<Self> {
        let uri: ureq::http::Uri = repository_url.parse().map_err(io::Error::other)?;
        let host = uri
            .host()
            .unwrap_or("")
            .trim_start_matches('[')
            .trim_end_matches(']');
        if uri.scheme_str() != Some("http")
            || uri.query().is_some()
            || !host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|a| a.is_loopback())
            || uri.authority().is_none_or(|a| a.as_str().contains('@'))
        {
            return Err(invalid("The fixture must use a loopback HTTP repository"));
        }
        let mut updater = Self::new(install, current)?;
        updater.fixture = Some(format!("http://{}", uri.authority().unwrap()));
        updater.feed = format!("{}/releases/latest", repository_url.trim_end_matches('/'));
        key.clone_into(&mut updater.key);
        Ok(updater)
    }
    pub fn installed(&self) -> bool {
        self.installed_at().is_some()
    }
    fn installed_at(&self) -> Option<(PathBuf, fs::File, Vec<fs::File>)> {
        let parents = unpack::pin_ancestors(&self.install).ok()?;
        let install_dir = unpack::open_dir(&self.install).ok()?;
        let bytes = fs::read(self.install.join("setup/installed.txt")).ok()?;
        let text = crate::text::host_text(&bytes);
        let marked = Path::new(text.trim());
        if !marked.is_absolute() {
            return None;
        }
        // Only probe a marker naming the running executable's directory. Setup always
        // receives the spelling derived from the executable, usable in PowerShell 5.1.
        let to = powershell_directory(&self.install)?;
        let marked = std::path::absolute(marked).ok()?;
        if !crate::text::same_windows_path(&powershell_directory(&marked)?, &to) {
            return None;
        }
        let running = file_id(&install_dir)?;
        let id_at = |path: &Path| file_id(&unpack::open_dir(path).ok()?);
        if id_at(&marked)? != running || (to != self.install && id_at(&to)? != running) {
            return None;
        }
        Some((to, install_dir, parents))
    }
    fn get(&self, url: &str, within: Duration) -> Result<Response<Body>, ureq::Error> {
        #[cfg(any(test, feature = "test-feed"))]
        if let Some(origin) = &self.fixture
            && !url
                .strip_prefix(origin)
                .is_some_and(|rest| rest.starts_with('/'))
        {
            return Err(invalid("Release asset is outside the fixture's loopback origin").into());
        }
        #[cfg(not(any(test, feature = "test-feed")))]
        let secure = true;
        #[cfg(any(test, feature = "test-feed"))]
        let secure = self.fixture.is_none();
        let url = url.to_owned();
        let silence = self.silence;
        // DNS, TLS and all redirects together get one minute to return headers.
        // The response body then has its own (potentially much longer) budget.
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("release-headers".into())
            .spawn(move || {
                let response = string_http::HTTP.get(
                    &url,
                    silence + within,
                    if secure { 10 } else { 0 },
                    |request| {
                        request
                            .header("Accept", "application/vnd.github+json")
                            .header("User-Agent", "String-updater/1.0")
                            .config()
                            .https_only(secure)
                            .timeout_connect(Some(silence))
                            .timeout_resolve(Some(silence))
                            .timeout_recv_response(Some(silence))
                            .timeout_per_read(Some(silence))
                            .timeout_send_request(Some(silence))
                            .timeout_recv_body(Some(within))
                            .build()
                    },
                );
                let _ = tx.send(response);
            })?;
        match rx.recv_timeout(silence) {
            Ok(response) => response,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                Err(ureq::Error::Timeout(ureq::Timeout::RecvResponse))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err(io::Error::other("Release request ended before headers").into())
            }
        }
    }
    fn latest(&self) -> io::Result<Option<Release>> {
        let response = match self.get(&self.feed, METADATA) {
            Err(ureq::Error::StatusCode(404)) => return Ok(None),
            r => r.map_err(io::Error::other)?,
        };
        let json: Feed = serde_json::from_slice(&small(response, 2 << 20)?)?;
        if json.draft || json.prerelease {
            return Ok(None);
        }
        let version = json.tag_name.trim_start_matches(['v', 'V']).parse()?;
        let (mut zip, mut sig, mut size) = (String::new(), String::new(), 0);
        for a in json.assets.unwrap_or_default() {
            match a.name.as_str() {
                "String.zip" => {
                    zip = a.browser_download_url;
                    size = a.size
                }
                "String.zip.sig" => sig = a.browser_download_url,
                _ => {}
            }
        }
        if zip.is_empty() || sig.is_empty() {
            return Err(invalid("The release has no String.zip with its signature"));
        }
        let body = json.body.unwrap_or_default();
        let text = body.trim();
        let notes = if text.encode_utf16().count() > 4000 {
            let mut units: Vec<_> = text.encode_utf16().take(4000).collect();
            if units.last().is_some_and(|v| (0xd800..=0xdbff).contains(v)) {
                units.pop();
            }
            String::from_utf16_lossy(&units) + "…"
        } else {
            text.into()
        };
        Ok(Some(Release {
            info: ReleaseInfo {
                version,
                notes,
                size,
            },
            zip,
            sig,
        }))
    }
    pub fn check(&self) -> io::Result<Check> {
        let installed = self.installed();
        let latest = if installed {
            self.latest()?.map(|r| r.info)
        } else {
            None
        };
        Ok(Check {
            current: self.current,
            installed,
            newer: latest.as_ref().is_some_and(|r| r.version > self.current),
            latest,
        })
    }
    pub fn prepare(
        &self,
        data: &Path,
        want: &str,
        mut progress: impl FnMut(Progress),
    ) -> io::Result<InstallPlan> {
        let (to, install_dir, install_parents) = self
            .installed_at()
            .ok_or_else(|| invalid("Only an installed String updates itself"))?;
        fixed_drive(&to)?;
        let release = self
            .latest()?
            .ok_or_else(|| invalid("There is no release to install"))?;
        let version = release.info.version;
        if version <= self.current {
            return Err(invalid("This String is already up to date"));
        }
        if !want.is_empty() && want != version.to_string() {
            return Err(invalid("The release changed: check again"));
        }
        if release.info.size > MAX_RELEASE {
            return Err(invalid("The release exceeds the size limit"));
        }
        let folder = unpack::UpdateFolder::open(data)?;
        let mut archive = tempfile::tempfile_in(&folder.path)?;
        let res = self.get(&release.zip, RELEASE).map_err(io::Error::other)?;
        let mut last = None;
        transfer(res, MAX_RELEASE, RELEASE, &mut archive, |got, total| {
            let now = Instant::now();
            if last.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(250)) {
                progress(Progress::Receiving {
                    got,
                    total: total.filter(|n| *n > 0).unwrap_or(release.info.size),
                });
                last = Some(now);
            }
        })?;
        progress(Progress::Verifying);
        let sig = small(
            self.get(&release.sig, METADATA).map_err(io::Error::other)?,
            4096,
        )?;
        let signature = Signature::from_slice(&decode(&sig)?).map_err(io::Error::other)?;
        let key = VerifyingKey::from_public_key_der(&decode(self.key.as_bytes())?)
            .map_err(io::Error::other)?;
        archive.rewind()?;
        key.verify_prehash(&hash_copy(&mut archive, io::sink())?, &signature)
            .map_err(|_| invalid("The release signature does not match String's key"))?;
        archive.rewind()?;
        let release = folder.unpack(archive, version)?;
        Ok(InstallPlan {
            release,
            _install_dir: install_dir,
            install_parents,
            to,
            version,
        })
    }
}
fn hash_copy(mut source: impl Read, mut dest: impl Write) -> io::Result<[u8; 32]> {
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let count = source.read(&mut buf)?;
        if count == 0 {
            break;
        }
        hash.update(&buf[..count]);
        dest.write_all(&buf[..count])?;
    }
    Ok(hash.finalize().into())
}

fn decode(data: &[u8]) -> io::Result<Vec<u8>> {
    STANDARD
        .decode(
            data.iter()
                .copied()
                .filter(|b| !b.is_ascii_whitespace())
                .collect::<Vec<_>>(),
        )
        .map_err(io::Error::other)
}
fn transfer(
    mut res: Response<Body>,
    cap: u64,
    within: Duration,
    to: &mut impl Write,
    mut progress: impl FnMut(u64, Option<u64>),
) -> io::Result<()> {
    if !res.status().is_success() {
        return Err(invalid(format!(
            "Release request returned {}",
            res.status()
        )));
    }
    let total = res
        .headers()
        .get("content-length")
        .and_then(|h| h.to_str().ok()?.parse().ok());
    if total.is_some_and(|t| t > cap) {
        return Err(invalid("Release response exceeds the size limit"));
    }
    let mut got = 0;
    let mut buf = [0; 65536];
    let mut reader = res.body_mut().as_reader();
    let started = Instant::now();
    loop {
        if started.elapsed() >= within {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "The release transfer took too long",
            ));
        }
        let n = reader.read(&mut buf).map_err(|err| {
            match err.get_ref().and_then(|e| e.downcast_ref::<ureq::Error>()) {
                Some(ureq::Error::Timeout(ureq::Timeout::PerRead)) => {
                    io::Error::new(io::ErrorKind::TimedOut, "The release stopped arriving")
                }
                Some(ureq::Error::Timeout(_)) => io::Error::new(
                    io::ErrorKind::TimedOut,
                    "The release transfer took too long",
                ),
                _ => err,
            }
        })?;
        if n == 0 {
            break;
        }
        got += n as u64;
        if got > cap {
            return Err(invalid("Release response exceeds the size limit"));
        }
        to.write_all(&buf[..n])?;
        progress(got, total);
    }
    Ok(())
}
fn small(res: Response<Body>, cap: u64) -> io::Result<Vec<u8>> {
    let mut out = vec![];
    transfer(res, cap, METADATA, &mut out, |_, _| {})?;
    Ok(out)
}
