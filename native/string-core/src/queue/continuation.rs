//! Choose the next song without publishing a speculative list to the queue or disk.

use super::{Boundary, Catalog, Group, Item, Queue, Repeat};
use rand::{RngExt, SeedableRng, rngs::SmallRng, seq::SliceRandom};
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaybackOptions {
    pub autoplay: bool,
    pub looping: bool,
}
impl Default for PlaybackOptions {
    fn default() -> Self {
        Self {
            autoplay: true,
            looping: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextAction {
    Start,
    Restart,
    Stop,
}

#[derive(Debug)]
struct List {
    ids: Vec<String>,
    orig: Vec<String>,
    ctx: Value,
}

/// Hold this ticket while preloading. Discard it if the library order changes.
#[derive(Debug)]
pub struct AdvancePlan {
    pub(super) revision: u64,
    options: PlaybackOptions,
    action: NextAction,
    id: Option<String>,
    index: Option<usize>,
    list: Option<List>,
}
impl AdvancePlan {
    pub fn starts_list(&self) -> bool {
        self.list.is_some()
    }
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }
    pub fn action(&self) -> NextAction {
        self.action
    }
}
fn songs() -> Value {
    json!({"kind":"songs","key":"","name":"All songs"})
}

impl Catalog {
    /// Early native profiles included the separator for a drive-root album directory.
    pub fn resolve_album_key<'a>(&self, key: &'a str) -> &'a str {
        if let Some((_, dir)) = key.rsplit_once('\u{1}')
            && matches!(dir.as_bytes(), [drive, b':', b'\\' | b'/'] if drive.is_ascii_alphabetic())
            && !self.albums.iter().any(|g| g.key == key)
        {
            return &key[..key.len() - 1];
        }
        key
    }
}

impl Queue {
    fn next_index(&self, lib: &Catalog) -> Option<usize> {
        let ahead = self.ahead();
        let wrap = self.repeat() == Repeat::All;
        let (start, end) = if wrap {
            let (s, e, _) = self.list(self.at().unwrap_or(0));
            (s, e)
        } else {
            (0, self.items.len())
        };
        let mut order = (ahead..end).chain(start..if wrap { ahead.min(end) } else { start });
        order.find(|i| lib.playable(&self.items[*i].id))
    }
    fn pick_group(
        &self,
        groups: &[Group],
        key: Option<&str>,
        lib: &Catalog,
        kind: &str,
        rng: &mut SmallRng,
    ) -> Option<List> {
        let at = groups.iter().position(|g| Some(&*g.key) == key);
        let ordered = groups[at.map_or(0, |i| i + 1)..]
            .iter()
            .chain(&groups[..at.unwrap_or(0)]);
        let mut candidates = ordered.filter_map(|g| {
            let ids: Vec<_> = g
                .ids
                .iter()
                .filter(|id| lib.playable(id))
                .cloned()
                .collect();
            (!ids.is_empty()).then_some((g, ids))
        });
        let (g, orig) = if self.shuffled() {
            let recent: HashSet<_> = self.items[..self.ahead()]
                .iter()
                .rev()
                .take(31)
                .map(|x| &x.id)
                .collect();
            let mut pool: Vec<_> = candidates.collect();
            if pool
                .iter()
                .any(|(_, ids)| ids.iter().any(|id| !recent.contains(id)))
            {
                pool.retain(|(_, ids)| ids.iter().any(|id| !recent.contains(id)));
            }
            if pool.is_empty() {
                return None;
            }
            let i = rng.random_range(0..pool.len());
            pool.swap_remove(i)
        } else {
            candidates.next()?
        };
        let mut ids = orig.clone();
        if self.shuffled() {
            ids.shuffle(rng);
        }
        Some(List {
            ids,
            orig,
            ctx: json!({"kind":kind,"key":g.key,"name":g.name}),
        })
    }

