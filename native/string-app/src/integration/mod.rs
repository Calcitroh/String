pub mod files;
pub mod helpers;
pub mod media;
pub mod phone;
pub mod screens;
pub mod smoke;
pub mod update_check;
pub mod updates;

#[cfg(test)]
mod tests {
    use crate::storage::{self, Storage};
    use std::{path::Path, sync::mpsc, time::Instant};

    #[test]
    fn close_keeps_a_slow_write_pending_then_flushes_the_latest_edit() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-runs/close-checks");
        std::fs::create_dir_all(&base).unwrap();
        let dir = tempfile::tempdir_in(base).unwrap();
        let folder = storage::folder(dir.path(), true);
        let (mut writer, mut session) = Storage::open(&folder, &egui::Context::default()).unwrap();
        let (release, hold) = mpsc::channel();
        writer.with_profile(move |_| {
            let _ = hold.recv();
        });
        session.set_field("route", "songs".into());
        writer.schedule(&mut session);
        session.set_field("route", "liked".into());
        assert!(writer.flush_until(&mut session, Instant::now()).failed());
        assert!(session.state_dirty);
        release.send(()).unwrap();
        assert!(!writer.flush(&mut session).failed());
        assert_eq!(folder.open().unwrap().state.value.fields["route"], "liked");
    }
}
