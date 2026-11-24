use crate::{
    Args, Error,
    platform::Platform,
    playback::{self, Shared},
    route::Route,
    scan::Scanner,
    shell,
    smoke::Smoke,
    storage::{self, Storage},
    theme::{self, Tint},
    ui::{self, covers::CoverCache},
    view::{View, Views},
    viz::Viz,
};
use egui::{Context, Ui, ViewportCommand};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicI32, Ordering},
    },
    time::{Duration, Instant},
};
use string_audio::{Player, Spectrum};
const ADD_CLOSE_WAIT: Duration = Duration::from_secs(1);
/// Whether an add String has taken isn't finished: a drop still being sorted, or paths being checked. The sorter
/// queues a drop's songs before it stops counting the drop, so asking it first never misses songs passing between.
fn add_unfinished(sorting: impl FnOnce() -> bool, checking: impl FnOnce() -> bool) -> bool {
    sorting() || checking()
}
struct Runtime {
    session: Shared,
    player: Player,
    spectrum: Spectrum,
    store: Storage,
}
pub struct App {
    args: Args,
    runtime: Option<Runtime>,
    error: String,
    platform: Platform,
    adds: crate::add_work::Adds,
    views: Views,
    covers: CoverCache,
    viz: Viz,
    scan: Scanner,
    smoke: Option<Smoke>,
    exit: Arc<AtomicI32>,
    closing: bool,
    /// Finish accepted adds and pending moves to the Recycle Bin before closing.
    close_work_deadline: Option<Instant>,
    minimized: bool,
    last: Instant,
    close_reason: String,
    transport_until: Instant,
}
impl App {
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        args: Args,
        mut platform: Platform,
        adds: crate::add_work::Adds,
        exit: Arc<AtomicI32>,
    ) -> Result<Self, Error> {
        let ctx = &cc.egui_ctx;
        let lyrics = crate::screens::lyrics::LyricsView::new(&args.root, args.smoke.is_some());
        platform.attach(cc, lyrics.ollama())?;
        adds.attach(ctx);
        theme::install(ctx);
        let now = Instant::now();
        let mut a = Self {
            smoke: args.smoke.clone().map(Smoke::new),
            args,
            runtime: None,
            error: String::new(),
            platform,
            adds,
            views: Views {
                lyrics: Some(lyrics),
                ..Default::default()
            },
            covers: CoverCache::new(ctx),
            viz: Default::default(),
            scan: Default::default(),
            exit,
            closing: false,
            close_work_deadline: None,
            minimized: false,
            last: now,
            close_reason: String::new(),
            transport_until: now,
        };
        a.open(ctx);
        Ok(a)
    }
    fn open(&mut self, ctx: &Context) {
        let folder = storage::folder(&self.args.root, self.args.explicit);
        match Storage::open(&folder, ctx) {
            Ok((mut store, s)) => {
                Self::save_feedback(&self.platform, &mut self.views, &mut store);
                self.views.history.current =
                    Route::parse(s.field("route").as_str().unwrap_or("home"));
                if let Some(sm) = &mut self.smoke
                    && s.current().is_some()
                {
                    sm.expect_resume(s.state.queue.pos());
                }
                let session = Arc::new(Mutex::new(s));
                let player = playback::open(&session, ctx, self.smoke.is_some());
                self.runtime = Some(Runtime {
                    spectrum: player.spectrum(),
                    player,
                    session,
                    store,
                });
                self.error.clear();
                self.scan.requested = true;
            }
            Err(e) => {
                self.error = e.to_string();
                self.platform.log.write("open profile", &self.error);
            }
        }
    }
    /// An add that String has taken but not finished: paths being checked, or a drop still being sorted.
    fn adding(&self) -> bool {
        add_unfinished(|| self.views.drops.sorting(), || self.adds.pending())
    }
    fn save_feedback(platform: &Platform, views: &mut Views, store: &mut Storage) {
        for msg in store.feedback.logs.drain(..) {
            platform.log.write("save", &msg);
        }
        for msg in store.feedback.messages.drain(..) {
            views.toast.set(msg);
        }
    }
    fn flush(&mut self) -> bool {
        self.adds.set_closed(true);
        let add_deadline = self
            .close_work_deadline
            .take()
            .unwrap_or_else(|| Instant::now() + ADD_CLOSE_WAIT);
        if let Some(rt) = &self.runtime {
            loop {
                while let Some(batch) = self.adds.next_at_exit(add_deadline) {
                    batch.apply(&mut rt.session.lock().unwrap(), &mut self.scan);
                }
                // A drop still being sorted queues its songs once sorted; no new drop starts now adds are closed.
                if !self.views.drops.wait_sorted(add_deadline)
                    || !self.adds.pending()
                    || Instant::now() >= add_deadline
                {
                    break;
                }
            }
        }
        let mut reasons = Vec::new();
        if self.adding() {
            reasons.push(
                if self.runtime.is_some() {
                    "String is still checking paths for a pending add."
                } else {
                    "String cannot apply the pending add because the profile could not open."
                }
                .to_owned(),
            );
        }
        self.platform.save_placement();
        // Completed work gets the full save window even when a filesystem check has stalled.
        let deadline = Instant::now() + Duration::from_secs(10);
        if let Some(error) = self.platform.phone.wait(deadline) {
            if self.platform.phone.busy() {
                reasons.push(error);
                self.close_reason = reasons.join("\n");
                return false;
            }
            self.platform.log.write("phone listening", &error);
        }
        let Some(rt) = self.runtime.as_mut() else {
            self.close_reason = reasons.join("\n");
            return reasons.is_empty();
        };
        let mut s = rt.session.lock().unwrap();
        s.tick(rt.player.status(), true);
        if let Some(msg) = self.platform.phone.backup_warning.take() {
            rt.store.feedback.backup(msg);
        }
        let r = rt.store.flush_until(&mut s, deadline);
        if r.failed() {
            reasons.push(
                rt.store
                    .feedback
                    .reason
                    .clone()
                    .unwrap_or_else(|| "The profile could not be saved.".into()),
            );
        }
        self.close_reason = reasons.join("\n");
        Self::save_feedback(&self.platform, &mut self.views, &mut rt.store);
        reasons.is_empty()
    }
    fn finish(&mut self, ctx: &Context, mut err: Option<String>) {
        if !self.flush() && err.is_none() {
            err = Some(self.close_reason.clone());
        }
        if let Some(sm) = &self.smoke
            && let Err(e) = sm.report(err.as_deref())
        {
            err = Some(e.to_string());
        }
        if let Some(e) = &err
            && e != &self.close_reason
        {
            self.platform.log.write("close", e);
        }
        self.exit.store(i32::from(err.is_some()), Ordering::Release);
        self.closing = true;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
}
impl eframe::App for App {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0196, 0.0196, 0.0235, 1.]
    }
    fn logic(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
        if self.closing {
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            self.adds.set_closed(true);
            let now = Instant::now();
            let deadline = *self.close_work_deadline.get_or_insert(now + ADD_CLOSE_WAIT);
            // songs still moving to the Recycle Bin: wait for them, so the copies kept take over their likes,
            // plays and playlist places before the last save
            if crate::screens::dupes::recycling()
                || (self.runtime.is_some() && self.adding() && now < deadline)
            {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                ctx.request_repaint_after(Duration::from_millis(50));
            } else if self.flush() {
                self.closing = true;
                return;
            } else {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.views.close_failed = true;
            }
        }
        if self.smoke.is_some() {
            ctx.send_viewport_cmd(ViewportCommand::RequestPaintWhileHidden);
        }
        self.minimized = frame
            .winit_window()
            .and_then(|w| w.is_minimized())
            .unwrap_or(false);
        self.platform.open_dialogs();
        let Some(rt) = &mut self.runtime else {
            self.platform.commands(ctx, None);
            return;
        };
        let mut s = rt.session.lock().unwrap();
        self.platform.commands(ctx, Some((&mut s, &rt.player)));
        crate::integration::update_check::tick(ctx, &mut s, &mut self.views, self.smoke.is_some());
        let st = rt.player.status();
        if let Some(lyrics) = &mut self.views.lyrics {
            lyrics.check_visible(self.views.overlay, self.minimized, s.state.queue.current());
        }
        rt.store.tick(&mut s, st, Instant::now());
        // Songs moved to the Recycle Bin are merged into the copies kept here, which runs while String is minimised
        // too; a close that waited for them goes on once they are. Paths from `--add` are added here too, so a
        // helper's songs come in while String is minimised.
        if !self.views.close_failed {
            // Hold unapplied results until the person decides whether to keep the profile open.
            self.views.drops.poll(&mut s, &mut self.views.toast);
            self.adds
                .poll(&mut s, &mut self.scan, &mut self.views.toast);
        }
        let recycling = crate::screens::dupes::recycling();
        if recycling {
            let tint = Tint::parse(s.setting("tint").as_str());
            let mono = s.setting("artMono").as_bool().unwrap_or(true);
            let mut v = View {
                adds: &self.adds.input,
                platform: &mut self.platform,
                s: &mut s,
                player: &rt.player,
                st,
                views: &mut self.views,
                covers: &mut self.covers,
                viz: &self.viz,
                scan: &mut self.scan,
                tint,
                mono,
                force_mute: self.smoke.is_some(),
            };
            crate::screens::dupes::settle(&mut v);
        }
        if let Some(deadline) = self.close_work_deadline {
            let adding = add_unfinished(|| self.views.drops.sorting(), || self.adds.pending());
            if crate::screens::dupes::recycling() || (adding && Instant::now() < deadline) {
                ctx.request_repaint_after(Duration::from_millis(50));
            } else {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
        self.platform.publish(&s, st, ctx);
        if s.resync {
            s.resync = false;
            let pos = s.state.queue.pos();
            s.load_current(&rt.player, st.playing, pos);
        }
        let generation = s.generation;
        let errors = self.scan.poll(&mut s, ctx);
        if generation != s.generation {
            rt.player.queue_changed();
            if st.key.is_none() && s.current().is_some() {
                let pos = s.state.queue.pos();
                s.load_current(&rt.player, false, pos);
            }
        }
        for msg in errors {
            self.platform.log.write("scan", &msg);
            self.views.toast.set(msg);
        }
        while let Some(msg) = s.notices.pop_front() {
            self.platform.log.write("player", &msg);
            self.views.toast.set(msg);
        }
        rt.store.poll(&mut s);
        if let Some(msg) = s.poll_spotify() {
            self.views.toast.set(msg);
        }
        if self.views.stats.sync_phone(&self.platform.phone.users) {
            self.platform.phone.request();
        }
        if let Some(msg) = self.platform.phone.poll(
            s.setting("phoneStats") != false,
            self.minimized,
            &rt.session,
            &rt.store,
            ctx,
        ) {
            self.platform.log.write("phone listening", &msg);
        }
        if let Some(msg) = self.platform.phone.backup_warning.take() {
            rt.store.feedback.backup(msg);
        }
        if let Some(msg) = self.platform.phone.worker_warning.take() {
            self.views.toast.set(msg);
        }
        Self::save_feedback(&self.platform, &mut self.views, &mut rt.store);
        if !self.platform.phone.busy()
            && let Some(delay) = rt.store.autosave(&mut s, Instant::now())
        {
            ctx.request_repaint_after(delay);
        }
        if self.minimized && st.playing {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        if self.scan.running() {
            ctx.request_repaint_after(Duration::from_millis(350));
        }
    }
    fn ui(&mut self, ui: &mut Ui, _: &mut eframe::Frame) {
        if self.closing {
            return;
        }
        let ctx = ui.ctx().clone();
        if self.minimized && self.smoke.is_none() {
            return;
        }
        let adding = self.adding();
        if self.views.close_failed
            && let Some(quit) = ui::confirm(
                &ctx,
                if adding {
                    "String couldn’t finish the add"
                } else {
                    "Your latest changes aren’t saved"
                },
                &format!(
                    "{}\n\n{}",
                    self.close_reason,
                    if adding {
                        "Keep String open to retry, or close and discard the pending add and any unsaved changes."
                    } else {
                        "Keep String open to retry saving, or close and discard the unsaved changes."
                    }
                ),
                "Close anyway",
            )
        {
            self.views.close_failed = false;
            if quit {
                self.closing = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
                return;
            }
            self.adds.set_closed(false);
        }
        if self.runtime.is_none() {
            let mut retry = false;
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.35);
                ui.heading("Your library could not open");
                ui.label(&self.error);
                if ui::btn(ui, "Try again").clicked() {
                    retry = true;
                }
            });
            if retry {
                self.open(&ctx);
            }
            if self.smoke.is_some() {
                self.finish(&ctx, Some(self.error.clone()));
            }
            return;
        }
        let rt = self.runtime.as_mut().unwrap();
        self.covers.poll(&ctx);
        let picked = self.platform.picked_paths().unwrap_or_else(|e| {
            self.views.toast.set(e.to_string());
            None
        });
        let mut s = rt.session.lock().unwrap();
        let st = rt.player.status();
        let tint = Tint::parse(s.setting("tint").as_str());
        let mono = s.setting("artMono").as_bool().unwrap_or(true);
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;
        let mut raw = [0.; 64];
        rt.spectrum.read(&mut raw);
        self.viz.tint = tint;
        let animate = st.playing || self.viz.settling();
        if animate {
            self.viz.step(&raw, st.playing, dt);
        }
        let mut v = View {
            adds: &self.adds.input,
            platform: &mut self.platform,
            s: &mut s,
            player: &rt.player,
            st,
            views: &mut self.views,
            covers: &mut self.covers,
            viz: &self.viz,
            scan: &mut self.scan,
            tint,
            mono,
            force_mute: self.smoke.is_some(),
        };
        if ctx.input(|i| !i.events.is_empty()) {
            self.transport_until = now + Duration::from_millis(350);
        }
        match picked {
            Some((picker, paths)) if picker.spotify() => {
                if !paths.is_empty()
                    && let Err(why) = v.s.import_spotify(paths)
                {
                    v.views.toast.set(why);
                }
            }
            Some((_, paths)) => {
                for path in paths {
                    v.add_path(path);
                }
            }
            None => {}
        }
        shell::shortcuts(ui, &mut v);
        shell::show(ui, &mut v);
        if v.platform.picker.is_some() {
            ctx.request_repaint();
        }
        let result = self.smoke.as_mut().map(|sm| sm.tick(&ctx, &mut v));
        drop(s);
        match result {
            Some(Err(e)) => self.finish(&ctx, Some(e)),
            Some(Ok(true)) => self.finish(&ctx, None),
            _ if animate || self.smoke.is_some() => ctx.request_repaint(),
            _ if now < self.transport_until => ctx.request_repaint_after(Duration::from_millis(16)),
            _ => {}
        }
    }
    fn on_exit(&mut self) {
        if !self.closing && !self.flush() {
            self.platform.log.write("close", &self.close_reason);
        }
    }
}