    fn pick_continuation(&self, lib: &Catalog) -> Option<List> {
        // Repeated preload queries see the same shuffle until the queue changes.
        let mut rng = SmallRng::seed_from_u64(self.revision);
        let ctx = self.context(None);
        let kind = ctx["kind"].as_str().unwrap_or("");
        let key = ctx["key"].as_str();
        let groups = match kind {
            "album" => Some((&lib.albums, key.map(|k| lib.resolve_album_key(k)))),
            "artist" => Some((&lib.artists, key)),
            _ => None,
        };
        if let Some((groups, key)) = groups
            && let Some(next) = self.pick_group(groups, key, lib, kind, &mut rng)
        {
            return Some(next);
        }
        let cur = self
            .current()
            .filter(|id| lib.tracks.contains_key(*id))
            .unwrap_or("");
        let all: Vec<_> = lib
            .songs
            .iter()
            .filter(|id| lib.playable(id))
            .cloned()
            .collect();
        if !all.iter().any(|id| id != cur) {
            return None;
        }
        let wrap = |orig: Vec<String>, ctx, rng: &mut SmallRng| {
            let mut ids = orig.clone();
            if self.shuffled() {
                ids.retain(|id| id != cur);
                ids.shuffle(rng);
                ids.extend(orig.iter().filter(|id| *id == cur).cloned());
            } else if ids.first().is_some_and(|id| id == cur) {
                ids.rotate_left(1);
            }
            List { ids, orig, ctx }
        };
        if kind == "songs" {
            return Some(wrap(all, songs(), &mut rng));
        }
        let excludes = matches!(kind, "liked" | "pl" | "search" | "album" | "artist");
        let mut in_list = HashSet::from([cur]);
        if excludes {
            in_list.extend(self.orig.iter().map(String::as_str));
        }
        let artist = lib
            .tracks
            .get(cur)
            .map(|s| s.artist_key.as_str())
            .unwrap_or("");
        let (mut same, mut others): (Vec<_>, Vec<_>) = all
            .iter()
            .filter(|id| !in_list.contains(id.as_str()))
            .cloned()
            .partition(|id| !artist.is_empty() && lib.tracks[id].artist_key == artist);
        if !same.is_empty() || !others.is_empty() {
            if self.shuffled() {
                same.shuffle(&mut rng);
                others.shuffle(&mut rng);
            }
            same.extend(others);
            return Some(List {
                orig: same.clone(),
                ids: same,
                ctx: songs(),
            });
        }
        let orig = if self.orig.is_empty() {
            all
        } else {
            self.orig
                .iter()
                .filter(|id| lib.playable(id))
                .cloned()
                .collect()
        };
        orig.iter().any(|id| id != cur).then(|| {
            wrap(
                orig,
                if ctx.is_null() { songs() } else { ctx.clone() },
                &mut rng,
            )
        })
    }

    pub fn what_next(
        &self,
        user_skip: bool,
        options: PlaybackOptions,
        lib: &Catalog,
    ) -> AdvancePlan {
        let mut p = AdvancePlan {
            revision: self.revision,
            options,
            action: NextAction::Stop,
            id: None,
            index: None,
            list: None,
        };
        if self.items.is_empty() {
            return p;
        }
        if !user_skip && self.repeat() == Repeat::One {
            p.action = NextAction::Restart;
            p.id = self.current().map(str::to_owned);
            return p;
        }
        if let Some(i) = self.next_index(lib) {
            p.action = NextAction::Start;
            p.id = Some(self.items[i].id.clone());
            p.index = Some(i);
        } else if options.autoplay && !options.looping && self.repeat() == Repeat::Off {
            if let Some(list) = self.pick_continuation(lib) {
                p.action = NextAction::Start;
                p.id = list.ids.first().cloned();
                p.list = Some(list);
            } else if user_skip && self.current().is_some_and(|id| lib.playable(id)) {
                p.action = NextAction::Restart;
                p.id = self.current().map(str::to_owned);
            }
        }
        p
    }

    /// Returns false if a queue edit, mode change or unavailable target invalidated the preload.
    pub fn commit_next(&mut self, p: AdvancePlan, options: PlaybackOptions, lib: &Catalog) -> bool {
        if p.revision != self.revision
            || p.options != options
            || (p.action == NextAction::Start && !p.id().is_some_and(|id| lib.playable(id)))
        {
            return false;
        }
        if let Some(list) = p.list {
            let ahead = self.ahead();
            let keep = ahead.saturating_sub(200);
            let mut history = std::mem::take(&mut self.position.past);
            history.push(Boundary {
                start: self.position.list_start,
                ctx: self.position.ctx.take(),
                ..Default::default()
            });
            for i in 0..history.len() {
                if history.get(i + 1).map_or(ahead, |b| b.start) > keep {
                    let mut b = history[i].clone();
                    b.start = b.start.saturating_sub(keep);
                    self.position.past.push(b);
                }
            }
            self.items.truncate(ahead);
            self.items.drain(..keep);
            self.position.list_start = self.items.len();
            self.items.extend(list.ids.into_iter().map(Item::song));
            self.orig = list.orig;
            self.position.ctx = list.ctx;
            self.start(self.position.list_start);
        } else if let Some(i) = p.index {
            self.start(i);
        } else {
            self.position.pos = 0.;
            self.changed();
        }
        true
    }
    pub fn next(&mut self, user_skip: bool, options: PlaybackOptions, lib: &Catalog) -> NextAction {
        let plan = self.what_next(user_skip, options, lib);
        let action = plan.action;
        self.commit_next(plan, options, lib);
        action
    }
}
