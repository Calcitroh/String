use super::{files::Files, updates::Updates};
use crate::services::{self, Discord, Services, UpdateInfo, UpdateProgress};
use egui::Context;
use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
};
use string_platform::{Hwnd, files::Pick};

pub struct Dialog {
    title: String,
    filter: Option<String>,
    start: Option<PathBuf>,
    multi: bool,
    reply: mpsc::Sender<io::Result<Vec<PathBuf>>>,
}
impl Dialog {
    pub fn open(self, files: Files) {
        let options = Pick {
            title: &self.title,
            start: self.start.as_deref(),
            multi: self.multi,
        };
        let _ = self
            .reply
            .send(files.choose(&options, self.filter.as_deref()));
    }
}
struct Screens {
    ctx: Context,
    dialogs: mpsc::Sender<Dialog>,
    files: Files,
    smoke: bool,
    updates: Arc<Updates>,
    discord: Arc<Mutex<Discord>>,
    ollama: Arc<string_lyrics::Ollama>,
}

pub fn connect(
    ctx: &Context,
    hwnd: Hwnd,
    ollama: Arc<string_lyrics::Ollama>,
    smoke: bool,
    updates: Arc<Updates>,
    discord: Arc<Mutex<Discord>>,
) -> mpsc::Receiver<Dialog> {
    let (dialogs, rx) = mpsc::channel();
    crate::ui::menus::hooks().reveal = Some(|v, path| v.reveal(path));
    crate::ui::menus::hooks().edit_info = Some(|v, id| crate::ui::edit_info::open(v, id));
    services::install(Arc::new(Screens {
        ctx: ctx.clone(),
        dialogs,
        files: Files::new(Some(hwnd), smoke),
        smoke,
        updates,
        discord,
        ollama,
    }));
    rx
}

impl Screens {
    fn pick(
        &self,
        title: &str,
        filter: Option<&str>,
        start: Option<&Path>,
        multi: bool,
    ) -> io::Result<Vec<PathBuf>> {
        if self.smoke {
            return Ok(vec![]);
        }
        let (reply, rx) = mpsc::channel();
        self.dialogs
            .send(Dialog {
                title: title.into(),
                filter: filter.map(str::to_owned),
                start: start.map(Path::to_path_buf),
                multi,
                reply,
            })
            .map_err(|_| io::Error::other("The window closed"))?;
        self.ctx.request_repaint();
        rx.recv()
            .map_err(|_| io::Error::other("The window closed"))?
    }
}
impl Services for Screens {
    fn pickers(&self) -> bool {
        true
    }
    fn pick_folders(&self, title: &str) -> io::Result<Vec<PathBuf>> {
        self.pick(title, None, None, true)
    }
    fn pick_files(
        &self,
        title: &str,
        filter: &str,
        start: Option<&Path>,
        multi: bool,
    ) -> io::Result<Vec<PathBuf>> {
        self.pick(title, Some(filter), start, multi)
    }
    fn reveal(&self, path: &Path) -> io::Result<()> {
        self.files.reveal(path)
    }
    fn recycle(&self, path: &Path) -> io::Result<()> {
        self.files.recycle(path)
    }
    fn open_link(&self, url: &str) -> io::Result<()> {
        self.files.open_web_link(url)
    }
    fn update_info(&self) -> UpdateInfo {
        self.updates.info.clone()
    }
    fn update_check(&self) -> io::Result<UpdateInfo> {
        self.updates.check()
    }
    fn update_install(
        &self,
        version: &str,
        progress: &mut dyn FnMut(UpdateProgress),
    ) -> io::Result<bool> {
        let ready = self.updates.prepare(version, progress)?;
        self.updates.launch(ready)?;
        Ok(true)
    }
    fn discord(&self) -> Option<Discord> {
        (!self.smoke).then(|| self.discord.lock().unwrap().clone())
    }
    fn translation_models(&self) -> io::Result<Vec<String>> {
        self.ollama.models()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_worker_marshals_picker_options_and_receives_the_result() {
        let (dialogs, rx) = mpsc::channel();
        let root = PathBuf::from(".");
        let screen = Screens {
            ctx: Context::default(),
            dialogs,
            files: Files::new(None, true),
            smoke: false,
            updates: Arc::new(Updates::new(&root, true)),
            discord: Default::default(),
            ollama: Arc::new(
                string_lyrics::Ollama::with_base_url(&root, "http://127.0.0.1:0").unwrap(),
            ),
        };
        let worker = std::thread::spawn(move || {
            screen.pick_files("Cover", "Pictures|*.png", Some(Path::new("album")), false)
        });
        let dialog = rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        assert_eq!(dialog.title, "Cover");
        assert_eq!(dialog.filter.as_deref(), Some("Pictures|*.png"));
        assert_eq!(dialog.start.as_deref(), Some(Path::new("album")));
        assert!(!dialog.multi);
        dialog
            .reply
            .send(Ok(vec![PathBuf::from("art.png")]))
            .unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap(),
            vec![PathBuf::from("art.png")]
        );
    }
}
