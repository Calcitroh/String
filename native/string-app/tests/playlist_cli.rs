use std::{ffi::OsStr, fs, process::Command};

#[test]
fn refused_playlist_calls_exit_two_before_touching_the_profile() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("profile");
    let out = dir.path().join("out");
    let song = dir.path().join("song.wav");
    fs::write(&song, b"").unwrap();
    let add = OsStr::new("--add");
    let playlist = OsStr::new("--playlist");
    for tail in [
        vec![add, playlist],
        vec![add, playlist, OsStr::new("Mix")],
        vec![add, song.as_os_str(), playlist, OsStr::new("  ")],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_String"))
            .args(["--smoke-test", "--data"])
            .arg(&data)
            .arg("--out")
            .arg(&out)
            .args(tail)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(!data.exists() && !out.exists());
    }
}
