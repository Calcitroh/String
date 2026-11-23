//! Check a published release and prepare its signed installer for the app.
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
    process::Command,
    str::FromStr,
    time::{Duration, Instant},
};
use ureq::{Agent, Body, http::Response};

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
    pub ready: PathBuf,
    pub script: PathBuf,
    pub to: PathBuf,
}
impl InstallPlan {
    /// A prepared command. The app chooses when to open its installer console.
    pub fn command(&self) -> Command {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new("powershell.exe");
        const RUN: &str = concat!(
            "$Host.UI.RawUI.WindowTitle = 'Updating String'; ",
            "& $env:STRING_UPDATE_SCRIPT -To $env:STRING_UPDATE_TO; ",
            "if ($LASTEXITCODE) { Read-Host 'Press Enter to close' }"
        );
        cmd.current_dir(&self.ready)
            .env_remove("PSModulePath")
            .env("STRING_UPDATE_SCRIPT", &self.script)
            .env("STRING_UPDATE_TO", &self.to)
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", RUN])
            .creation_flags(0x10); // CREATE_NEW_CONSOLE
        cmd
    }
}
pub struct Updater {
    install: PathBuf,
    current: Version,
    agent: Agent,
    feed: String,
    key: String,
    #[cfg(any(test, feature = "test-feed"))]
    fixture: Option<String>,
}
impl Updater {
    pub fn new(install: &Path, current: &str) -> io::Result<Self> {
        Ok(Self {
            install: std::path::absolute(install)?,
            current: current.parse()?,
            agent: agent(true, SILENCE),
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
        updater.agent = agent(false, SILENCE);
        key.clone_into(&mut updater.key);
        Ok(updater)
    }
    pub fn installed(&self) -> bool {
        fs::read(self.install.join("setup/installed.txt"))
            .ok()
            .and_then(|bytes| std::path::absolute(crate::text::host_text(&bytes).trim()).ok())
            .is_some_and(|p| crate::text::same_windows_path(&p, &self.install))
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
        let request = self
            .agent
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "String-updater/1.0")
            .config()
            .timeout_global(Some(SILENCE + within))
            .timeout_recv_body(Some(within))
            .build();
        // DNS, TLS and all redirects together get one minute to return headers.
        // The response body then has its own (potentially much longer) budget.
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("release-headers".into())
            .spawn(move || {
                let _ = tx.send(request.call());
            })?;
        match rx.recv_timeout(SILENCE) {
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
        if !self.installed() {
            return Err(invalid("Only an installed String updates itself"));
        }
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
        let mut hash = Sha256::new();
        archive.rewind()?;
        let mut buf = [0; 65536];
        loop {
            let n = archive.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        key.verify_prehash(&hash.finalize(), &signature)
            .map_err(|_| invalid("The release signature does not match String's key"))?;
        archive.rewind()?;
        let ready = folder.unpack(archive, version)?;
        Ok(InstallPlan {
            script: ready.join("app/setup/install.ps1"),
            ready,
            to: self.install.clone(),
            version,
        })
    }
}
fn agent(secure: bool, silence: Duration) -> Agent {
    let mut cfg = Agent::config_builder()
        .https_only(secure)
        .timeout_connect(Some(silence))
        .timeout_resolve(Some(silence))
        .timeout_recv_response(Some(silence))
        .timeout_per_read(Some(silence))
        .timeout_send_request(Some(silence));
    if !secure {
        cfg = cfg.proxy(None).max_redirects(0);
    }
    cfg.build().into()
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
