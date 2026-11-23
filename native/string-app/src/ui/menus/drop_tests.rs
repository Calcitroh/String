use super::*;
use crate::{
    add_work::{Adds, Location},
    scan::Scanner,
};
use std::{
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
struct File(PathBuf);
impl egui::DroppedFile for File {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        panic!("The drop handler should only need the path")
    }
}

fn drop_frame(ctx: &Context, adds: &Adds, toast: &mut ui::Toast, paths: &[PathBuf]) {
    let input = egui::RawInput {
        dropped_files: paths
            .iter()
            .cloned()
            .map(|p| Arc::new(File(p)) as egui::DroppedFileHandle)
            .collect(),
        ..Default::default()
    };
    let _ = ctx.run_ui(input, |ui| drops(ui.ctx(), Tint::Mono, &adds.input, toast));
}

fn finish(adds: &mut Adds, s: &mut Session, scan: &mut Scanner, toast: &mut ui::Toast) {
    let until = Instant::now() + Duration::from_secs(5);
    while adds.pending() {
        adds.poll(s, scan, toast);
        assert!(Instant::now() < until, "Drop did not finish");
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn hover_and_drop_keep_processing_frames_while_the_classifier_is_blocked() {
    let temp = tempfile::tempdir().unwrap();
    // Only the injected classifier knows these paths. An on-frame stat would reject them.
    let paths = ["disc.txt", "one.FLAC", "two.wav", "notes.txt"].map(|n| temp.path().join(n));
    let (entered, started) = mpsc::channel();
    let (resume, wait) = mpsc::channel();
    let mut adds = Adds::with_classifier(move |paths| {
        entered
            .send((thread::current().id(), paths.clone()))
            .unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        paths
            .into_iter()
            .map(|path| Location {
                directory: path.ends_with("disc.txt"),
                path,
            })
            .collect()
    })
    .unwrap();
    let ctx = Context::default();
    crate::theme::install(&ctx);
    let mut toast = ui::Toast::default();
    let hover = egui::RawInput {
        hovered_files: vec![egui::HoveredFile {
            path: Some(paths[0].clone()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let out = ctx.run_ui(hover, |ui| {
        drops(ui.ctx(), Tint::Mono, &adds.input, &mut toast)
    });
    assert!(out.shapes.iter().any(|s| matches!(&s.shape, Shape::Text(t)
        if t.galley.job.text.eq_ignore_ascii_case("Drop folders or songs to add them"))));
    assert!(
        !adds.pending(),
        "Hover alone should never enqueue a disk check"
    );
    drop_frame(&ctx, &adds, &mut toast, &paths);
    let (worker, received) = started
        .recv_timeout(Duration::from_secs(2))
        .expect("The UI should forward every path, including unavailable ones");
    assert_ne!(thread::current().id(), worker);
    assert_eq!(received, paths);

    let mut session = Session::new(Default::default(), Default::default(), Default::default());
    let mut scan = Scanner::default();
    adds.poll(&mut session, &mut scan, &mut toast);
    drop_frame(&ctx, &adds, &mut toast, &[]);
    assert!(adds.pending());
    assert!(toast.current.is_none());
    assert!(!scan.requested);
    resume.send(()).unwrap();
    finish(&mut adds, &mut session, &mut scan, &mut toast);
    assert_eq!(*session.field("roots"), json!([paths[0]]));
    assert_eq!(*session.field("files"), json!([paths[1], paths[2]]));
    assert_eq!(
        toast.current.unwrap().0,
        "Folder added: reading its songs · 2 songs added"
    );
    assert!(scan.requested);
}

#[test]
fn rejected_and_duplicate_drops_keep_their_messages() {
    let temp = tempfile::tempdir().unwrap();
    let note = temp.path().join("notes.txt");
    let missing = temp.path().join("missing.wav");
    let song = temp.path().join("one.mp3");
    std::fs::write(&note, []).unwrap();
    std::fs::write(&song, []).unwrap();
    let ctx = Context::default();
    let mut adds = Adds::new().unwrap();
    let mut session = Session::new(Default::default(), Default::default(), Default::default());
    let mut scan = Scanner::default();
    let mut toast = ui::Toast::default();
    for (paths, message) in [
        (
            vec![note.clone(), missing.clone()],
            "Those aren't songs String can play",
        ),
        (vec![missing.clone()], "Those aren't songs String can play"),
        (vec![song.clone(), note.clone(), missing], "1 song added"),
        (
            vec![song.clone(), note],
            "Those are already in your library",
        ),
    ] {
        drop_frame(&ctx, &adds, &mut toast, &paths);
        finish(&mut adds, &mut session, &mut scan, &mut toast);
        assert_eq!(toast.current.take().unwrap().0, message);
    }
    assert_eq!(*session.field("files"), json!([song]));
    adds.set_closed(true);
    drop_frame(&ctx, &adds, &mut toast, &[song]);
    assert_eq!(
        toast.current.unwrap().0,
        "String could not take more songs."
    );
}

#[test]
fn review_partial_drop_reports_added_and_skipped_together() {
    let temp = tempfile::tempdir().unwrap();
    let song = temp.path().join("a.wav");
    let second = temp.path().join("b.wav");
    std::fs::write(&song, []).unwrap();
    std::fs::write(&second, []).unwrap();
    let mut adds = Adds::new().unwrap();
    use std::os::windows::ffi::OsStrExt;
    let room = (song.as_os_str().encode_wide().count() + 1) * 2;
    let taken = string_platform::instance::Instance::MAX_ADD_BYTES - room;
    assert!(
        adds.input
            .submit(None, vec![temp.path().join("missing")], taken)
    );
    let mut toast = ui::Toast::default();
    drop_frame(
        &Context::default(),
        &adds,
        &mut toast,
        &[song.clone(), second],
    );
    let immediate = toast.current.take();
    let mut session = Session::new(Default::default(), Default::default(), Default::default());
    finish(&mut adds, &mut session, &mut Scanner::default(), &mut toast);
    assert_eq!(*session.field("files"), json!([song]));
    assert_eq!(toast.current.unwrap().0, "1 song added · 1 skipped");
    assert!(
        immediate.is_none(),
        "a partially admitted drop has one completion message"
    );
}
