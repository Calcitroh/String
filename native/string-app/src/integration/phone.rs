use crate::{
    playback::{Session, Shared},
    storage::Storage,
};
use chrono::NaiveDateTime;
use egui::Context;
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::Instant,
};
use string_core::{
    data::{DataFolder, Profile},
    stats::{Listening, Track},
};
use string_extras::phone::{self, STARTUP_DELAY, SYNC_INTERVAL, SyncResult};

type Reply = Result<Option<SyncResult>, String>;

pub struct Phone {
    root: PathBuf,
    due: Instant,
    requested: bool,
    minimized: bool,
    enabled: Option<bool>,
    smoke_db: Option<PathBuf>,
    pending: Option<mpsc::Receiver<Reply>>,
    pub users: Vec<(String, String)>,
}

impl Phone {
    pub fn new(root: &Path, smoke: bool) -> Self {
        Self {
            root: root.into(),
            due: Instant::now() + STARTUP_DELAY,
            requested: false,
            minimized: false,
            enabled: None,
            smoke_db: smoke.then(|| root.join("navidrome.db")),
            pending: None,
            users: vec![],
        }
    }
    pub fn request(&mut self) {
        self.requested = true;
    }

    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    // Wait without the session lock: the writer may still be acquiring it.
    pub fn wait(&mut self, deadline: Instant) -> Option<String> {
        let result = match self
            .pending
            .as_ref()?
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Disconnected) => Err("Phone import worker stopped".into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Some("Phone listening is still being saved".into());
            }
        };
        self.pending = None;
        result
            .map(|r| {
                if let Some(r) = r {
                    self.users = r.users;
                }
            })
            .err()
    }

    fn due(&mut self, now: Instant, enabled: bool, minimized: bool) -> bool {
        if self.minimized && !minimized || self.enabled == Some(false) && enabled {
            self.request();
        }
        self.minimized = minimized;
        self.enabled = Some(enabled);
        if !enabled {
            self.requested = false;
            return false;
        }
        self.pending.is_none() && (self.requested || now >= self.due)
    }

    pub fn poll(
        &mut self,
        enabled: bool,
        minimized: bool,
        shared: &Shared,
        store: &Storage,
        ctx: &Context,
    ) -> Option<String> {
        let mut error = None;
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(result) => {
                    match result {
                        Ok(Some(r)) => self.users = r.users,
                        Ok(None) => self.requested = true,
                        Err(e) => error = Some(e),
                    }
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    error = Some("Phone import worker stopped".into());
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let now = Instant::now();
        if self.due(now, enabled, minimized) {
            self.requested = false;
            if now >= self.due {
                self.due = now + SYNC_INTERVAL;
            }
            let (tx, rx) = mpsc::channel();
            self.pending = Some(rx);
            let shared = shared.clone();
            let db = self.smoke_db.clone();
            let root = self.root.clone();
            store.with_profile(move |profile| {
                let db = db.unwrap_or_else(phone::database_path);
                let _ = tx.send(
                    import(profile, &shared, &root, |stage| phone::sync(stage, &db))
                        .map_err(|e| e.to_string()),
                );
            });
        }
        if enabled && !self.busy() {
            ctx.request_repaint_after(self.due.saturating_duration_since(now));
        }
        error
    }
}

pub(crate) struct Heard(Track, f64, NaiveDateTime);
impl Session {
    pub(crate) fn heard(&mut self, track: Track, seconds: f64, at: NaiveDateTime) {
        if self.listening.add(&track, seconds, at) {
            self.listening_dirty = true;
            if let Some(events) = &mut self.import_listens {
                events.push(Heard(track, seconds, at));
            }
        }
    }
}

fn replay(listening: &mut Listening, heard: &[Heard]) {
    for Heard(track, seconds, at) in heard {
        listening.add(track, *seconds, *at);
    }
}