#[cfg(test)]
mod add_close_tests {
    use super::*;
    use crate::add_work::Adds;
    use std::{path::Path, sync::mpsc};
    use string_platform::instance::{self, Claim, Names};

    fn app(root: &Path, adds: Adds) -> App {
        let ctx = Context::default();
        let args = Args {
            root: root.to_owned(),
            explicit: true,
            smoke: Some(root.join("out")),
            quit: false,
            add: None,
            playlist: None,
        };
        let names = Names {
            add: None,
            ..Names::custom(&format!("AddClose.{}", rand::random::<u64>()))
        };
        let Claim::First(first) = instance::claim_quietly(&names).unwrap() else {
            panic!()
        };
        let log = Arc::new(string_platform::errors_log::ErrorLog::new(root));
        let now = Instant::now();
        App {
            platform: Platform::new(first, &args, log),
            args,
            runtime: None,
            adds,
            error: String::new(),
            views: Views::default(),
            covers: CoverCache::new(&ctx),
            viz: Default::default(),
            scan: Scanner::default(),
            smoke: Some(Smoke::new(root.join("out"))),
            exit: Default::default(),
            closing: false,
            close_work_deadline: None,
            minimized: false,
            last: now,
            close_reason: String::new(),
            transport_until: now,
        }
    }

