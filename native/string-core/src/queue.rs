//! Playback order, manual entries and list history shared by the player and screens.

mod continuation;
mod edits;
pub use continuation::{AdvancePlan, NextAction, PlaybackOptions};

use crate::Fields;
use rand::{RngExt, seq::SliceRandom};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub manual: bool,
}
impl Item {
    pub(crate) fn song(id: String) -> Self {
        Self { id, manual: false }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Boundary {
    pub start: usize,
    pub ctx: Value,
    #[serde(flatten)]
    pub extra: Fields,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Position {
    pub at: isize,
    pub pos: f64,
    pub ctx: Value,
    pub list_start: usize,
    pub past: Vec<Boundary>,
    pub shuffle: bool,
    pub repeat: Repeat,
    #[serde(flatten)]
    pub extra: Fields,
}
impl Default for Position {
    fn default() -> Self {
        Self {
            at: -1,
            pos: 0.,
            ctx: Value::Null,
            list_start: 0,
            past: vec![],
            shuffle: false,
            repeat: Repeat::Off,
            extra: Fields::new(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Song {
    pub artist_key: String,
    pub hidden: bool,
    pub unplayable: bool,
}
#[derive(Debug, Clone)]
pub struct Group {
    pub key: String,
    pub name: String,
    pub ids: Vec<String>,
}

/// Supply the same displayed order as the sorted Songs, Albums and Artists pages.
#[derive(Debug, Default)]
pub struct Catalog {
    pub tracks: HashMap<String, Song>,
    pub songs: Vec<String>,
    pub albums: Vec<Group>,
    pub artists: Vec<Group>,
}
impl Catalog {
    fn playable(&self, id: &str) -> bool {
        self.tracks
            .get(id)
            .is_some_and(|s| !s.hidden && !s.unplayable)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Queue {
    pub(crate) items: Vec<Item>,
    pub(crate) orig: Vec<String>,
    pub(crate) position: Position,
    pub(crate) legacy_missing_order: bool,
    revision: u64,
    history: Vec<String>,
}

impl Queue {
    pub(crate) fn changed(&mut self) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: std::sync::LazyLock<AtomicU64> =
            std::sync::LazyLock::new(|| AtomicU64::new(rand::random::<u64>() | 1));
        self.revision = NEXT.fetch_add(1, Ordering::Relaxed);
    }
    pub fn history(&self) -> &[String] {
        &self.history
    }
    /// Call only after the active output confirms it started or resumed this song.
    pub fn confirm_playing(&mut self, id: &str) {
        if self.current() == Some(id) && !self.history.first().is_some_and(|last| last == id) {
            self.history.insert(0, id.to_owned());
            self.history.truncate(80);
        }
    }
    pub fn items(&self) -> &[Item] {
        &self.items
    }
    pub fn original(&self) -> &[String] {
        &self.orig
    }
    pub fn at(&self) -> Option<usize> {
        usize::try_from(self.position.at).ok()
    }
    pub fn current(&self) -> Option<&str> {
        self.items.get(self.at()?).map(|x| x.id.as_str())
    }
    pub fn pos(&self) -> f64 {
        self.position.pos
    }
    pub fn seek(&mut self, seconds: f64) {
        if seconds.is_finite() {
            self.position.pos = seconds.max(0.);
        }
    }
    pub fn shuffled(&self) -> bool {
        self.position.shuffle
    }
    pub fn repeat(&self) -> Repeat {
        self.position.repeat
    }
    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.position.repeat = repeat;
        self.changed();
    }
    pub fn cycle_repeat(&mut self) {
        self.set_repeat(match self.repeat() {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        });
    }
    fn ahead(&self) -> usize {
        self.at().map_or(0, |i| i + 1)
    }
    pub fn upcoming(&self) -> &[Item] {
        &self.items[self.ahead()..]
    }

    fn list(&self, i: usize) -> (usize, usize, &Value) {
        let p = &self.position;
        if i >= p.list_start || p.past.is_empty() {
            return (p.list_start, self.items.len(), &p.ctx);
        }
        let mut end = p.list_start;
        for b in p.past.iter().rev() {
            if i >= b.start {
                return (b.start, end, &b.ctx);
            }
            end = b.start;
        }
        (0, end, &Value::Null)
    }
    pub fn context(&self, index: Option<usize>) -> &Value {
        self.list(index.or(self.at()).unwrap_or(0)).2
    }

    fn start(&mut self, i: usize) {
        self.position.at = i as isize;
        self.position.pos = 0.;
        self.changed();
    }
    pub fn jump(&mut self, i: usize, lib: &Catalog) -> bool {
        if self.items.get(i).is_some_and(|x| lib.playable(&x.id)) {
            self.start(i);
            true
        } else {
            false
        }
    }
    pub fn previous(&mut self, seconds: f64, lib: &Catalog) -> Option<&str> {
        if let Some(at) = self.at()
            && seconds <= 3.5
        {
            let mut prev = (0..at).rev().find(|i| lib.playable(&self.items[*i].id));
            if prev.is_none() && self.repeat() == Repeat::All {
                prev = (at..self.list(at).1)
                    .rev()
                    .find(|i| lib.playable(&self.items[*i].id));
            }
            if let Some(i) = prev {
                self.start(i);
                return self.current();
            }
        }
        self.seek(0.);
        self.current()
    }
    pub fn play_ids(
        &mut self,
        given: &[String],
        start_at: isize,
        ctx: Value,
        lib: &Catalog,
    ) -> bool {
        let ids: Vec<_> = given
            .iter()
            .filter(|id| lib.playable(id))
            .cloned()
            .collect();
        if ids.is_empty() {
            return false;
        }
        let manual: Vec<_> = self
            .upcoming()
            .iter()
            .filter(|x| x.manual)
            .cloned()
            .collect();
        let mut at = if let Ok(i) = usize::try_from(start_at)
            && i < given.len()
        {
            given[..i]
                .iter()
                .filter(|id| lib.playable(id))
                .count()
                .min(ids.len() - 1)
        } else {
            rand::rng().random_range(0..ids.len())
        };
        self.orig = ids.clone();
        self.legacy_missing_order = false;
        self.items = ids.into_iter().map(Item::song).collect();
        if self.shuffled() {
            self.items.swap(0, at);
            self.items[1..].shuffle(&mut rand::rng());
            at = 0;
        }
        self.items.splice(at + 1..at + 1, manual);
        self.position.ctx = ctx;
        self.position.list_start = 0;
        self.position.past.clear();
        self.start(at);
        true
    }
}