fn import(
    profile: &mut Profile,
    shared: &Shared,
    root: &Path,
    sync: impl FnOnce(&mut Profile) -> Result<SyncResult, phone::Error>,
) -> Result<Option<SyncResult>, crate::Error> {
    // sync writes each page. Stage those writes until the selected account and the live stats agree.
    let temp = tempfile::Builder::new()
        .prefix("phone-import-")
        .tempdir_in(root)?;
    let mut stage = DataFolder::new(temp.path(), temp.path().join("backup")).open()?;
    let (mut before, generation, account) = {
        let mut s = shared.lock().unwrap();
        if s.setting("phoneStats") == false {
            return Ok(None);
        }
        for key in ["set", "phone", "edits"] {
            if let Some(value) = s.state.fields.get(key) {
                stage.state.value.fields.insert(key.into(), value.clone());
            }
        }
        stage.library.value.clone_from(&s.library);
        stage.listening.value.clone_from(&s.listening);
        s.import_listens = Some(Vec::new());
        (
            s.listening.clone(),
            s.generation,
            s.setting("phoneUser").clone(),
        )
    };

    let result = sync(&mut stage);
    let mut s = shared.lock().unwrap();
    let heard = s.import_listens.take().unwrap_or_default();
    if s.setting("phoneStats") == false
        || s.setting("phoneUser") != &account
        || s.generation != generation
    {
        return Ok(None);
    }
    let report = match result {
        Ok(report) => report,
        Err(e) => {
            let mut phone = s.field("phone").clone();
            if !phone.is_object() {
                phone = serde_json::json!({});
            }
            phone["error"] = e.to_string().into();
            phone["at"] = chrono::Local::now().timestamp_millis().into();
            s.set_field("phone", phone);
            return Err(e.into());
        }
    };
    if stage.listening.value.phone_import().map(|h| &h.after)
        != before.phone_import().map(|h| &h.after)
    {
        replay(&mut before, &heard);
        // A screen may have merged or replaced listening data during the import. Retry from its result.
        if serde_json::to_vec(&before)? != serde_json::to_vec(&s.listening)? {
            return Ok(None);
        }
        let mut imported = stage.listening.value;
        replay(&mut imported, &heard);
        profile.listening.value = imported;
        if let Some(e) = profile.listening.save()?.backup_error {
            s.notices.push_back(format!("Backup of listening: {e}"));
        }
        s.listening.clone_from(&profile.listening.value);
        s.listening_dirty = false;
    }
    // The listening cursor is durable before state.json can advertise it.
    s.set_field("phone", stage.state.value.fields["phone"].clone());
    if let Some(user) = stage.state.value.fields["set"].get("phoneUser") {
        s.set_setting("phoneUser", user.clone());
    }
    Ok(Some(report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use string_core::stats::PhoneImport;

    fn fixture() -> (tempfile::TempDir, DataFolder, Shared) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-runs/phone-checks");
        std::fs::create_dir_all(&root).unwrap();
        let dir = tempfile::tempdir_in(root).unwrap();
        let folder = DataFolder::new(dir.path(), dir.path().join("backup"));
        let session = Session::new(Default::default(), Default::default(), Default::default());
        (dir, folder, Arc::new(Mutex::new(session)))
    }

    fn page(stage: &mut Profile) -> Result<SyncResult, phone::Error> {
        let when = chrono::Local::now().naive_local();
        stage.listening.value.add_phone(
            &Track("p".into(), "Phone".into(), "Artist".into()),
            123.,
            when,
        );
        let mut cursor = PhoneImport::default();
        cursor.after.insert("one".into(), 1);
        cursor.plays = 1;
        stage.listening.value.set_phone_import(cursor);
        stage
            .state
            .value
            .fields
            .insert("phone".into(), serde_json::json!({"after":1,"plays":1}));
        Ok(SyncResult {
            added: 1,
            users: Vec::new(),
        })
    }

    #[test]
    fn import_keeps_desktop_listening_and_edits_made_while_the_database_is_read() {
        let (dir, folder, session) = fixture();
        let mut profile = folder.open().unwrap();
        let when = chrono::Local::now().naive_local();
        let r = import(&mut profile, &session, dir.path(), |stage| {
            let mut s = session
                .try_lock()
                .expect("Session must be available during the import");
            s.heard(
                Track("d".into(), "Desktop".into(), "Artist".into()),
                3.,
                when,
            );
            s.set_setting("volumeOnReturn", true.into());
            page(stage)
        })
        .unwrap()
        .unwrap();
        assert_eq!(r.added, 1);
        let s = session.lock().unwrap();
        assert_eq!(s.setting("volumeOnReturn"), true);
        let disk = folder.open().unwrap();
        let total = disk
            .listening
            .value
            .days()
            .values()
            .map(|d| d.s)
            .sum::<f64>();
        assert_eq!(total, 126.);
        assert_eq!(s.listening.phone_import().unwrap().plays, 1);
        assert_eq!(s.field("phone")["after"], 1);
        assert!(s.state_dirty);
    }

    #[test]
    fn changed_account_discards_the_staged_import() {
        let (dir, folder, session) = fixture();
        let mut profile = folder.open().unwrap();
        let result = import(&mut profile, &session, dir.path(), |stage| {
            session
                .lock()
                .unwrap()
                .set_setting("phoneUser", "two".into());
            page(stage)
        })
        .unwrap();
        assert!(result.is_none());
        assert!(folder.open().unwrap().listening.value.days().is_empty());
        assert!(session.lock().unwrap().field("phone").is_null());
    }

    #[test]
    fn refused_listening_write_preserves_the_external_file_and_cursor() {
        let (dir, folder, session) = fixture();
        let mut profile = folder.open().unwrap();
        let path = dir.path().join("listening.json");
        let other = b"external change";
        let result = import(&mut profile, &session, dir.path(), |stage| {
            std::fs::write(&path, other).unwrap();
            page(stage)
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(path).unwrap(), other);
        assert!(session.lock().unwrap().field("phone").is_null());
    }
    #[test]
    fn schedules_start_interval_restore_and_opt_in() {
        let mut p = Phone::new(Path::new("."), true);
        let start = p.due - STARTUP_DELAY;
        assert!(!p.due(start, true, false));
        assert!(p.due(start + STARTUP_DELAY, true, false));
        p.due += SYNC_INTERVAL;
        assert!(!p.due(start + STARTUP_DELAY, true, true));
        assert!(p.due(start + STARTUP_DELAY, true, false));
        assert!(!p.due(start + SYNC_INTERVAL + STARTUP_DELAY, false, false));
        assert!(p.due(start + SYNC_INTERVAL + STARTUP_DELAY, true, false));
    }
}
