//! Windows services for the main window.
use crate::{
    Args, Error,
    integration::{
        files::Files,
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
};
use string_audio::Player;
use string_platform::{
    Hwnd,
    errors_log::ErrorLog,
    instance::{self, Claim, Instance, Names, Signal},
    media::{MediaButton, MediaControls},
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

pub fn claim(smoke: bool, quit: bool) -> io::Result<Option<Instance>> {
    let names = if smoke {
        Names::smoke()
    } else {
        Names::string()
    };
    if quit {
        instance::request_quit(&names);
        return Ok(None);
    }
    match instance::claim(&names)? {
        Claim::First(instance) => Ok(Some(instance)),
        Claim::AlreadyRunning => Ok(None),
    }
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
    taskbar: Option<ThumbButtons>,
    transport: Transport,
    pub log: Arc<ErrorLog>,
    pub phone: Phone,
    pub(crate) picker: Option<crate::integration::files::LibraryPicker>,
    pub updates: Arc<Updates>,
}

impl Platform {
    pub fn new(instance: Instance, args: &Args, log: Arc<ErrorLog>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let smoke = args.smoke.is_some();
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
            taskbar: None,
            transport: Transport::default(),
            log,
            phone: Phone::new(&args.root, smoke),
            updates: Arc::new(Updates::new(&args.root, smoke)),
            picker: None,
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

    pub fn attach(&mut self, cc: &eframe::CreationContext<'_>) -> Result<(), Error> {
        let hwnd = Hwnd::from_window(cc).ok_or("A Windows window is required")?;
        self.hwnd = Some(hwnd);
        self.dialogs = Some(screens::connect(
            &cc.egui_ctx,
            hwnd,
            &self.root,
            self.smoke,
            self.updates.clone(),
            self.transport.discord_status(),
        ));
        self.optional("title bar", window::style_title_bar(hwnd));
        window::set_background(hwnd, window::BACKGROUND);
        if self.smoke {
            return Ok(offscreen(hwnd)?);
        }
        let send = self.sender.clone();
        let ctx = cc.egui_ctx.clone();
        let post = move |command| {
            let _ = send.send(command);
            ctx.request_repaint();
        };
        let listener = post.clone();
        self.instance.listen(move |s| listener(Command::Window(s)));
        let buttons = post.clone();
        self.taskbar = self.optional(
            "taskbar",
            ThumbButtons::new(hwnd, move |b| buttons(Command::Thumb(b))),
        );
        self.media = self.optional(
            "media controls",
            MediaControls::new(hwnd, move |b| post(Command::Media(b))),
        );
        if window::restore_placement(
            hwnd,
            &self.root.join("window.json"),
            (1400., 880.),
            (820., 520.),
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
    pub fn picked_paths(&mut self) -> io::Result<Vec<PathBuf>> {
        match self.picker.take() {
            Some(which) => self.files().pick(which),
            None => Ok(Vec::new()),
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
        if self.smoke {
            let _ = self.sender.send(Command::Thumb(ThumbButton::PlayPause));
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
                command => {
                    if let Some((s, p)) = playback.as_mut() {
                        match command {
                            Command::Thumb(ThumbButton::PlayPause) => s.toggle(p),
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
