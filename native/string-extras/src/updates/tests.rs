use super::*;
use std::{
    fs::File,
    net::{TcpListener, TcpStream},
    os::windows::io::{AsRawHandle, OwnedHandle, RawHandle},
    process::{Child, Command, Stdio},
    thread,
};
use tempfile::tempdir;
use zip::{ZipWriter, write::SimpleFileOptions};

// The fixture's entry point directs the real release installer to test-only shell locations.
const FIXTURE_SETUP: &str = r#"param([Parameter(Mandatory)][string]$To)
if ($To -cne $env:STRING_TEST_TARGET) { throw 'Unexpected install target' }
& (Join-Path $PSScriptRoot 'isolated.ps1') -To $To -Menu "$To menu" -Desktop "$To desktop" -AppsKey 'HKCU:\Software\StringUpdateFixture' -NoStart
exit $LASTEXITCODE
"#;

fn manifest_row(name: &str, contents: &str) -> String {
    let hash: String = Sha256::digest(contents.as_bytes())
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    format!(
        "{hash}\t{}\t{}\r\n",
        contents.len(),
        name.replace('/', "\\")
    )
}

fn archive(file: File, version: &str, setup: &str, symlink: bool) -> File {
    let mut z = ZipWriter::new(file);
    let opts = SimpleFileOptions::default();
    let script_name = if setup.starts_with("# Mixed-case entry") {
        "setup/Install.PS1"
    } else {
        "setup/install.ps1"
    };
    let files = [
        ("setup/version.txt", version),
        ("setup/imports.txt", "kernel32.dll GetLastError"),
        ("String.exe", "fixture"),
        (script_name, setup),
        (
            "setup/isolated.ps1",
            include_str!("../../../../tools/share/install.ps1"),
        ),
        (
            "setup/uninstall.ps1",
            include_str!("../../../../tools/share/uninstall.ps1"),
        ),
    ];
    let mut manifest = String::new();
    for (name, bytes) in files {
        z.start_file(format!("app/{name}"), opts).unwrap();
        z.write_all(bytes.as_bytes()).unwrap();
        manifest.push_str(&manifest_row(name, bytes));
    }
    manifest.push_str(&format!("#end\t{}\r\n", files.len()));
    z.start_file("app/setup/files.txt", opts).unwrap();
    z.write_all(manifest.as_bytes()).unwrap();
    if symlink {
        z.add_symlink("app/alias", "elsewhere", opts).unwrap();
    }
    let mut f = z.finish().unwrap();
    f.rewind().unwrap();
    f
}
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn config(install: &Path, origin: &str, repo: &str, key: &str) -> Updater {
    Updater::for_test_feed(install, "2.0.0", &format!("{origin}/repos/{repo}"), key).unwrap()
}
#[test]
fn windows_installer_marker_matches_the_verbatim_exe_directory() {
    let tmp = tempdir().unwrap();
    let install = tmp.path().join("Player");
    fs::create_dir_all(install.join("setup")).unwrap();
    // install.ps1 writes an absolute Win32 path in UTF-8 without a BOM.
    fs::write(
        install.join("setup/installed.txt"),
        install.to_str().unwrap(),
    )
    .unwrap();
    let exe_dir = fs::canonicalize(&install).unwrap();
    let updater = Updater::new(&exe_dir, "2.0.0").unwrap();
    assert_eq!(
        updater.installed_at().map(|(path, ..)| path).as_deref(),
        Some(install.as_path())
    );
    fs::write(
        install.join("setup/installed.txt"),
        tmp.path().to_str().unwrap(),
    )
    .unwrap();
    assert!(!updater.installed());
}
#[test]
fn marker_spelling_does_not_select_the_installer_destination() {
    let tmp = tempdir().unwrap();
    let install = tmp.path().join("Player");
    fs::create_dir_all(install.join("setup")).unwrap();
    let marker = install.join("setup/installed.txt");
    // Exercise the volume/file ID query even when the host folds case.
    assert!(
        file_id(&unpack::open_dir(&install).unwrap()).unwrap()
            != file_id(&unpack::open_dir(tmp.path()).unwrap()).unwrap()
    );
    let other_spelling = tmp.path().join("player");
    fs::write(&marker, other_spelling.to_str().unwrap()).unwrap();
    let updater = Updater::new(&fs::canonicalize(&install).unwrap(), "2.0").unwrap();
    // On a case-sensitive volume the alternate spelling is a different directory.
    if !other_spelling.exists() {
        fs::create_dir(&other_spelling).unwrap();
        assert!(!updater.installed());
    } else {
        assert_eq!(
            updater.installed_at().map(|(path, ..)| path).as_deref(),
            Some(install.as_path())
        );
    }
}
#[test]
fn signed_release_feed_end_to_end() {
    with_prepared_release(FIXTURE_SETUP, |plan, root, install, origin, key| {
        let child = plan
            .spawn_command(
                &[("STRING_TEST_TARGET", install), ("LOCALAPPDATA", root)],
                0x08000000,
            )
            .unwrap();
        drop(plan);
        wait_for_setup(&child);
        let version = fs::read_to_string(install.join("setup/version.txt")).unwrap();
        assert_eq!(version, "9.9.9");
        assert_eq!(fs::read(install.join("String.exe")).unwrap(), b"fixture");
        let updated =
            Updater::for_test_feed(install, &version, &format!("{origin}/repos/good"), key)
                .unwrap();
        assert!(!updated.check().unwrap().newer);
    });
}

