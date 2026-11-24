//! Windows services for the main window.
pub(crate) mod live_checks;
#[cfg(test)]
mod presence_test;
use crate::{
    Args, Error,
    integration::{
        files::Files,
        helpers::Helpers,
        media::Transport,
        phone::Phone,
        screens::{self, Dialog},
        updates::Updates,
    },
    playback::Session,
};
use egui::Context;
use std::{
    io,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::Duration,
};
use string_audio::Player;
use string_platform::{
    Hwnd,
    errors_log::ErrorLog,
    instance::{self, Claim, Instance, Names, Signal},
    media::{AppCommands, MediaButton, MediaControls},
    taskbar::{ThumbButton, ThumbButtons},
    window,
};
use windows::Win32::{
    Foundation::{HWND, RECT},
    UI::WindowsAndMessaging::{
        GetSystemMetrics, GetWindowRect, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE,
        SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
    },
};

/// What this launch does: run as String, or end at once with this exit code.
pub enum Start {
    Run(Instance),
    Done(i32),
}

/// How long `--add` waits for a String that is still starting to take its paths.
const ADD_WAIT: Duration = Duration::from_secs(5);

pub fn claim(args: &Args) -> io::Result<Start> {
    let names = if args.smoke.is_some() {
        Names::smoke()
    } else {
        Names::string()
    };
    claim_as(&names, args, ADD_WAIT)
}

/// `--quit` asks the running copy to close. `--add` hands its paths to the running copy without bringing it forward
/// (the helper that asked keeps its own window in front) and ends: 0 once that copy has them, 1 when it didn't take
/// them (String 1.x runs); with none running, this launch becomes String, the paths added. A plain launch brings the
/// running copy forward and ends.
pub(crate) fn claim_as(names: &Names, args: &Args, wait: Duration) -> io::Result<Start> {
    if args.quit {
        instance::request_quit(names);
        return Ok(Start::Done(0));
    }
    if let Some(paths) = &args.add {
        // nothing to add (none given, or none there): nothing to do
        if paths.is_empty() {
            return Ok(Start::Done(0));
        }
        return Ok(match instance::claim_quietly(names)? {
            Claim::First(instance) => Start::Run(instance),
            Claim::AlreadyRunning => Start::Done(if instance::send_add(names, paths, wait) {
                0
            } else {
                1
            }),
        });
    }
    Ok(match instance::claim(names)? {
        Claim::First(instance) => Start::Run(instance),
        Claim::AlreadyRunning => Start::Done(0),
    })
}

enum Command {
    Window(Signal),
    Media(MediaButton),
    Thumb(ThumbButton),
}

pub struct Platform {
    instance: Instance,
    root: PathBuf,
    hwnd: Option<Hwnd>,
    smoke: bool,
    uncloaking: bool,
    sender: mpsc::Sender<Command>,
    receiver: mpsc::Receiver<Command>,
    dialogs: Option<mpsc::Receiver<Dialog>>,
    media: Option<MediaControls>,
    app_commands: Option<AppCommands>,
    taskbar: Option<ThumbButtons>,
    live_check: live_checks::Probe,
    transport: Transport,
    pub log: Arc<ErrorLog>,
    pub phone: Phone,
    pub(crate) picker: Option<crate::integration::files::LibraryPicker>,
    pub updates: Arc<Updates>,
    pub helpers: Helpers,
    /// Paths `--add` brought (at the start, or from another launch), added once the library is open.
    added: Vec<PathBuf>,
    /// The ones given at the start, for the self-test.
    pub(crate) added_at_start: Vec<PathBuf>,
}

impl Platform {
    pub fn new(instance: Instance, args: &Args, log: Arc<ErrorLog>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let smoke = args.smoke.is_some();
        let mut helpers = Helpers::new(&args.root, smoke);
        for msg in helpers.refresh() {
            log.write("add-ons", msg);
        }
        let added = args.add.clone().unwrap_or_default();
        Self {
            instance,
            root: args.root.clone(),
            hwnd: None,
            smoke,
            uncloaking: false,
            sender,
            receiver,
            dialogs: None,
            media: None,
            app_commands: None,
            taskbar: None,
            live_check: Default::default(),
            transport: Transport::default(),
            log,
            phone: Phone::new(&args.root, smoke),
            updates: Arc::new(Updates::new(&args.root, smoke)),
            picker: None,
            helpers,
            added_at_start: added.clone(),
            added,
        }
    }

