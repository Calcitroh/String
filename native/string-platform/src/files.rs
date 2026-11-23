//! Files and Explorer: folder and file pickers (the shell's own dialog, IFileOpenDialog), showing a file in Explorer
//! (SHOpenFolderAndSelectItems), opening a web link, moving a song to the Recycle Bin (undoable, like Explorer's Delete),
//! and the backup folder (`Documents\String Backup`).
//!
//! The pickers belong to the window's thread: they run their own message loop until the person chooses.

use crate::{Hwnd, com};
use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::{fs, io};
use windows::Win32::Foundation::ERROR_CANCELLED;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING,
    FOLDERID_Documents, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM,
    FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, ILCreateFromPathW, ILFree,
    IShellItem, KF_FLAG_DEFAULT, SHCreateItemFromParsingName, SHFILEOPSTRUCTW, SHFileOperationW,
    SHGetKnownFolderPath, SHOpenFolderAndSelectItems, SIGDN_FILESYSPATH, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

/// What a picker asks.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pick<'a> {
    /// The dialog's title; "" for "Choose a folder" / "Choose a file".
    pub title: &'a str,
    /// The folder it opens in, when that folder exists.
    pub start: Option<&'a Path>,
    /// More than one may be chosen.
    pub multi: bool,
}

/// The folder picker. `None` when the person cancelled.
pub fn pick_folders(owner: Option<Hwnd>, pick: &Pick) -> io::Result<Option<Vec<PathBuf>>> {
    choose(owner, pick, None)
}

/// The file picker. `filter` is WPF's (the page's): `"Songs|*.mp3;*.flac|All files|*.*"`, "" for all files. `None` when
/// the person cancelled.
pub fn pick_files(
    owner: Option<Hwnd>,
    pick: &Pick,
    filter: &str,
) -> io::Result<Option<Vec<PathBuf>>> {
    choose(owner, pick, Some(filter))
}

/// `"Name|*.a;*.b|Name|*.c"` as (name, patterns) pairs; all files when there are none.
pub fn parse_filter(filter: &str) -> Vec<(String, String)> {
    let parts: Vec<&str> = filter.split('|').collect();
    let pairs: Vec<(String, String)> = parts
        .as_chunks::<2>()
        .0
        .iter()
        .filter(|[_, spec]| !spec.trim().is_empty())
        .map(|[name, spec]| (name.trim().to_string(), spec.trim().to_string()))
        .collect();
    if pairs.is_empty() {
        vec![("All files".into(), "*.*".into())]
    } else {
        pairs
    }
}

fn choose(
    owner: Option<Hwnd>,
    pick: &Pick,
    files: Option<&str>,
) -> io::Result<Option<Vec<PathBuf>>> {
    com::init();
    let dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }?;
    let filters: Vec<(HSTRING, HSTRING)> = files
        .map(parse_filter)
        .unwrap_or_default()
        .into_iter()
        .map(|(n, s)| (HSTRING::from(n), HSTRING::from(s)))
        .collect();
    unsafe {
        let mut options = dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST;
        options |= if files.is_some() {
            FOS_FILEMUSTEXIST
        } else {
            FOS_PICKFOLDERS
        };
        if pick.multi {
            options |= FOS_ALLOWMULTISELECT;
        }
        dialog.SetOptions(options)?;
        let fallback = if files.is_some() {
            "Choose a file"
        } else {
            "Choose a folder"
        };
        dialog.SetTitle(&HSTRING::from(if pick.title.is_empty() {
            fallback
        } else {
            pick.title
        }))?;
        if let Some(start) = pick.start.filter(|p| p.is_dir()) {
            let item: windows::core::Result<IShellItem> =
                SHCreateItemFromParsingName(&wide(start.as_os_str()), None);
            if let Ok(item) = item {
                dialog.SetFolder(&item)?;
            }
        }
        if !filters.is_empty() {
            let specs: Vec<COMDLG_FILTERSPEC> = filters
                .iter()
                .map(|(n, s)| COMDLG_FILTERSPEC {
                    pszName: PCWSTR(n.as_ptr()),
                    pszSpec: PCWSTR(s.as_ptr()),
                })
                .collect();
            dialog.SetFileTypes(&specs)?;
        }
        match dialog.Show(owner.map(Hwnd::get)) {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let results = dialog.GetResults()?;
        let mut paths = Vec::new();
        for i in 0..results.GetCount()? {
            let name = results.GetItemAt(i)?.GetDisplayName(SIGDN_FILESYSPATH)?;
            paths.push(PathBuf::from(OsString::from_wide(name.as_wide())));
            CoTaskMemFree(Some(name.0 as *const _));
        }
        Ok(Some(paths))
    }
}