#[test]
fn launch_uses_the_signed_entry_spelling() {
    with_prepared_release("# Mixed-case entry\nparam($To)", |plan, _, _, _, _| {
        let script = &plan.release.script;
        assert_eq!(script.file_name().unwrap().to_str(), Some("Install.PS1"));
        assert!(
            script.to_string_lossy().starts_with(r"\\?\Volume{"),
            "PowerShell still locates the signed script by a drive letter"
        );
    });
}

fn wait_for_setup(child: &OwnedHandle) {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn WaitForSingleObject(handle: RawHandle, milliseconds: u32) -> u32;
        fn GetExitCodeProcess(handle: RawHandle, code: *mut u32) -> i32;
    }
    let mut code = 0;
    assert_eq!(
        unsafe { WaitForSingleObject(child.as_raw_handle(), 90000) },
        0
    );
    assert_ne!(
        unsafe { GetExitCodeProcess(child.as_raw_handle(), &mut code) },
        0
    );
    assert_eq!(code, 0);
}

fn delete_handle(dir: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .access_mode(0x0001_0000) // DELETE
        .share_mode(7)
        .custom_flags(0x0200_0000) // FILE_FLAG_BACKUP_SEMANTICS
        .open(dir)
}

fn assert_delete_denied(dir: &Path) {
    let error = delete_handle(dir).expect_err("guarded folder allowed a delete handle");
    assert_eq!(error.raw_os_error(), Some(32), "{}", dir.display());
}

#[test]
fn prepared_release_holds_checked_folders_until_launch() {
    // This signed fixture entry point only checks its working directory and the target marker.
    let probe = r#"param([string]$To)
$release = [IO.Path]::GetDirectoryName([IO.Path]::GetDirectoryName($PSScriptRoot))
if ($PWD.ProviderPath -eq $release -and [Environment]::CurrentDirectory -eq $release -and
    (Test-Path -LiteralPath (Join-Path $To 'setup/installed.txt'))) {
    [IO.File]::WriteAllText($env:STRING_TEST_RESULT, 'prepared')
}
"#;
    with_prepared_release(probe, |plan, root, install, _, _| {
        let dirs = [
            install.to_owned(),
            plan.release.path.clone(),
            plan.release.path.join("app"),
            plan.release.path.join("app/setup"),
        ];
        for dir in &dirs {
            assert_delete_denied(dir);
        }
        let result = root.join("result");
        let child = plan
            .spawn_command(&[("STRING_TEST_RESULT", &result)], 0x08000000)
            .unwrap();
        drop(plan);
        wait_for_setup(&child);
        assert_eq!(fs::read(result).unwrap(), b"prepared");
        for dir in &dirs {
            delete_handle(dir).unwrap();
        }
    });
}

#[test]
fn prepared_payload_is_read_only_through_the_child_lifetime() {
    with_prepared_release(
        "param($To)\nwhile (!(Test-Path -LiteralPath $env:STRING_TEST_WAIT)) { Start-Sleep -Milliseconds 20 }",
        |plan, root, _, _, _| {
            let script = plan.release.path.join("app/setup/install.ps1");
            let exe = plan.release.path.join("app/String.exe");
            let write_error = |p: &Path| {
                File::options()
                    .write(true)
                    .open(p)
                    .err()
                    .and_then(|e| e.raw_os_error())
            };
            let before = [&script, &exe].map(|p| write_error(p));
            let done = root.join("continue");
            let child = plan
                .spawn_command(&[("STRING_TEST_WAIT", &done)], 0x08000000)
                .unwrap();
            drop(plan);
            // Opening for write is enough to check sharing. Leave the files untouched.
            let after = [&script, &exe].map(|p| write_error(p));
            let parent = delete_handle(root).err().and_then(|e| e.raw_os_error());
            fs::write(done, "").unwrap();
            wait_for_setup(&child);
            assert_eq!(
                (before, after, parent),
                ([Some(32); 2], [Some(32); 2], Some(32))
            );
        },
    );
}