    #[test]
    fn review_close_explains_pending_add_without_an_open_profile() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = app(dir.path(), Adds::new().unwrap());
        assert!(
            a.adds
                .input
                .submit(None, vec![dir.path().join("song.wav")], 128)
        );
        assert!(!a.flush());
        assert!(
            a.close_reason.contains("profile") && a.close_reason.contains("add"),
            "close reason: {:?}",
            a.close_reason
        );
    }

    #[test]
    fn review_close_saves_completed_changes_with_a_stalled_add() {
        let dir = tempfile::tempdir().unwrap();
        let (release, gate) = mpsc::channel();
        let adds = Adds::with_classifier(move |paths| {
            let _ = gate.recv_timeout(Duration::from_secs(5));
            crate::add_work::classify(paths)
        })
        .unwrap();
        let mut a = app(dir.path(), adds);
        a.open(&Context::default());
        let song = dir.path().join("song.wav");
        std::fs::write(&song, []).unwrap();
        a.runtime
            .as_ref()
            .unwrap()
            .session
            .lock()
            .unwrap()
            .set_field("route", "liked".into());
        assert!(a.adds.input.submit(None, vec![song.clone()], 128));
        let start = Instant::now();
        assert!(
            !a.flush(),
            "close consumed a stalled add without a decision"
        );
        assert!(start.elapsed() < Duration::from_secs(4));
        assert!(a.close_reason.contains("add"), "{}", a.close_reason);
        let folder = storage::folder(dir.path(), true);
        assert_eq!(folder.open().unwrap().state.value.fields["route"], "liked");
        release.send(()).unwrap();
        assert!(a.flush());
        assert_eq!(
            folder.open().unwrap().state.value.fields["files"],
            serde_json::json!([song])
        );
    }

    #[test]
    fn review_normal_close_reaches_a_decision_while_worker_is_stalled() {
        let dir = tempfile::tempdir().unwrap();
        let (_release, gate) = mpsc::channel::<()>();
        let adds = Adds::with_classifier(move |_| {
            let _ = gate.recv_timeout(Duration::from_secs(8));
            Vec::new()
        })
        .unwrap();
        let ctx = Context::default();
        let mut a = app(dir.path(), adds);
        a.open(&ctx);
        assert!(
            a.adds
                .input
                .submit(None, vec![dir.path().join("pending.wav")], 128)
        );
        let start = Instant::now();
        let mut close = true;
        let mut frame = eframe::Frame::_new_kittest();
        while !a.views.close_failed {
            let mut input = egui::RawInput::default();
            if close {
                input
                    .viewports
                    .get_mut(&egui::ViewportId::ROOT)
                    .unwrap()
                    .events
                    .push(egui::ViewportEvent::Close);
            }
            let out = ctx.run_ui(input, |ui| eframe::App::logic(&mut a, ui.ctx(), &mut frame));
            close = out.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|cmd| matches!(cmd, ViewportCommand::Close));
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "close never offered a decision"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(a.adds.pending() && !a.closing);
    }

    #[test]
    fn a_close_waits_for_a_drop_still_being_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("song.wav");
        std::fs::write(&song, []).unwrap();
        let (entered, sorting) = mpsc::channel();
        let (release, gate) = mpsc::channel::<()>();
        let ctx = Context::default();
        let mut a = app(dir.path(), Adds::new().unwrap());
        a.open(&ctx);
        a.views.drops = crate::integration::spotify::Drops::with_classifier(move |_| {
            let _ = entered.send(());
            let _ = gate.recv_timeout(Duration::from_secs(5));
            string_extras::spotify::Holds {
                music: true,
                ..Default::default()
            }
        });
        a.views.drops.attach(&ctx);
        assert!(a.views.drops.send(vec![song.clone()], false, &a.adds.input));
        sorting.recv_timeout(Duration::from_secs(2)).unwrap();
        let start = Instant::now();
        let mut frame = eframe::Frame::_new_kittest();
        let mut close = true;
        let mut first = true;
        while !a.closing {
            let mut input = egui::RawInput::default();
            if close {
                input
                    .viewports
                    .get_mut(&egui::ViewportId::ROOT)
                    .unwrap()
                    .events
                    .push(egui::ViewportEvent::Close);
            }
            let out = ctx.run_ui(input, |ui| eframe::App::logic(&mut a, ui.ctx(), &mut frame));
            let commands = &out.viewport_output[&egui::ViewportId::ROOT].commands;
            if std::mem::take(&mut first) {
                assert!(
                    !a.closing,
                    "the close didn't wait for the drop being sorted"
                );
                assert!(commands.contains(&ViewportCommand::CancelClose));
                release.send(()).unwrap();
            }
            close = commands.contains(&ViewportCommand::Close);
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the close never ended"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let folder = storage::folder(dir.path(), true);
        assert_eq!(
            folder.open().unwrap().state.value.fields["files"],
            serde_json::json!([song])
        );
    }

    #[test]
    fn the_close_check_asks_the_sorter_before_the_add_queue() {
        // The sorter puts a drop's songs in the add queue, then stops counting the drop. Should that happen between
        // the two questions, asking the queue first would hear "no" from both.
        let moved = std::cell::Cell::new(false);
        let sorting = || !moved.replace(true);
        let checking = || moved.replace(true);
        assert!(add_unfinished(sorting, checking));
    }

    #[test]
    fn an_exit_without_close_frames_waits_for_a_drop_being_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("song.wav");
        std::fs::write(&song, []).unwrap();
        let (entered, sorting) = mpsc::channel();
        let (release, gate) = mpsc::channel::<()>();
        let ctx = Context::default();
        let mut a = app(dir.path(), Adds::new().unwrap());
        a.open(&ctx);
        a.views.drops = crate::integration::spotify::Drops::with_classifier(move |_| {
            let _ = entered.send(());
            let _ = gate.recv_timeout(Duration::from_secs(5));
            string_extras::spotify::Holds {
                music: true,
                ..Default::default()
            }
        });
        assert!(a.views.drops.send(vec![song.clone()], false, &a.adds.input));
        sorting.recv_timeout(Duration::from_secs(2)).unwrap();
        // The sort ends well inside the time a close gives an add.
        let sorter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            let _ = release.send(());
        });
        let flushed = a.flush();
        sorter.join().unwrap();
        assert!(flushed, "{}", a.close_reason);
        let folder = storage::folder(dir.path(), true);
        assert_eq!(
            folder.open().unwrap().state.value.fields["files"],
            serde_json::json!([song])
        );
    }
}