    pub fn configure(&self, v: egui::ViewportBuilder) -> egui::ViewportBuilder {
        if self.smoke {
            v.with_app_id(Names::smoke().mutex)
                .with_active(false)
                .with_position([-30000., -30000.])
                .with_taskbar(false)
        } else {
            v.with_app_id("String.Native")
        }
    }

    pub fn attach(
        &mut self,
        cc: &eframe::CreationContext<'_>,
        ollama: std::sync::Arc<string_lyrics::Ollama>,
    ) -> Result<(), Error> {
        let hwnd = Hwnd::from_window(cc).ok_or("A Windows window is required")?;
        self.hwnd = Some(hwnd);
        self.helpers.attach(&cc.egui_ctx);
        if let Some(e) = self.instance.add_error() {
            self.log.write("--add", e);
        }
        self.dialogs = Some(screens::connect(
            &cc.egui_ctx,
            hwnd,
            ollama,
            self.smoke,
            self.updates.clone(),
            self.transport.discord_status(),
        ));
        self.optional("title bar", window::style_title_bar(hwnd));
        window::set_background(hwnd, window::BACKGROUND);
        if self.smoke {
            offscreen(hwnd)?;
            live_checks::contain_keys(hwnd)?;
        }
        let send = self.sender.clone();
        let ctx = cc.egui_ctx.clone();
        let post = move |command| {
            let _ = send.send(command);
            ctx.request_repaint();
        };
        if !self.smoke {
            let listener = post.clone();
            self.instance.listen(move |s| listener(Command::Window(s)));
        }
        let keys = post.clone();
        self.app_commands = self.optional(
            "media keys",
            AppCommands::new(hwnd, move |b| keys(Command::Media(b))),
        );
        let buttons = post.clone();
        let pressed = move |b| buttons(Command::Thumb(b));
        self.taskbar = self.optional(
            "taskbar",
            if self.smoke {
                ThumbButtons::messages_only(hwnd, pressed)
            } else {
                ThumbButtons::new(hwnd, pressed)
            },
        );
        self.media = self.optional(
            "media controls",
            MediaControls::new(hwnd, move |b| post(Command::Media(b))),
        );
        if self.smoke {
            return Ok(());
        }
        if window::restore_placement(
            hwnd,
            &self.root.join("window.json"),
            (1400., 880.),
            window::MIN_SIZE,
        ) {
            self.uncloaking = self
                .optional("window cloak", window::cloak(hwnd, true))
                .is_some();
            window::show(hwnd, true);
            cc.egui_ctx.request_repaint();
        }
        Ok(())
    }

    fn optional<T>(&self, place: &str, r: io::Result<T>) -> Option<T> {
        r.map_err(|e| self.log.write(place, e)).ok()
    }

    pub fn files(&self) -> Files {
        Files::new(self.hwnd, self.smoke)
    }
    /// The paths chosen in the picker asked for, and which picker it was; `None` when none was asked for.
    pub fn picked_paths(
        &mut self,
    ) -> io::Result<Option<(crate::integration::files::LibraryPicker, Vec<PathBuf>)>> {
        let Some(which) = self.picker.take() else {
            return Ok(None);
        };
        Ok(Some((which, self.files().pick(which)?)))
    }