#[test]
fn preparation_pins_profile_and_install_ancestors() {
    let t = tempdir().unwrap();
    let profile = t.path().join("profile");
    let folder = unpack::UpdateFolder::open(&profile).unwrap();
    let profile_parent = delete_handle(t.path()).err().and_then(|e| e.raw_os_error());
    drop(folder);
    let install = t.path().join("Player");
    fs::create_dir_all(install.join("setup")).unwrap();
    fs::write(
        install.join("setup/installed.txt"),
        install.to_str().unwrap(),
    )
    .unwrap();
    let updater = Updater::new(&install, "2.0").unwrap();
    let _installed = updater.installed_at().unwrap();
    let install_parent = delete_handle(t.path()).err().and_then(|e| e.raw_os_error());
    assert_eq!((profile_parent, install_parent), (Some(32), Some(32)));
}

#[test]
fn update_folder_guard_denies_delete_access() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("updates");
    let folder = unpack::UpdateFolder::open(tmp.path()).unwrap();
    assert_delete_denied(&dir);
    drop(folder);
    delete_handle(&dir).unwrap();
}

fn with_prepared_release(
    setup: &str,
    check_plan: impl FnOnce(InstallPlan, &Path, &Path, &str, &str),
) {
    let runs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-runs");
    fs::create_dir_all(&runs).unwrap();
    let tmp = tempfile::tempdir_in(std::path::absolute(runs).unwrap()).unwrap();
    let root = tmp.path();
    for (name, ver) in [("good.zip", "9.9.9"), ("liar.zip", "9.9.8")] {
        archive(File::create(root.join(name)).unwrap(), ver, setup, false);
    }
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fake-github.js");
    let mut server = Server(
        Command::new("node")
            .arg(script)
            .arg(root)
            .arg(address.port().to_string())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(address).is_err() {
        assert!(
            server.0.try_wait().unwrap().is_none() && Instant::now() < deadline,
            "fixture failed to start"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let key = fs::read_to_string(root.join("update-key.txt")).unwrap();
    let origin = format!("http://{address}");
    let install = root.join("String & % '");
    let data = root.join("profile");
    fs::create_dir_all(install.join("setup")).unwrap();
    let good = config(&install, &origin, "good", &key);
    assert!(!good.check().unwrap().installed);
    let marker = format!("\u{feff}{}", install.display())
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    fs::write(install.join("setup/installed.txt"), marker).unwrap();
    fs::write(install.join("setup/version.txt"), "2.0.0").unwrap();
    fs::write(
        install.join("setup/files.txt"),
        manifest_row("setup/version.txt", "2.0.0") + "#end\t1\r\n",
    )
    .unwrap();
    let check = good.check().unwrap();
    assert!(check.installed && check.newer);
    let latest = check.latest.unwrap();
    assert_eq!(latest.notes, "Fake notes for v9.9.9");
    assert_eq!(
        latest.size,
        fs::metadata(root.join("good.zip")).unwrap().len()
    );
    assert!(good.prepare(&data, "9.9.8", |_| {}).is_err());
    let mut events = vec![];
    let plan = good
        .prepare(&data, "9.9.9", |p| {
            assert_delete_denied(&install);
            events.push(p);
        })
        .unwrap();
    assert!(
        matches!(events.first(),Some(Progress::Receiving{got,total}) if got> &0 && *total==latest.size)
    );
    assert!(matches!(events.last(), Some(Progress::Verifying)));
    let expected = unpack::open_dir(&data.join(r"updates\String-9.9.9")).unwrap();
    assert!(
        file_id(&unpack::open_dir(&plan.release.path).unwrap()).unwrap()
            == file_id(&expected).unwrap()
    );
    drop(expected);
    let prepared_script = plan.release.script.clone();
    assert_eq!(fs::read_to_string(&prepared_script).unwrap(), setup);
    assert_eq!(plan.to, install);
    // Another preparation must wait while the pending plan owns its release.
    assert_eq!(
        unpack::UpdateFolder::open(&data)
            .unwrap_err()
            .raw_os_error(),
        Some(32)
    );
    check_plan(plan, root, &install, &origin, &key);
    for (repo, message) in [
        ("tampered", "signature"),
        ("liar", "version inside"),
        ("old", "up to date"),
        ("none", "no release"),
    ] {
        let u = config(&install, &origin, repo, &key);
        let check = u.check().unwrap();
        assert_eq!(check.newer, matches!(repo, "liar" | "tampered"));
        assert_eq!(check.latest.is_none(), repo == "none");
        let error = u.prepare(&data, "", |_| {}).unwrap_err().to_string();
        assert!(error.contains(message), "{repo}: {error}");
        assert!(prepared_script.exists());
    }
    // Preparing the same good release twice also cleans up an older preparation.
    let old = data.join("updates/previous");
    fs::create_dir(&old).unwrap();
    let retry = good.prepare(&data, "", |_| {}).unwrap();
    assert_delete_denied(&retry.release.path);
    assert_eq!(fs::read_to_string(&retry.release.script).unwrap(), setup);
    assert!(!prepared_script.exists());
    assert!(!old.exists());
}
#[test]
fn archive_paths_and_links_are_refused() {
    for path in [
        "../x",
        "a/../b",
        "d:/x",
        "\\server\\share",
        "/x",
        "a:x",
        "con.txt",
        "a/aux .bin",
        "a//b",
        "x.",
    ] {
        assert!(unpack::entry_path(path).is_err(), "{path}");
    }
    assert!(unpack::entry_path("app/setup/install.ps1").is_ok());
    let tmp = tempdir().unwrap();
    let folder = unpack::UpdateFolder::open(tmp.path()).unwrap();
    let f = archive(tempfile::tempfile().unwrap(), "9.9.9", FIXTURE_SETUP, true);
    assert!(
        folder
            .unpack(f, "9.9.9".parse().unwrap())
            .unwrap_err()
            .to_string()
            .contains("link")
    );
}
#[test]
fn updates_directory_junction_is_refused() {
    let t = tempdir().unwrap();
    let dest = t.path().join("outside");
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("sentinel"), "kept").unwrap();
    let output=Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command",
        "New-Item -ItemType Junction -Path $env:LINK_PATH -Target $env:LINK_TARGET -ErrorAction Stop | Out-Null"])
        .env("LINK_PATH",t.path().join("updates")).env("LINK_TARGET",&dest).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(unpack::UpdateFolder::open(t.path()).is_err());
    assert_eq!(fs::read_dir(dest).unwrap().count(), 1);
}
fn response(body: &'static [u8], size: Option<usize>) -> Response<Body> {
    let mut r = Response::builder();
    if let Some(size) = size {
        r = r.header("content-length", size);
    }
    r.body(Body::builder().data(body)).unwrap()
}
#[test]
fn versions_limits_and_transport() {
    assert_eq!(
        "2.3".parse::<Version>().unwrap(),
        "2.3.0.42".parse().unwrap()
    );
    for bad in ["v2.3", "2", "2.3.4.5.6", "2.-1", "2.2147483648"] {
        assert!(bad.parse::<Version>().is_err());
    }
    assert!(small(response(b"12345", None), 4).is_err());
    assert!(small(response(b"x", Some(10)), 4).is_err());
    assert_eq!(small(response(b"1234", None), 4).unwrap(), b"1234");
    let t = tempdir().unwrap();
    let u = Updater::new(t.path(), "1.0").unwrap();
    assert!(matches!(
        u.get("http://127.0.0.1:1/no", METADATA),
        Err(ureq::Error::RequireHttpsOnly(_))
    ));
    assert!(VerifyingKey::from_public_key_der(&decode(KEY.as_bytes()).unwrap()).is_ok());
}

