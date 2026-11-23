use crate::view::View;
use std::{
    io,
    path::{Path, PathBuf},
};
use string_platform::{
    Hwnd,
    files::{self, Pick},
};

const SONG_FILTER: &str =
    "Songs|*.mp3;*.flac;*.m4a;*.m4b;*.aac;*.ogg;*.oga;*.opus;*.wav;*.weba;*.webm|All files|*.*";

#[derive(Clone, Copy)]
pub enum LibraryPicker {
    Folders,
    Songs,
}

#[derive(Clone, Copy)]
pub struct Files {
    hwnd: Option<Hwnd>,
    smoke: bool,
}

impl Files {
    pub(crate) fn new(hwnd: Option<Hwnd>, smoke: bool) -> Self {
        Self { hwnd, smoke }
    }

    pub(crate) fn pick(self, picker: LibraryPicker) -> io::Result<Vec<PathBuf>> {
        let (title, filter) = match picker {
            LibraryPicker::Folders => ("Choose folders with your songs", None),
            LibraryPicker::Songs => ("Choose songs", Some(SONG_FILTER)),
        };
        let mut paths = self.choose(
            &Pick {
                title,
                start: None,
                multi: true,
            },
            filter,
        )?;
        if matches!(picker, LibraryPicker::Songs) {
            paths.retain(|p| song_path(p));
        }
        Ok(paths)
    }
    pub(crate) fn choose(self, options: &Pick, filter: Option<&str>) -> io::Result<Vec<PathBuf>> {
        if self.smoke {
            return Ok(Vec::new());
        }
        match filter {
            Some(filter) => files::pick_files(self.hwnd, options, filter),
            None => files::pick_folders(self.hwnd, options),
        }
        .map(Option::unwrap_or_default)
    }

    pub fn reveal(self, path: &Path) -> io::Result<()> {
        if self.smoke {
            return Ok(());
        }
        files::reveal(path)
    }

    pub fn open_web_link(self, url: &str) -> io::Result<()> {
        if !files::is_web_link(url) {
            return Err(io::Error::other("A web address is required"));
        }
        if self.smoke {
            return Ok(());
        }
        files::open_web_link(url)
    }

    /// Call on a worker for each confirmed duplicate. Merge only successful moves.
    pub fn recycle(self, path: &Path) -> io::Result<()> {
        if self.smoke {
            return Err(io::Error::other(
                "The Recycle Bin is unavailable in smoke mode",
            ));
        }
        if !path.is_absolute() || !recyclable_song(path) {
            return Err(io::Error::other("A full path to a song file is required"));
        }
        files::recycle(path)
    }
}

pub(crate) fn recyclable_song(path: &Path) -> bool {
    // The library can play WebM video containers. Only audio types may go to the bin.
    song_path(path)
        && !path
            .extension()
            .unwrap_or_default()
            .eq_ignore_ascii_case("webm")
}

fn song_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        [
            "mp3", "flac", "m4a", "m4b", "aac", "ogg", "oga", "opus", "wav", "weba", "webm",
        ]
        .iter()
        .any(|ext| e.eq_ignore_ascii_case(ext))
    })
}

impl View<'_> {
    pub fn pick_folders(&mut self) {
        self.platform.picker = Some(LibraryPicker::Folders);
    }
    pub fn pick_songs(&mut self) {
        self.platform.picker = Some(LibraryPicker::Songs);
    }
    pub fn open_web_link(&mut self, url: &str) {
        if let Err(e) = self.platform.files().open_web_link(url) {
            self.views.toast.set(e.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn smoke_never_opens_a_picker_shell_or_bin() {
        let f = Files::new(None, true);
        assert!(f.pick(LibraryPicker::Folders).unwrap().is_empty());
        assert!(f.pick(LibraryPicker::Songs).unwrap().is_empty());
        assert!(f.open_web_link("https://example.com").is_ok());
        assert!(f.open_web_link("file:///example.exe").is_err());
        assert!(f.recycle(Path::new(r"C:\fixture\song.flac")).is_err());
    }

    #[test]
    fn only_song_extensions_are_accepted() {
        assert!(song_path(Path::new("song.M4B")));
        assert!(!song_path(Path::new("song.mp3.exe")));
    }
}