    /// The paths `--add` brought since the last call.
    pub fn take_added(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.added)
    }

    /// Lists the helpers again when the window gains the focus; refusals go to the error log.
    pub fn helpers_focus(&mut self, focused: bool) {
        for msg in self.helpers.focus(focused) {
            self.log.write("add-ons", msg);
        }
    }

    pub fn open_dialogs(&self) {
        if let Some(rx) = &self.dialogs {
            for dialog in rx.try_iter() {
                dialog.open(self.files());
            }
        }
    }

    pub fn save_placement(&self) {
        if let Some(hwnd) = self.hwnd.filter(|_| !self.smoke) {
            self.optional(
                "window placement",
                window::save_placement(hwnd, &self.root.join("window.json")),
            );
        }
    }

    pub(crate) fn smoke_seek(&self, to: std::time::Duration) {
        if self.smoke {
            let _ = self.sender.send(Command::Media(MediaButton::Seek(to)));
        }
    }

    pub(crate) fn smoke_play_pause(&self) {
        if let Some(hwnd) = self.hwnd.filter(|_| self.smoke) {
            live_checks::thumbnail(hwnd, 0x5302);
        }
    }

    pub fn commands(&mut self, ctx: &Context, mut playback: Option<(&mut Session, &Player)>) {
        if self.uncloaking {
            if ctx.cumulative_frame_nr() > 0 {
                self.uncloaking = false;
                self.optional("window cloak", window::cloak(self.hwnd.unwrap(), false));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
                ctx.request_repaint();
            }
        }
        for command in self.receiver.try_iter() {
            match command {
                Command::Window(Signal::Show) => {
                    if let Some(hwnd) = self.hwnd {
                        window::bring_to_front(hwnd);
                        self.phone.request();
                    }
                }
                Command::Window(Signal::Quit) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close)
                }
                Command::Window(Signal::Add(paths)) => self.added.extend(paths),
                command => {
                    if let Some((s, p)) = playback.as_mut() {
                        match command {
                            Command::Thumb(ThumbButton::PlayPause)
                            | Command::Media(MediaButton::Toggle) => s.toggle(p),
                            Command::Thumb(ThumbButton::Next)
                            | Command::Media(MediaButton::Next) => s.next(p),
                            Command::Thumb(ThumbButton::Previous)
                            | Command::Media(MediaButton::Previous) => s.previous(p),
                            Command::Media(MediaButton::Play) => s.resume(p),
                            Command::Media(MediaButton::Pause | MediaButton::Stop) => p.pause(),
                            Command::Media(MediaButton::Rewind) => {
                                s.seek(p, p.status().position - 10.)
                            }
                            Command::Media(MediaButton::FastForward) => {
                                s.seek(p, p.status().position + 10.)
                            }
                            Command::Media(MediaButton::Seek(to)) => s.seek(p, to.as_secs_f64()),
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    pub fn publish(&mut self, s: &Session, st: string_audio::Status, ctx: &Context) {
        if !self.smoke {
            self.transport.update(
                s,
                st,
                self.media.as_ref(),
                self.taskbar.as_ref(),
                &self.root,
                &self.log,
                ctx,
            );
        }
    }
}

// Verify the actual window is outside the virtual desktop before eframe presents it.
fn offscreen(handle: Hwnd) -> io::Result<()> {
    let hwnd = HWND(handle.0 as *mut _);
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        };
        let old = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            old | (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize,
        );
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN) - 8000;
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN) - 8000;
        SetWindowPos(
            hwnd,
            None,
            x,
            y,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
        )?;
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r)?;
        if r.right >= GetSystemMetrics(SM_XVIRTUALSCREEN)
            || r.bottom >= GetSystemMetrics(SM_YVIRTUALSCREEN)
        {
            return Err(io::Error::other("Could not verify off-screen placement"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // Never String's own names (StringApp.*): a test would raise, close or add to the person's String.
    use super::*;
    use std::{
        ffi::OsString,
        path::Path,
        sync::atomic::{AtomicU32, Ordering},
    };

    fn names() -> Names {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = Names::custom(&format!(
            "StringAppTest.{}.{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(!n.mutex.starts_with("StringApp."));
        n
    }

    fn folder() -> tempfile::TempDir {
        let base = std::path::absolute(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-runs/add-checks"),
        )
        .unwrap();
        std::fs::create_dir_all(&base).unwrap();
        tempfile::tempdir_in(base).unwrap()
    }

    fn args(data: &Path, list: &[&Path]) -> Args {
        let mut a: Vec<OsString> = vec!["--data".into(), data.into()];
        a.extend(list.iter().map(|p| p.as_os_str().to_owned()));
        Args::parse_from(a).unwrap()
    }

    fn adding(data: &Path, paths: &[&Path]) -> Args {
        let mut list: Vec<&Path> = vec![Path::new("--add")];
        list.extend(paths);
        args(data, &list)
    }

    fn running(names: &Names, data: &Path) -> (Instance, mpsc::Receiver<Signal>) {
        let Start::Run(mut me) = claim_as(names, &args(data, &[]), Duration::ZERO).unwrap() else {
            panic!("expected to run")
        };
        let (tx, rx) = mpsc::channel();
        me.listen(move |s| {
            let _ = tx.send(s);
        });
        (me, rx)
    }

    #[test]
    fn add_takes_the_paths_up_to_the_next_option_that_exist() {
        let t = folder();
        let song = t.path().join("song.mp3");
        std::fs::write(&song, b"").unwrap();
        let a = Args::parse_from([
            OsString::from("--add"),
            song.clone().into(),
            "".into(),
            t.path().into(),
            t.path().join("missing.mp3").into(),
            "relative-missing.mp3".into(),
            "--data".into(),
            t.path().into(),
        ])
        .unwrap();
        assert_eq!(a.add, Some(vec![song, t.path().to_path_buf()]));
        assert_eq!(a.root, t.path());
        assert_eq!(adding(t.path(), &[]).add, Some(vec![]));
        assert_eq!(args(t.path(), &[]).add, None);
    }

    #[test]
    fn add_hands_paths_to_the_running_copy_without_raising_it() {
        let (t, names) = (folder(), names());
        let song = t.path().join("a.flac");
        std::fs::write(&song, b"").unwrap();
        let (_me, rx) = running(&names, t.path());
        let add = adding(t.path(), &[&song, t.path()]);
        assert!(matches!(
            claim_as(&names, &add, Duration::from_secs(5)).unwrap(),
            Start::Done(0)
        ));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Signal::Add(vec![song, t.path().to_path_buf()]))
        );
        // nothing there to add: nothing happens
        let none = adding(t.path(), &[&t.path().join("gone.mp3")]);
        assert!(matches!(
            claim_as(&names, &none, Duration::from_secs(5)).unwrap(),
            Start::Done(0)
        ));
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(300)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        // a plain second launch still brings it forward
        assert!(matches!(
            claim_as(&names, &args(t.path(), &[]), Duration::ZERO).unwrap(),
            Start::Done(0)
        ));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(Signal::Show));
    }

    #[test]
    fn add_with_none_running_starts_string_with_the_paths() {
        let (t, names) = (folder(), names());
        let add = adding(t.path(), &[t.path()]);
        let Start::Run(_me) = claim_as(&names, &add, Duration::ZERO).unwrap() else {
            panic!("expected to run")
        };
        assert_eq!(add.add, Some(vec![t.path().to_path_buf()]));
    }

    /// The running copy in a process of its own, for `add_reaches_another_process`: it writes `ready`, then what one
    /// `--add` brought to `got.txt`. Run alone it does nothing.
    #[test]
    #[ignore = "run by add_reaches_another_process"]
    fn other_process_copy() {
        let (Some(prefix), Some(out)) = (
            std::env::var_os("STRING_TEST_INSTANCE"),
            std::env::var_os("STRING_TEST_OUT"),
        ) else {
            return;
        };
        let out = PathBuf::from(out);
        let names = Names::custom(prefix.to_str().unwrap());
        let Claim::First(mut me) = instance::claim(&names).unwrap() else {
            panic!("expected to be the first copy")
        };
        let (tx, rx) = mpsc::channel();
        me.listen(move |s| {
            let _ = tx.send(s);
        });
        std::fs::write(out.join("ready"), b"").unwrap();
        if let Ok(Signal::Add(paths)) = rx.recv_timeout(Duration::from_secs(30)) {
            let text: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into()).collect();
            std::fs::write(
                out.join("got.tmp"),
                text.join(
                    "
",
                ),
            )
            .unwrap();
            std::fs::rename(out.join("got.tmp"), out.join("got.txt")).unwrap();
        }
    }

    fn wait_for(path: &Path) -> bool {
        let end = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < end {
            if path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn add_reaches_another_process() {
        let (t, names) = (folder(), names());
        let prefix = names.mutex.trim_end_matches(".SingleInstance").to_owned();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "platform::tests::other_process_copy",
                "--ignored",
                "--test-threads=1",
            ])
            .env("STRING_TEST_INSTANCE", &prefix)
            .env("STRING_TEST_OUT", t.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        assert!(
            wait_for(&t.path().join("ready")),
            "the other copy didn't start"
        );
        let song = t.path().join("Ágætis — ✓.flac");
        std::fs::write(&song, b"").unwrap();
        let add = adding(t.path(), &[&song, t.path()]);
        assert!(matches!(
            claim_as(&names, &add, Duration::from_secs(5)).unwrap(),
            Start::Done(0)
        ));
        assert!(wait_for(&t.path().join("got.txt")));
        assert_eq!(
            std::fs::read_to_string(t.path().join("got.txt")).unwrap(),
            format!(
                "{}
{}",
                song.display(),
                t.path().display()
            )
        );
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn add_to_a_copy_without_the_channel_fails() {
        // the mutex held by a copy with no --add window, as String 1.x
        let (t, names) = (folder(), names());
        let old = Names {
            add: None,
            ..names.clone()
        };
        let (_me, _rx) = running(&old, t.path());
        let add = adding(t.path(), &[t.path()]);
        assert!(matches!(
            claim_as(&names, &add, Duration::from_millis(200)).unwrap(),
            Start::Done(1)
        ));
    }
}