/// Explorer at the file, selected; a folder opens itself.
pub fn reveal(path: &Path) -> io::Result<()> {
    let meta = fs::metadata(path)
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "It isn't there any more"))?;
    com::init();
    if meta.is_dir() {
        return shell_open(path.as_os_str());
    }
    let full = std::path::absolute(path)?;
    let pidl = unsafe { ILCreateFromPathW(&wide(full.as_os_str())) };
    if pidl.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Explorer couldn't find it",
        ));
    }
    let shown = unsafe { SHOpenFolderAndSelectItems(pidl, None, 0) };
    unsafe { ILFree(Some(pidl)) };
    Ok(shown?)
}

/// An `http` or `https` address with a host, no spaces or control characters: the only links String opens.
pub fn is_web_link(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    else {
        return false;
    };
    !rest.is_empty()
        && !rest.starts_with(['/', '\\', '?', '#', '@', ':'])
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// Opens a web link in the person's browser; anything else is refused (never runs a program).
pub fn open_web_link(url: &str) -> io::Result<()> {
    if !is_web_link(url) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Only web links (http, https) open",
        ));
    }
    shell_open(OsStr::new(url))
}

fn shell_open(target: &OsStr) -> io::Result<()> {
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &wide(target),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = r.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Windows couldn't open it (error {code})"
        )))
    }
}

/// Moves one file to the Recycle Bin, the way Explorer's Delete does: it can be put back. Where a drive has no Recycle
/// Bin, Windows asks before deleting for good. A full path to a file is needed. (App.cs allowed only song files, and only
/// after the person's yes on the Duplicates page: those checks stay with the caller.)
pub fn recycle(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Not a full path",
        ));
    }
    let path = std::path::absolute(path)?;
    if !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "The file isn't there any more",
        ));
    }
    // a list of paths, each ending in a nul, the list in another
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let flags =
        FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI | FOF_WANTNUKEWARNING;
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: flags.0 as u16,
        ..Default::default()
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    if op.fAnyOperationsAborted.as_bool() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Left where it was",
        ));
    }
    if r != 0 {
        return Err(io::Error::other(format!(
            "Windows couldn't move it to the Recycle Bin (error {r})"
        )));
    }
    if path.exists() {
        return Err(io::Error::other("Windows didn't move it"));
    }
    Ok(())
}

/// The person's Documents folder (wherever they moved it).
pub fn documents_folder() -> io::Result<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None)?;
        let path = PathBuf::from(OsString::from_wide(p.as_wide()));
        CoTaskMemFree(Some(p.0 as *const _));
        Ok(path)
    }
}

/// `Documents\String Backup`: one dated copy a day of state and listening (string-core writes them). A self-test uses a
/// folder of its own instead (App.cs: `<data>\backup`).
pub fn backup_folder() -> io::Result<PathBuf> {
    Ok(documents_folder()?.join("String Backup"))
}

fn wide(s: &OsStr) -> HSTRING {
    HSTRING::from_wide(&s.encode_wide().collect::<Vec<u16>>())
}

#[cfg(test)]
mod tests {
    // Nothing here opens a dialog, Explorer, a browser, or touches the Recycle Bin: only the refusals are tried.
    use super::*;

    #[test]
    fn filters_as_wpf_wrote_them() {
        assert_eq!(
            parse_filter("Songs|*.mp3;*.flac|All files|*.*"),
            [
                ("Songs".to_string(), "*.mp3;*.flac".to_string()),
                ("All files".into(), "*.*".into())
            ]
        );
        assert_eq!(
            parse_filter(""),
            [("All files".to_string(), "*.*".to_string())]
        );
        assert_eq!(
            parse_filter("Broken"),
            [("All files".to_string(), "*.*".to_string())]
        );
        assert_eq!(
            parse_filter("Pictures|*.png|Odd"),
            [("Pictures".to_string(), "*.png".to_string())]
        );
    }

    #[test]
    fn only_web_links_open() {
        for ok in [
            "https://lrclib.net",
            "http://example.test/a?b=c",
            "HTTPS://Example.test",
        ] {
            assert!(is_web_link(ok), "{ok}");
        }
        for bad in [
            "file:///C:/Windows/notepad.exe",
            r"C:\Windows\notepad.exe",
            "calc.exe",
            "https://",
            "https:///path",
            "https://exa mple.test",
            "https://example.test/\n",
            "javascript:alert(1)",
            "",
        ] {
            assert!(!is_web_link(bad), "{bad:?}");
            assert_eq!(
                open_web_link(bad).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn reveal_says_when_its_gone() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            reveal(&dir.path().join("gone.mp3")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn recycle_refuses_what_it_shouldnt_touch() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            recycle(Path::new(r"songs\a.mp3")).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            recycle(&dir.path().join("gone.mp3")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            recycle(dir.path()).unwrap_err().kind(),
            io::ErrorKind::NotFound,
            "a folder is not a file"
        );
        assert!(dir.path().exists());
    }

    #[test]
    fn backup_folder_is_in_documents() {
        let docs = documents_folder().unwrap();
        assert!(docs.is_absolute() && docs.is_dir());
        assert_eq!(backup_folder().unwrap(), docs.join("String Backup"));
    }
}
