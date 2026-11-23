//! Upcoming entries, shuffle changes and recovery of truncated queues.

use super::{Catalog, Item, Queue};
use rand::seq::SliceRandom;

impl Queue {
    /// Transfer playlist and manual queue places when merging a duplicate.
    /// A hidden copy may finish playing. A recycled current copy hands over in place.
    pub fn replace_track_id(&mut self, from: &str, to: &str, include_current: bool) {
        if from == to {
            return;
        }
        let current = self.at();
        for (i, item) in self.items.iter_mut().enumerate() {
            if item.id == from && (include_current || Some(i) != current) {
                to.clone_into(&mut item.id);
            }
        }
        for id in &mut self.orig {
            if id == from {
                to.clone_into(id);
            }
        }
        // Even a candidate outside the queue may have come from a list continuation.
        self.changed();
    }

    fn edit(&mut self, at: usize, count: usize, items: Vec<Item>) {
        let ahead = self.ahead();
        let shift = |i: usize| {
            if i >= ahead && i >= at {
                i.saturating_sub(count).max(at) + items.len()
            } else {
                i
            }
        };
        self.position.list_start = if self.position.past.is_empty() {
            0
        } else {
            shift(self.position.list_start)
        };
        for list in &mut self.position.past {
            list.start = shift(list.start);
        }
        self.items.splice(at..at + count, items);
        self.changed();
    }
    /// With after_manual set, append after consecutive manual entries. Otherwise play next.
    pub fn insert(&mut self, ids: &[String], after_manual: bool, lib: &Catalog) -> Vec<String> {
        let accepted: Vec<_> = ids.iter().filter(|id| lib.playable(id)).cloned().collect();
        if accepted.is_empty() {
            return accepted;
        }
        let mut i = self.ahead();
        if after_manual {
            i += self.upcoming().iter().take_while(|x| x.manual).count();
        }
        self.edit(
            i,
            0,
            accepted
                .iter()
                .map(|id| Item {
                    id: id.clone(),
                    manual: true,
                })
                .collect(),
        );
        accepted
    }
    pub fn remove(&mut self, i: usize) {
        if i < self.ahead() || i >= self.items.len() {
            return;
        }
        if !self.items[i].manual
            && i >= self.position.list_start
            && let Some(k) = self.orig.iter().position(|id| id == &self.items[i].id)
        {
            self.orig.remove(k);
        }
        self.edit(i, 1, vec![]);
    }
    pub fn move_before(&mut self, from: usize, to: usize) {
        if from < self.ahead() || to < self.ahead() || from >= self.items.len() {
            return;
        }
        let item = self.items.remove(from);
        self.items
            .insert((to - usize::from(to > from)).min(self.items.len()), item);
        self.changed();
    }
    pub fn clear_manual(&mut self) {
        let ahead = self.ahead();
        let mut removed = vec![0; self.items.len() + 1];
        for (i, item) in self.items.iter().enumerate() {
            removed[i + 1] = removed[i] + usize::from(i >= ahead && item.manual);
        }
        self.position.list_start -= removed[self.position.list_start];
        for b in &mut self.position.past {
            b.start -= removed[b.start];
        }
        let mut i = 0;
        self.items.retain(|_| {
            i += 1;
            removed[i] == removed[i - 1]
        });
        self.changed();
    }
    pub fn set_shuffle(&mut self, on: bool, lib: &Catalog) {
        self.position.shuffle = on;
        self.changed();
        let Some(at) = self.at() else {
            return;
        };
        let (start, end, _) = self.list(at);
        let after = &self.items[at + 1..end];
        let older;
        let orig = if at >= self.position.list_start {
            &self.orig
        } else {
            older = self.items[start..end]
                .iter()
                .filter(|x| !x.manual)
                .map(|x| x.id.clone())
                .collect();
            &older
        };
        let k = orig.iter().position(|id| id == &self.items[at].id);
        let mut rest: Vec<_> = if on && orig.is_empty() {
            after.iter().filter(|x| !x.manual).cloned().collect()
        } else if on {
            orig.iter()
                .enumerate()
                .filter(|(i, id)| Some(*i) != k && lib.playable(id))
                .map(|(_, id)| Item::song(id.clone()))
                .collect()
        } else {
            orig[k.map_or(0, |i| i + 1)..]
                .iter()
                .filter(|id| lib.tracks.get(*id).is_some_and(|s| !s.hidden))
                .cloned()
                .map(Item::song)
                .collect()
        };
        if on {
            rest.shuffle(&mut rand::rng());
        }
        for (i, item) in after.iter().enumerate().filter(|(_, x)| x.manual) {
            rest.insert(i.min(rest.len()), item.clone());
        }
        self.edit(at + 1, after.len(), rest);
    }

    /// Old truncated queues need the full context order after the library has loaded.
    pub fn needs_legacy_order(&self) -> bool {
        self.legacy_missing_order
    }
    pub fn restore_legacy_order(&mut self, order: Vec<String>) {
        self.changed();
        self.legacy_missing_order = false;
        self.orig = order;
        let Some(at) = self.at() else {
            return;
        };
        let missing_tail = self.items.len() == at + 800;
        if self.shuffled() || (at != 100 && !missing_tail) {
            return;
        }
        let saved: Vec<_> = self
            .items
            .iter()
            .filter(|x| !x.manual)
            .map(|x| &x.id)
            .collect();
        let Some(first) = saved.first() else {
            return;
        };
        let mut matches = self.orig.iter().enumerate().filter(|(_, id)| id == first);
        let Some((i, _)) = matches.next() else {
            return;
        };
        if matches.next().is_some()
            || !self.orig[i..]
                .iter()
                .take(saved.len())
                .eq(saved.iter().copied())
        {
            return;
        }
        let end = i + saved.len();
        if missing_tail {
            self.items
                .extend(self.orig[end..].iter().cloned().map(Item::song));
        }
        if at == 100 {
            self.items
                .splice(0..0, self.orig[..i].iter().cloned().map(Item::song));
            self.position.at += i as isize;
        }
    }
}
