//! The Duplicates page: songs the library has more than once. For each, play the copies, pick the one to keep and
//! tick the ones to drop, then hide those in String or move them to the Recycle Bin. Either way the kept copy takes
//! over their likes, plays, loops, listening time and playlist places.
mod bin;
mod files;
use super::settings::plural;
use crate::{
    playback::Session,
    services::{self, Event, Task},
    theme::*,
    ui::{
        self,
        panel::{self, Btn, Kind},
    },
    view::View,
};
use chrono::Local;
use egui::{
    Align, Context, Layout, Rect, Sense, Stroke, StrokeKind, Ui, pos2, text::LayoutJob, vec2,
};
use files::{Checked, Pair, Stamp};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{LazyLock, Mutex, MutexGuard},
};
use string_core::{
    library::{Library, Track},
    state::State as Saved,
};
use string_extras::{
    duplicates::{
        CopyToMerge, Disposition, DuplicateGroup, duplicates, forget_not_same, mark_not_same,
        merge_copy, still_copies, unhide_all,
    },
    edit::display_names,
};

struct Copy {
    id: String,
    path: String,
    name: String,
    dir: String,
    /// "MP3 · 161 kbps · 0.5 MB · 0:24"
    sound: String,
    title: String,
    artist: String,
    duration: f64,
    size: u64,
    stamp: Option<Stamp>,
}
impl Copy {
    fn unchanged(&self, old: &Self) -> bool {
        self.stamp.is_some()
            && self.stamp == old.stamp
            && self.path == old.path
            && self.title == old.title
            && self.artist == old.artist
            && self.duration == old.duration
    }
}
struct Group {
    found: DuplicateGroup,
    copies: Vec<Copy>,
    artists: String,
}
/// What the list was made from: a change to any of it means looking again.
type Basis = (u64, u64);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Intent {
    Hide,
    Ask,
    Recycle,
}

type FileCheck = Result<Checked, (Problem, HashSet<String>)>;