fn serve_once(send: impl FnOnce(TcpStream) + Send + 'static) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let task = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = vec![];
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        send(stream);
    });
    (format!("http://{address}"), task)
}

#[test]
fn drafts_and_prereleases_are_skipped_before_version_and_asset_checks() {
    let tmp = tempdir().unwrap();
    for flag in ["draft", "prerelease"] {
        let (origin, task) = serve_once(move |mut s| {
            let body = serde_json::json!({flag:true}).to_string();
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let u = config(tmp.path(), &origin, "recent", KEY);
        assert!(u.latest().unwrap().is_none());
        task.join().unwrap();
    }
}

#[test]
fn body_deadlines_cover_a_silent_server_and_one_that_never_finishes() {
    let t = tempdir().unwrap();
    for trickle in [false, true] {
        let (done, wait) = std::sync::mpsc::channel::<()>();
        let (origin, task) = serve_once(move |mut s| {
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20000\r\n\r\na")
                .unwrap();
            if trickle {
                for _ in 0..200 {
                    if wait.recv_timeout(Duration::from_millis(10)).is_ok()
                        || s.write_all(b"a").is_err()
                    {
                        break;
                    }
                }
            } else {
                let _ = wait.recv_timeout(Duration::from_secs(3));
            }
        });
        let mut u = config(t.path(), &origin, "recent", KEY);
        let (silence, overall) = if trickle { (1000, 180) } else { (150, 1500) };
        u.silence = Duration::from_millis(silence);
        let res = u
            .get(&format!("{origin}/asset"), Duration::from_millis(overall))
            .unwrap();
        let err = small(res, 20000).unwrap_err();
        assert!(
            matches!(
                err.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ),
            "{err:?}"
        );
        let _ = done.send(());
        task.join().unwrap();
    }
}
