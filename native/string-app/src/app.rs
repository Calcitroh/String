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
    views: Views,
    covers: CoverCache,
    viz: Viz,
    scan: Scanner,
    smoke: Option<Smoke>,
    exit: Arc<AtomicI32>,
    closing: bool,
    /// Asked to close while songs were on their way to the Recycle Bin: String closes once they're merged.
    close_after_recycle: bool,
    minimized: bool,
    last: Instant,
    save_due: Option<Instant>,
    transport_until: Instant,
}
impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        args: Args,
        mut platform: Platform,
        exit: Arc<AtomicI32>,
    ) -> Result<Self, Error> {
        let ctx = &cc.egui_ctx;
        platform.attach(cc)?;
        theme::install(ctx);
        let now = Instant::now();
        let lyrics = crate::screens::lyrics::LyricsView::new(&args.root, args.smoke.is_some());
        let mut a = Self {
            smoke: args.smoke.clone().map(Smoke::new),
            args,
            runtime: None,
            error: String::new(),
            platform,
            views: Views {
                lyrics: Some(lyrics),
                ..Default::default()
            },
            covers: CoverCache::new(ctx),
            viz: Default::default(),
            scan: Default::default(),
            exit,
            closing: false,
            close_after_recycle: false,
            minimized: false,
            last: now,
            save_due: None,
            transport_until: now,
        };
        a.open(ctx);
        Ok(a)
    }
    fn open(&mut self, ctx: &Context) {
        let folder = storage::folder(&self.args.root, self.args.explicit);
        match Storage::open(&folder, ctx) {
            Ok((store, s)) => {
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
    fn flush(&mut self) -> bool {
        self.platform.save_placement();
        let deadline = Instant::now() + Duration::from_secs(10);
        if let Some(error) = self.platform.phone.wait(deadline) {
            self.platform.log.write("phone listening", error);
            if self.platform.phone.busy() {
                return false;
            }
        }
        let Some(rt) = self.runtime.as_mut() else {
            return true;
        };
        let mut s = rt.session.lock().unwrap();
        s.tick(rt.player.status(), true);
        let r = rt.store.flush_until(&mut s, deadline);
        for msg in &r.errors {
            self.platform.log.write("save", msg);
        }
        !r.failed()
    }
    fn finish(&mut self, ctx: &Context, mut err: Option<String>) {
        if !self.flush() && err.is_none() {
            err = Some("Final profile save failed.".into());
        }
        if let Some(sm) = &self.smoke
            && let Err(e) = sm.report(err.as_deref())
        {
            err = Some(e.to_string());
        }
        if let Some(e) = &err {
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
            // songs still moving to the Recycle Bin: wait for them, so the copies kept take over their likes,
            // plays and playlist places before the last save
            if crate::screens::dupes::recycling() {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.close_after_recycle = true;
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
        s.tick(st, false);
        // Songs moved to the Recycle Bin are merged into the copies kept here, which runs while String is minimised
        // too; a close that waited for them goes on once they are. Paths from `--add` are added here too, so a
        // helper's songs come in while String is minimised.
        let added = self.platform.take_added();
        let recycling = crate::screens::dupes::recycling();
        if recycling || !added.is_empty() {
            let tint = Tint::parse(s.setting("tint").as_str());
            let mono = s.setting("artMono").as_bool().unwrap_or(true);
            let mut v = View {
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
            if !added.is_empty() {
                crate::integration::helpers::add(&mut v, added);
            }
            if recycling {
                crate::screens::dupes::settle(&mut v);
            }
        }
        if self.close_after_recycle {
            if crate::screens::dupes::recycling() {
                ctx.request_repaint_after(Duration::from_millis(50));
            } else {
                self.close_after_recycle = false;
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
        if let Some(r) = rt.store.poll(&mut s) {
            for msg in r.errors {
                self.platform.log.write("save", &msg);
                self.views.toast.set(msg);
            }
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
            self.views.toast.set(msg);
        }
        if !self.platform.phone.busy() && (s.state_dirty || s.listening_dirty || s.library_dirty) {
            let due = *self.save_due.get_or_insert_with(|| {
                Instant::now() + Duration::from_millis(if st.playing { 5000 } else { 650 })
            });
            if Instant::now() >= due {
                rt.store.schedule(&mut s);
                self.save_due = None;
            } else {
                ctx.request_repaint_after(due.saturating_duration_since(Instant::now()));
            }
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
        if self.views.close_failed
            && let Some(quit) = ui::confirm(
                &ctx,
                "Your latest changes aren’t saved",
                "Keep String open to retry saving, or close and discard the unsaved changes.",
                "Close anyway",
            )
        {
            self.views.close_failed = false;
            if quit {
                self.closing = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
                return;
            }
        }
        let rt = self.runtime.as_mut().unwrap();
        self.covers.poll(&ctx);
        let paths = self.platform.picked_paths().unwrap_or_else(|e| {
            self.views.toast.set(e.to_string());
            Vec::new()
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
        for path in paths {
            v.add_path(path);
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
        if !self.closing {
            self.flush();
        }
    }
}