struct Problem {
    ids: Vec<String>,
    reason: String,
}
#[derive(Default)]
struct Progress {
    next: usize,
    moved: usize,
    failed: Vec<String>,
}
struct Recycling {
    active: CopyToMerge,
    original_drop: Track,
    progress: Progress,
    task: Task<(Checked, Result<(), String>)>,
}
#[derive(Default)]
pub struct State {
    groups: Vec<Group>,
    basis: Option<Basis>,
    look_number: u64,
    looking: Option<(Basis, Task<Vec<Group>>)>,
    keep: Vec<String>,
    drop: HashSet<String>,
    pub ask: bool,
    ctx: Option<Context>,
    checking: Option<(Intent, Basis, Task<FileCheck>)>,
    unticked: HashSet<String>,
    reread: HashSet<String>,
    awaiting_scan: bool,
    recycling: Option<Recycling>,
    skipped: Vec<String>,
}
static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);
pub fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap()
}
fn copies(n: usize) -> String {
    if n == 1 {
        "1 copy".into()
    } else {
        format!("{} copies", super::settings::num(n as u64))
    }
}
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.)
}
fn count(v: &View<'_>, field: &str) -> usize {
    v.s.field(field).as_object().map_or(0, |m| m.len())
}
fn basis(s: &Session) -> Basis {
    (s.generation, s.plays_revision)
}
/// "N songs hidden from String · show them again", for this page and the Library page.
pub fn hidden_note(ui: &mut Ui, v: &mut View<'_>) {
    let n = count(v, "hidden");
    if n == 0 {
        return;
    }
    ui.add_space(8.);
    if panel::note_link(
        ui,
        &format!("{} hidden from String", plural(n, "song")),
        "show them again",
    ) {
        unhide_all(&mut v.s.state);
        v.s.rebuild();
        v.s.queue_changed(v.player);
        v.views.toast.set("Hidden songs are back");
    }
}
fn apart_note(ui: &mut Ui, v: &mut View<'_>) {
    let n = count(v, "notDupes");
    if n == 0 {
        return;
    }
    ui.add_space(8.);
    let pairs = if n == 1 {
        "1 pair".into()
    } else {
        format!("{n} pairs")
    };
    if panel::note_link(
        ui,
        &format!("{pairs} you marked as not the same"),
        "list them again",
    ) {
        forget_not_same(&mut v.s.state);
        v.s.generation += 1;
        v.s.state_dirty = true;
    }
}
impl State {
    fn refresh(&mut self, ctx: &Context, s: &Session) {
        let current = basis(s);
        if self.awaiting_scan || self.checking.is_some() || self.recycling.is_some() {
            return;
        }
        if self.ask && self.basis != Some(current) {
            self.refuse(self.changed());
        }
        if let Some((_, task)) = &self.looking
            && let Some(groups) = task.poll()
        {
            let (revision, _) = self.looking.take().unwrap();
            if revision == current {
                self.basis = Some(revision);
                self.take(groups);
            }
        }
        if self.looking.is_none() && self.basis != Some(current) {
            self.look(ctx, s);
        }
    }
    fn look(&mut self, ctx: &Context, s: &Session) {
        self.ctx = Some(ctx.clone());
        self.look_number += 1;
        self.basis = None;
        let (lib, state) = (s.library.clone(), s.state.clone());
        self.looking = Some((
            basis(s),
            Task::run(ctx, move |_| Self::groups(&lib, &state)),
        ));
    }
    /// The copies surely the same as the kept one are ticked; "maybe" ones wait for the person.
    fn tick(&mut self, gi: usize) {
        let g = &self.groups[gi];
        let keep = g
            .copies
            .iter()
            .position(|c| c.id == self.keep[gi])
            .unwrap_or(0);
        for c in &g.copies {
            self.drop.remove(&c.id);
        }
        self.drop.extend(
            g.found
                .suggested_drops(keep)
                .filter(|id| !self.unticked.contains(*id))
                .map(str::to_owned),
        );
    }
    fn choose_keep(&mut self, group: usize, id: String) {
        // Even if the group disappears and later returns, this file was explicitly kept.
        self.unticked.insert(id.clone());
        self.keep[group] = id;
        self.tick(group);
    }
    fn choose_drop(&mut self, id: String, selected: bool) {
        if selected {
            self.unticked.remove(&id);
            self.drop.insert(id);
        } else {
            self.drop.remove(&id);
            self.unticked.insert(id);
        }
    }
    fn groups(lib: &Library, state: &Saved) -> Vec<Group> {
        let tracks: HashMap<_, _> = lib.tracks.iter().map(|t| (t.id(), t)).collect();
        duplicates(lib, state)
            .into_iter()
            .filter_map(|found| {
                let copies = found
                    .ids
                    .iter()
                    .map(|id| {
                        let t = tracks.get(id)?;
                        let path = std::path::Path::new(&t.path);
                        let names = display_names(state, t);
                        let kbps = if t.tags.duration > 0. && t.size > 0 {
                            (t.size as f64 * 8. / t.tags.duration / 1000.).round() as u64
                        } else {
                            0
                        };
                        let format = if t.tags.format.is_empty() {
                            "?".into()
                        } else {
                            t.tags.format.to_uppercase()
                        };
                        Some(Copy {
                            id: id.clone(),
                            path: t.path.clone(),
                            name: path.file_name()?.to_string_lossy().into_owned(),
                            dir: path.parent()?.to_string_lossy().into_owned(),
                            sound: format!(
                                "{format}{} · {} · {}",
                                if kbps > 0 {
                                    format!(" · {kbps} kbps")
                                } else {
                                    String::new()
                                },
                                megabytes(t.size),
                                time(t.tags.duration)
                            ),
                            title: names.title,
                            artist: names.artist,
                            duration: t.tags.duration,
                            size: t.size,
                            stamp: Stamp::seen(t),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                let mut artists: Vec<&str> = vec![];
                for c in &copies {
                    if !c.artist.is_empty() && !artists.contains(&c.artist.as_str()) {
                        artists.push(&c.artist);
                    }
                }
                Some(Group {
                    artists: artists.join(" / "),
                    found,
                    copies,
                })
            })
            .collect()
    }
    fn take(&mut self, groups: Vec<Group>) {
        let previous = std::mem::replace(&mut self.groups, groups);
        let keepers = std::mem::take(&mut self.keep);
        let drops = std::mem::take(&mut self.drop);
        let mut invalidated = false;
        let mut old = HashMap::new();
        for (group, keep) in previous.iter().zip(&keepers) {
            for c in &group.copies {
                old.insert(&c.id, (c, keep));
            }
        }
        for gi in 0..self.groups.len() {
            let group = &self.groups[gi];
            let old_keepers: HashSet<_> = group
                .copies
                .iter()
                .filter_map(|c| old.get(&c.id).map(|(_, k)| *k))
                .collect();
            let keeper = group.copies.iter().find(|c| {
                old_keepers.contains(&c.id)
                    && old
                        .get(&c.id)
                        .is_some_and(|(before, _)| c.unchanged(before))
            });
            let reviewed = old_keepers.len() == 1 && keeper.is_some();
            let keep = &keeper.unwrap_or(&group.copies[0]).id;
            self.keep.push(keep.clone());
            self.tick(gi);
            for c in &self.groups[gi].copies {
                if let Some((before, was_keep)) = old.get(&c.id) {
                    self.drop.remove(&c.id);
                    if reviewed
                        && **was_keep == self.keep[gi]
                        && c.unchanged(before)
                        && drops.contains(&c.id)
                    {
                        self.drop.insert(c.id.clone());
                    } else if drops.contains(&c.id) {
                        invalidated = true;
                    }
                } else if !old_keepers.is_empty() && !reviewed {
                    self.drop.remove(&c.id);
                }
            }
        }
        self.drop.retain(|id| !self.unticked.contains(id));
        if invalidated {
            services::notice("A reviewed pair changed. Its removal was unticked.");
        }
    }
    fn changed(&self) -> Problem {
        Problem {
            ids: self.drop.iter().cloned().collect(),
            reason: "The song list changed. Review the copies and tick them again.".into(),
        }
    }
    fn refuse(&mut self, problem: Problem) {
        for id in problem.ids {
            self.drop.remove(&id);
            self.unticked.insert(id);
        }
        self.ask = false;
        self.basis = None;
        services::notice(problem.reason);
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }
    fn plan(&self, s: &Session, intent: Intent) -> Result<Vec<Pair>, Problem> {
        if self.basis != Some(basis(s)) {
            return Err(self.changed());
        }
        let tracks: HashMap<_, _> = s.library.tracks.iter().map(|t| (t.id(), t)).collect();
        let mut plan = vec![];
        let mut refused = vec![];
        let mut reason = None;
        for (g, keep) in self.groups.iter().zip(&self.keep) {
            let Some(k) = g.copies.iter().find(|c| &c.id == keep) else {
                return Err(self.changed());
            };
            for d in g.copies.iter().filter(|c| self.drop.contains(&c.id)) {
                let pair = tracks
                    .get(keep)
                    .zip(tracks.get(&d.id))
                    .filter(|(k, d)| still_copies(&s.state, k, d))
                    .and_then(|(k, d)| CopyToMerge::new(k, d));
                let Some(copy) = pair else {
                    refused.push(d.id.clone());
                    reason.get_or_insert_with(|| {
                        format!(
                            "{} no longer matches the copy to keep. Nothing was merged.",
                            d.name
                        )
                    });
                    continue;
                };
                if intent != Intent::Hide
                    && !crate::integration::files::recyclable_song(std::path::Path::new(&copy.path))
                {
                    refused.push(d.id.clone());
                    reason.get_or_insert_with(|| {
                        format!(
                            "Only audio files can go to the Recycle Bin. {} was left in place.",
                            d.name
                        )
                    });
                    continue;
                }
                plan.push(Pair {
                    copy,
                    keep_path: k.path.clone(),
                    keep: k.stamp,
                    drop: d.stamp,
                });
            }
        }
        if let Some(reason) = reason {
            return Err(Problem {
                ids: refused,
                reason,
            });
        }
        if plan.len() != self.drop.len() {
            return Err(self.changed());
        }
        Ok(plan)
    }
    fn request(&mut self, s: &Session, intent: Intent) {
        if self.checking.is_some() || self.recycling.is_some() {
            return;
        }
        match self.plan(s, intent) {
            Ok(plan) if !plan.is_empty() => {
                let ctx = self.ctx.as_ref().expect("a displayed list has a context");
                self.checking = Some((
                    intent,
                    basis(s),
                    Task::run(ctx, move |_| files::check(plan)),
                ));
            }
            Ok(_) => {}
            Err(problem) => self.refuse(problem),
        }
    }
    fn checked(&mut self, s: &Session) -> Option<(Intent, Checked)> {
        let result = self.checking.as_ref()?.2.poll()?;
        let (intent, revision, _) = self.checking.take()?;
        match result {
            Ok(_) if revision != basis(s) => {
                self.refuse(self.changed());
                None
            }
            Ok(checked) => Some((intent, checked)),
            Err((problem, paths)) => {
                self.reread.extend(paths);
                self.refuse(problem);
                None
            }
        }
    }
    fn ticked(&self) -> (usize, u64) {
        let mut total = (0, 0);
        for c in self.groups.iter().flat_map(|g| &g.copies) {
            if self.drop.contains(&c.id) {
                total = (total.0 + 1, total.1 + c.size);
            }
        }
        total
    }
    /// The list matches the library and nothing is under way.
    pub fn settled(&self, v: &View<'_>) -> bool {
        self.looking.is_none()
            && self.checking.is_none()
            && self.recycling.is_none()
            && self.basis == Some(basis(v.s))
    }
    fn merge_hidden(s: &mut Session, plan: &[CopyToMerge]) {
        let now = Local::now().timestamp_millis();
        let svc = services::get();
        for copy in plan {
            merge_copy(&mut s.state, &mut s.listening, copy, Disposition::Hide, now);
            svc.event(Event::Merged {
                from: &copy.drop_id,
                to: &copy.keep_id,
            });
        }
        s.listening_dirty = true;
        s.rebuild();
    }
    /// Resolve one pair immediately before handing its path to the shell worker.
    fn matching_drop<'s>(
        &self,
        s: &'s Session,
        copy: &CopyToMerge,
        removed: Option<&'s Track>,
    ) -> Option<&'s Track> {
        let track = |id: &str| {
            let row = self
                .groups
                .iter()
                .flat_map(|g| &g.copies)
                .find(|c| c.id == id)?;
            let t = s
                .library
                .tracks
                .iter()
                .find(|t| t.path == row.path)
                .or_else(|| removed.filter(|t| t.path == row.path))?;
            let names = display_names(&s.state, t);
            (t.size == row.size
                && t.tags.duration == row.duration
                && names.title == row.title
                && names.artist == row.artist)
                .then_some(t)
        };
        let (k, d) = (track(&copy.keep_id)?, track(&copy.drop_id)?);
        still_copies(&s.state, k, d).then_some(d)
    }
    fn next_pair(
        &mut self,
        checked: &Checked,
        progress: &mut Progress,
        s: &Session,
    ) -> Option<(usize, Track)> {
        while let Some(copy) = checked.plan.get(progress.next) {
            let at = progress.next;
            progress.next += 1;
            if let Some(drop) = self.matching_drop(s, copy, None) {
                return Some((at, drop.clone()));
            }
            self.choose_drop(copy.drop_id.clone(), false);
            progress.failed.push(format!(
                "{} no longer matches the reviewed pair and was left in place",
                copy.path
            ));
        }
        None
    }
    fn dispatch(&mut self, mut checked: Checked, mut progress: Progress, v: &mut View<'_>) {
        if let Some((at, original_drop)) = self.next_pair(&checked, &mut progress, v.s) {
            v.scan.recycling = true;
            self.recycling = Some(Recycling {
                active: checked.plan[at].clone(),
                original_drop,
                progress,
                task: Task::run(self.ctx.as_ref().unwrap(), move |s| {
                    let result = checked.recycle(at, |p| s.recycle(p));
                    (checked, result.map_err(|e| e.to_string()))
                }),
            });
            return;
        }
        v.scan.recycling = false;
        v.scan.requested |= progress.moved > 0 || !progress.failed.is_empty();
        let moved = copies(progress.moved);
        let message = match progress.failed.first() {
            Some(first) => format!(
                "{moved} confirmed in the Recycle Bin. {} to review. {first}",
                copies(progress.failed.len())
            ),
            None => format!("{moved} moved to the Recycle Bin"),
        };
        self.skipped = progress.failed;
        v.views.toast.set(message);
    }
    fn finish_move(
        &mut self,
        copy: &CopyToMerge,
        original_drop: &Track,
        result: Result<(), String>,
        progress: &mut Progress,
        v: &mut View<'_>,
    ) {
        if let Err(error) = result {
            self.choose_drop(copy.drop_id.clone(), false);
            v.scan.reread_files(HashSet::from([copy.path.clone()]));
            progress.failed.push(format!("{}: {error}", copy.path));
            return;
        }
        // The keeper is still guarded and pair edits and scan application are serialized with
        // this one move. History belongs to the checked IDs even if a scan saw the removal.
        progress.moved += 1;
        v.s.removals += 1;
        if self.matching_drop(v.s, copy, Some(original_drop)).is_none() {
            self.choose_drop(copy.drop_id.clone(), false);
            progress.failed.push(format!(
                "{} moved, but the pair changed. Its history was left in place.",
                copy.path
            ));
            v.scan.reread_files(HashSet::from([copy.path.clone()]));
            return;
        }
        let swap = merge_copy(
            &mut v.s.state,
            &mut v.s.listening,
            copy,
            Disposition::Recycled,
            Local::now().timestamp_millis(),
        )
        .is_some();
        let service = services::get();
        service.event(Event::Merged {
            from: &copy.drop_id,
            to: &copy.keep_id,
        });
        service.event(Event::Removed(std::slice::from_ref(&copy.drop_id)));
        v.s.library.tracks.retain(|t| t.id() != copy.drop_id);
        v.s.library_dirty = true;
        v.s.listening_dirty = true;
        v.s.rebuild();
        v.s.queue_changed(v.player);
        if swap {
            v.s.load_current(v.player, v.st.playing, v.st.position);
        }
    }
}
/// Once the files are in the Recycle Bin, the copies kept take over their likes, plays and playlist places, whichever
/// page is open by then (the shell calls this every frame), so leaving Duplicates never leaves them on files that are
/// gone. Closing String waits while `recycling()`.
pub fn settle(v: &mut View<'_>) {
    let mut st = state();
    if let Some((intent, checked)) = st.checked(v.s) {
        match intent {
            Intent::Ask => st.ask = true,
            Intent::Recycle => st.dispatch(checked, Progress::default(), v),
            Intent::Hide => {
                State::merge_hidden(v.s, &checked.plan);
                v.s.queue_changed(v.player);
                v.views.toast.set(format!(
                    "{} hidden from String: the files are still there",
                    copies(checked.plan.len())
                ));
            }
        }
    }
    if !st.reread.is_empty() {
        v.scan.reread_files(std::mem::take(&mut st.reread));
        st.awaiting_scan = true;
    } else if st.awaiting_scan && !v.scan.requested && !v.scan.running() {
        st.awaiting_scan = false;
    }
    if let Some(job) = &st.recycling
        && let Some((checked, result)) = job.task.poll()
    {
        let mut job = st.recycling.take().unwrap();
        st.finish_move(
            &job.active,
            &job.original_drop,
            result,
            &mut job.progress,
            v,
        );
        st.dispatch(checked, job.progress, v);
    }
}
/// Edit info and Swap names can still change other songs, including pending pairs.
pub(crate) fn can_edit(id: &str) -> bool {
    let st = state();
    let busy = st
        .recycling
        .as_ref()
        .is_some_and(|j| j.active.keep_id == id || j.active.drop_id == id);
    if busy {
        services::notice("This copy is being moved. Try the edit again when the move finishes.");
    }
    !busy
}
pub fn recycling() -> bool {
    let st = state();
    st.recycling.is_some() || st.checking.is_some()
}
pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    settle(v);
    let mut st = state();
    let st = &mut *st;
    let tint = v.tint;
    st.refresh(ui.ctx(), v.s);
    let mut again = false;
    panel::page_title(ui, "Duplicates", "Songs you have more than once", |ui| {
        ui.add_space(2.);
        again = Btn::new("Look again", tint)
            .icon("reload")
            .sm()
            .enabled(st.settled(v) && !st.ask)
            .show(ui)
            .clicked();
    });
    if again {
        st.look(ui.ctx(), v.s);
    }
    if !st.skipped.is_empty() {
        ui.collapsing(
            format!("{} to review from the last move", copies(st.skipped.len())),
            |ui| {
                egui::ScrollArea::vertical()
                    .max_height(140.)
                    .show(ui, |ui| {
                        for reason in &st.skipped {
                            ui.label(reason);
                        }
                    });
            },
        );
    }
    if st.ask {
        let (n, bytes) = st.ticked();
        if let Some(yes) = panel::ask(
            ui.ctx(),
            &format!("Move {} to the Recycle Bin?", copies(n)),
            &format!(
                "{} of song files. You can put them back from the Recycle Bin; the copies you keep take over their likes, plays and playlist places.",
                megabytes(bytes)
            ),
            "Move to the Recycle Bin",
            true,
            tint,
        ) {
            st.ask = false;
            if yes {
                st.request(v.s, Intent::Recycle);
            }
        }
    }
    if st.basis.is_none() {
        panel::empty_note(ui, "Looking through your songs");
        return;
    }
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.set_width(ui.available_width() - 12.);
            if st.groups.is_empty() {
                panel::empty_note(ui, "No duplicates: every song is here once");
            } else {
                // A release from a mouse press on the previous list cannot activate this one.
                ui.push_id(("duplicate actions", st.look_number), |ui| {
                    summary(ui, v, st)
                });
                ui.add_space(20.);
                let mut apart = None;
                for gi in 0..st.groups.len() {
                    if ui
                        .add_enabled_ui(st.settled(v) && !st.ask, |ui| group(ui, v, st, gi))
                        .inner
                    {
                        apart = Some(gi);
                    }
                    ui.add_space(16.);
                }
                if let Some(gi) = apart {
                    mark_not_same(
                        &mut v.s.state,
                        &st.groups[gi].found,
                        Local::now().timestamp_millis(),
                    );
                    v.s.generation += 1;
                    v.s.state_dirty = true;
                }
            }
            hidden_note(ui, v);
            apart_note(ui, v);
        });
}
fn summary(ui: &mut Ui, v: &mut View<'_>, st: &mut State) {
    let tint = v.tint;
    let groups = st.groups.len();
    let extra: usize = st.groups.iter().map(|g| g.copies.len() - 1).sum();
    let maybes = st.groups.iter().filter(|g| !g.found.sure).count();
    let (ticked, bytes) = st.ticked();
    let busy = !st.settled(v) || st.ask;
    panel::panel(ui, 0., |ui| {
        let big = panel::line(ui, &super::settings::num(groups as u64), "HUD", 34., 0.02);
        let words = panel::line(
            ui,
            &format!(
                "{} {} TOO MANY",
                if groups == 1 {
                    "SONG HAS"
                } else {
                    "SONGS HAVE"
                },
                copies(extra).to_uppercase()
            ),
            "HUD",
            12.,
            0.14,
        );
        let (r, _) = ui.allocate_exact_size(
            vec2(big.size().x + 12. + words.size().x, big.size().y),
            Sense::hover(),
        );
        let p = ui.painter();
        p.galley(
            pos2(
                r.left() + big.size().x + 12.,
                r.bottom() - words.size().y - 7.,
            ),
            words,
            SECONDARY,
        );
        p.galley(r.min, big, TEXT);
        ui.add_space(6.);
        let mut job = LayoutJob::default();
        job.wrap.max_width = ui.available_width().min(760.);
        let plain = egui::TextFormat::simple(font("UI", 13.), MUTED);
        job.append(
            "For each song, String picks the copy to keep: the best sound, then the one in the folder with most of your songs. Copies that are surely the same song are ticked.",
            0.,
            plain.clone(),
        );
        if maybes > 0 {
            job.append(
                &if maybes == 1 {
                    " One song is only ".to_owned()
                } else {
                    format!(" {maybes} songs are only ")
                },
                0.,
                plain.clone(),
            );
            job.append(
                "maybe",
                0.,
                egui::TextFormat {
                    color: TEXT,
                    underline: Stroke::new(1., HAIR3),
                    ..plain.clone()
                },
            );
            job.append(
                " the same: the same name and length, but another credit or version. Play them to compare: tick what you want gone, or press Not the same and they stop being listed.",
                0.,
                plain,
            );
        }
        ui.allocate_ui(vec2(job.wrap.max_width, 0.), |ui| ui.label(job));
        ui.add_space(12.);
        let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.), Sense::hover());
        ui.painter().rect_filled(line, 0, HAIR);
        ui.add_space(10.);
        ui.horizontal(|ui| {
            let note = if ticked > 0 {
                format!("{} ticked · {}", copies(ticked), megabytes(bytes))
            } else {
                "nothing ticked".into()
            };
            ui.label(mono(&note, 10.5).color(FAINT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.;
                if Btn::new("Move to the Recycle Bin", tint)
                    .icon("trash")
                    .kind(Kind::Danger)
                    .sm()
                    .enabled(ticked > 0 && !busy)
                    .show(ui)
                    .clicked()
                {
                    st.request(v.s, Intent::Ask);
                }
                if Btn::new("Hide in String", tint)
                    .sm()
                    .enabled(ticked > 0 && !busy)
                    .hint("The files stay where they are; String leaves them out")
                    .show(ui)
                    .clicked()
                {
                    st.request(v.s, Intent::Hide);
                }
            });
        });
        ui.add_space(6.);
        ui.label(
            mono(
                "Either way, the copy you keep takes over their likes, plays, loops, listening time and playlist places.",
                11.,
            )
            .color(FAINT),
        );
    });
}
fn used_for(v: &View<'_>, id: &str) -> String {
    let mut bits = vec![];
    if v.s.models.liked.iter().any(|l| l == id) {
        bits.push("liked".to_owned());
    }
    let lists =
        v.s.field("playlists")
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| {
                p["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|x| x["id"] == id || x.as_str() == Some(id))
            })
            .count();
    if lists > 0 {
        bits.push(format!("in {}", plural(lists, "playlist")));
    }
    let plays = v.s.field("plays")[id][0].as_u64().unwrap_or(0);
    if plays > 0 {
        bits.push(plural(plays as usize, "play"));
    }
    bits.join(" · ")
}
/// One song's copies. True when the person pressed "Not the same".
fn group(ui: &mut Ui, v: &mut View<'_>, st: &mut State, gi: usize) -> bool {
    let tint = v.tint;
    let mut apart = false;
    let sure = st.groups[gi].found.sure;
    ui.push_id(("group", st.look_number, gi), |ui| {
        ui.spacing_mut().item_spacing.y = 0.;
        let g = &st.groups[gi];
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.), Sense::hover());
        let p = ui.painter().clone();
        p.line_segment([head.left_bottom(), head.right_bottom()], (1., HAIR2));
        let inner = head.shrink2(vec2(12., 0.));
        let mut right = inner.right();
        if sure {
            let n = panel::line(
                ui,
                &format!("{} COPIES", g.copies.len()),
                "Mono",
                10.5,
                0.08,
            );
            right -= n.size().x;
            p.galley(pos2(right, inner.center().y - n.size().y * 0.5), n, FAINT);
        } else {
            let mut cell = ui::child(
                ui,
                ui::area(inner.right() - 132., head.top() + 4., 132., 26.),
                "apart",
            );
            cell.with_layout(Layout::right_to_left(Align::Center), |ui| {
                apart = Btn::new("Not the same", tint)
                    .kind(Kind::Ghost)
                    .sm()
                    .hint("They're different songs or versions: stop listing them")
                    .show(ui)
                    .clicked();
            });
            right -= 132.;
            let tag = panel::line(ui, "MAYBE THE SAME", "Mono", 10.5, 0.08);
            let boxed = Rect::from_min_size(
                pos2(right - tag.size().x - 14., inner.center().y - 11.),
                tag.size() + vec2(14., 8.),
            );
            let c = [
                boxed.left_top(),
                boxed.right_top(),
                boxed.right_bottom(),
                boxed.left_bottom(),
                boxed.left_top(),
            ];
            p.extend(egui::Shape::dashed_line(&c, (1., HAIR3), 3., 2.));
            p.galley(boxed.min + vec2(7., 4.), tag, SECONDARY);
            right = boxed.left();
        }
        let names = Rect::from_min_max(inner.left_top(), pos2(right - 12., inner.bottom()));
        let clip = p.with_clip_rect(names.intersect(ui.clip_rect()));
        let title = panel::line(ui, &g.copies[0].title, "Strong", 15., 0.);
        let w = title.size().x;
        clip.galley(
            pos2(names.left(), names.center().y - title.size().y * 0.5),
            title,
            TEXT,
        );
        clip.text(
            pos2(names.left() + w + 12., names.center().y + 1.),
            egui::Align2::LEFT_CENTER,
            &g.artists,
            font("UI", 13.),
            MUTED,
        );
        for ci in 0..st.groups[gi].copies.len() {
            row(ui, v, st, gi, ci);
        }
    });
    apart
}
fn row(ui: &mut Ui, v: &mut View<'_>, st: &mut State, gi: usize, ci: usize) {
    let tint = v.tint;
    let c = &st.groups[gi].copies[ci];
    let (keep, drop) = (st.keep[gi] == c.id, st.drop.contains(&c.id));
    let current = v.s.state.queue.current() == Some(c.id.as_str());
    let (r, hover) = ui.allocate_exact_size(vec2(ui.available_width(), 54.), Sense::hover());
    let p = ui.painter().clone();
    if hover.hovered() {
        p.rect_filled(r, 0, panel::white(0.025));
    }
    p.line_segment([r.left_bottom(), r.right_bottom()], (1., HAIR));
    if keep {
        p.rect_filled(
            Rect::from_min_max(
                pos2(r.left(), r.top() + 10.),
                pos2(r.left() + 2., r.bottom() - 10.),
            ),
            0,
            tint.ink(1.),
        );
    }
    let inner = r.shrink2(vec2(12., 0.));
    let y = r.center().y;
    let wide = r.width() >= 700.;
    let pick = if wide { 92. } else { 84. };
    let fixed = 34. + pick * 2. + 34. + 12. * if wide { 5. } else { 4. };
    let flex = (inner.width() - fixed).max(60.);
    let names_w = if wide { flex * 1.3 / 2.3 } else { flex };
    let names = ui::area(inner.left() + 46., r.top(), names_w, r.height());
    let folder = ui::area(inner.right() - 32., y - 16., 32., 32.);
    let remove = ui::area(folder.left() - 13. - pick, r.top(), pick, r.height());
    let keeping = ui::area(remove.left() - 12. - pick, r.top(), pick, r.height());
    let mut action = None;
    ui.push_id(ci, |ui| {
        let play = ui::area(inner.left(), y - 16., 32., 32.);
        let resp = ui::hit(ui, play, "play", "Play this copy");
        if current || resp.hovered() {
            p.circle_stroke(
                play.center(),
                15.5,
                (1., if current { tint.ink(0.4) } else { HAIR2 }),
            );
        }
        ui::glyph::draw(
            &p,
            Rect::from_center_size(play.center(), vec2(13., 13.)),
            if current && v.st.playing {
                "pausef"
            } else {
                "playf"
            },
            if current { tint.ink(1.) } else { MUTED },
        );
        if resp.on_hover_text("Play this copy").clicked() {
            action = Some(0);
        }
        let fade = if drop { 0.45 } else { 1. };
        let text = p.with_clip_rect(names.intersect(ui.clip_rect()));
        let name = panel::line(ui, &c.name, "UI", 13.5, 0.);
        let at = pos2(names.left(), y - 17.);
        if drop {
            let end = (at.x + name.size().x).min(names.right());
            text.line_segment(
                [pos2(at.x, at.y + 10.), pos2(end, at.y + 10.)],
                (1., panel::white(0.35)),
            );
        }
        text.galley(at, name, TEXT.gamma_multiply(fade));
        ui::label(
            &p,
            ui::area(names.left(), y + 2., names.width(), 16.),
            &c.dir,
            "Mono",
            11.,
            FAINT.gamma_multiply(fade),
        );
        ui.interact(names, ui.scope_id().with("path"), Sense::hover())
            .on_hover_text(&c.path);
        if wide {
            let meta = ui::area(names.right() + 12., r.top(), flex - names_w, r.height());
            ui::label(
                &p,
                ui::area(meta.left(), y - 17., meta.width(), 16.),
                &c.sound,
                "Mono",
                11.5,
                MUTED.gamma_multiply(fade),
            );
            let mut bits = vec![];
            if !st.groups[gi].found.sure {
                bits.push(if c.artist.is_empty() {
                    c.title.clone()
                } else {
                    format!("{} · {}", c.title, c.artist)
                });
            }
            let used = used_for(v, &c.id);
            if !used.is_empty() {
                bits.push(used);
            }
            ui::label(
                &p,
                ui::area(meta.left(), y + 2., meta.width(), 16.),
                &bits.join(" · "),
                "UI",
                11.5,
                FAINT.gamma_multiply(fade),
            );
        }
        let resp = ui::hit(ui, keeping, "keep", "Keep this copy");
        let ink = if keep { tint.ink(1.) } else { MUTED };
        let dot = pos2(keeping.left() + 7., y);
        p.circle_stroke(dot, 6.5, (1., if keep { ink } else { FAINT }));
        if keep {
            p.circle_filled(dot, 3.5, ink);
        } else {
            p.circle_filled(dot, 6., GHOST);
        }
        let word = panel::line(ui, if keep { "KEEPING" } else { "KEEP" }, "HUD", 10.5, 0.12);
        p.galley(
            pos2(keeping.left() + 21., y - word.size().y * 0.5),
            word,
            ink,
        );
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
            && !keep
        {
            action = Some(1);
        }
        let off = if keep { 0.25 } else { 1. };
        let boxed = Rect::from_center_size(pos2(remove.left() + 7., y), vec2(13., 13.));
        p.rect(
            boxed,
            2,
            if drop {
                tint.ink(1.)
            } else {
                GHOST.gamma_multiply(off)
            },
            (1., FAINT.gamma_multiply(off)),
            StrokeKind::Inside,
        );
        if drop {
            let c = boxed.center();
            p.add(egui::Shape::line(
                vec![c + vec2(-3.5, 0.), c + vec2(-1., 2.5), c + vec2(3.5, -2.5)],
                Stroke::new(1.8, panel::INK),
            ));
        }
        let word = panel::line(ui, "REMOVE", "HUD", 10.5, 0.12);
        p.galley(
            pos2(remove.left() + 21., y - word.size().y * 0.5),
            word,
            MUTED.gamma_multiply(off),
        );
        if !keep
            && ui::hit(ui, remove, "remove", "Remove this copy")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
        {
            action = Some(2);
        }
        if ui::icon_btn(ui, folder, "folder", "Show in Explorer", false, tint).clicked() {
            action = Some(3);
        }
    });
    let id = c.id.clone();
    match action {
        Some(0) if current => v.s.toggle(v.player),
        Some(0) => v.s.play_ids(v.player, &[id], 0, Value::Null),
        Some(1) => st.choose_keep(gi, id),
        Some(2) => st.choose_drop(id, !drop),
        Some(3) => {
            let path = PathBuf::from(&c.path);
            services::fire(ui.ctx(), move |s| s.reveal(&path));
        }
        _ => {}
    }
}
/// Smoke cases for this page.
pub mod smoke {
    use super::*;
    use crate::route::Route;
    pub fn open(v: &mut View<'_>) {
        crate::screens::settings::smoke::tidy();
        v.go(Route::Dupes);
    }
    pub fn settled(v: &mut View<'_>) -> bool {
        v.views.history.current == Route::Dupes && state().settled(v)
    }
    pub fn ask(_: &mut View<'_>) {
        state().ask = true;
    }
    pub fn asking(_: &mut View<'_>) -> bool {
        state().ask
    }
    /// Hide the ticked copies, the way the button does.
    pub fn hide(v: &mut View<'_>) {
        let mut st = state();
        st.ask = false;
        st.request(v.s, Intent::Hide);
    }
    /// Put the hidden songs back, the way the link does.
    pub fn back(v: &mut View<'_>) {
        unhide_all(&mut v.s.state);
        v.s.rebuild();
        v.s.queue_changed(v.player);
        v.views.toast.set("Hidden songs are back");
    }
}

#[cfg(test)]
mod tests;
