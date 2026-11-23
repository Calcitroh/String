//! Find copies in the visible library, then merge a selection the person confirmed.
use crate::{
    edit::display_names,
    object,
    text::{collate, norm, titles},
    truthy,
};
use serde_json::{Value, json};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};
use string_core::{
    library::{Library, Track},
    state::State,
    stats::Listening,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    /// Track IDs, best copy from the largest sure set first.
    pub ids: Vec<String>,
    /// Equal set numbers are sure copies. Other pairs are possible matches.
    pub sets: Vec<usize>,
    pub sure: bool,
}
impl DuplicateGroup {
    pub fn suggested_drops(&self, keep: usize) -> impl Iterator<Item = &str> {
        self.ids
            .iter()
            .zip(&self.sets)
            .enumerate()
            .filter_map(move |(i, (id, set))| {
                (i != keep && Some(set) == self.sets.get(keep)).then_some(id.as_str())
            })
    }
}
struct Entry<'a> {
    track: &'a Track,
    id: String,
    title: String,
    artist: String,
    plain: String,
    prefer: f64,
}
fn quality(s: &str) -> u8 {
    match s {
        "flac" => 5,
        "wav" => 4,
        "m4a" | "opus" | "ogg" => 3,
        "mp3" | "aac" => 2,
        _ => 1,
    }
}
fn rate(t: &Track) -> f64 {
    if t.tags.duration > 0. {
        t.size as f64 / t.tags.duration
    } else {
        0.
    }
}
fn rank(a: &Entry<'_>, b: &Entry<'_>) -> Ordering {
    quality(&b.track.tags.format)
        .cmp(&quality(&a.track.tags.format))
        .then_with(|| {
            let (ar, br) = (rate(a.track), rate(b.track));
            if br * 0.97 > ar {
                Ordering::Greater
            } else if ar * 0.97 > br {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        })
        .then_with(|| b.prefer.total_cmp(&a.prefer))
        .then_with(|| b.track.size.cmp(&a.track.size))
        .then_with(|| collate(&a.track.path, &b.track.path))
}

// The bitrate tolerance can give cyclic comparisons. Use the page engine's
// natural run and stable binary insertion for small groups, without requiring
// the comparator to be a total order. Each title bucket is limited to 200.
fn rank_group(group: &mut [usize], entries: &[Entry<'_>]) {
    let less = |a: usize, b: usize| rank(&entries[a], &entries[b]).is_lt();
    let descending = less(group[1], group[0]);
    let mut run = 2;
    while run < group.len() && less(group[run], group[run - 1]) == descending {
        run += 1;
    }
    if descending {
        group[..run].reverse();
    }
    for i in run..group.len() {
        let (mut lo, mut hi) = (0, i);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if less(group[i], group[mid]) {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        group[lo..=i].rotate_right(1);
    }
}
struct Sets(Vec<usize>);
impl Sets {
    fn new(n: usize) -> Self {
        Self((0..n).collect())
    }
    fn root(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }
    fn join(&mut self, i: usize, j: usize) {
        let (a, b) = (self.root(i), self.root(j));
        self.0[a] = b;
    }
}
fn pair_key(a: &str, b: &str) -> String {
    if a.encode_utf16().cmp(b.encode_utf16()).is_lt() {
        format!("{a}\u{1}{b}")
    } else {
        format!("{b}\u{1}{a}")
    }
}
pub fn forget_not_same(state: &mut State) {
    object(&mut state.fields, "notDupes").clear();
}
pub fn unhide_all(state: &mut State) {
    object(&mut state.fields, "hidden").clear();
}
fn title_parts(title: &str) -> (String, String) {
    let (plain, core) = titles(title);
    let key = if core.is_empty() { plain.clone() } else { core };
    (plain, key)
}

// Possible and sure edges use the same two-second window in the list and at confirmation.
fn related(a: f64, b: f64, same: bool) -> bool {
    (a > 0. && b > 0. && (a - b).abs() <= 2.) || (same && (a == 0. || b == 0.))
}

pub fn still_copies(state: &State, keep: &Track, drop: &Track) -> bool {
    let (k, d) = (keep.id(), drop.id());
    if k == d
        || state.hidden(&k)
        || state.hidden(&d)
        || truthy(&state.fields["notDupes"][pair_key(&k, &d)])
    {
        return false;
    }
    let (a, b) = (display_names(state, keep), display_names(state, drop));
    let (ap, ak) = title_parts(&a.title);
    let (bp, bk) = title_parts(&b.title);
    !ak.is_empty()
        && ak == bk
        && related(
            keep.tags.duration,
            drop.tags.duration,
            ap == bp && norm(&a.artist) == norm(&b.artist),
        )
}
pub fn duplicates(lib: &Library, state: &State) -> Vec<DuplicateGroup> {
    let playlists = state.fields["playlists"].as_array().into_iter().flatten();
    let listed: HashSet<_> = playlists
        .flat_map(|p| p["items"].as_array().into_iter().flatten())
        .filter_map(|row| row["id"].as_str())
        .collect();
    let tracks: Vec<_> = lib.visible(state).collect();
    let dir = |t: &Track| t.path.rfind(['\\', '/']).unwrap_or(0);
    let mut dirs = HashMap::new();
    for t in &tracks {
        *dirs.entry(&t.path[..dir(t)]).or_insert(0usize) += 1;
    }
    let mut entries = Vec::with_capacity(tracks.len());
    let mut buckets: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, &track) in tracks.iter().enumerate() {
        let id = track.id();
        let names = display_names(state, track);
        let (plain, key) = title_parts(&names.title);
        if !key.is_empty() {
            buckets.entry(key).or_default().push(i);
        }
        let prefer = 4. * dirs[&track.path[..dir(track)]] as f64 / tracks.len() as f64
            + f64::from(truthy(&state.fields["liked"][&id]))
            + f64::from(listed.contains(id.as_str()))
            + 0.5 * f64::from(truthy(&state.fields["plays"][&id][0]));
        entries.push(Entry {
            track,
            id,
            title: norm(&names.title),
            artist: norm(&names.artist),
            plain,
            prefer,
        });
    }
    let n = entries.len();
    let (mut all, mut sure) = (Sets::new(n), Sets::new(n));
    let mut bounds: Vec<_> = tracks
        .iter()
        .map(|t| (t.tags.duration, t.tags.duration))
        .collect();
    for group in buckets.values().filter(|g| (2..=200).contains(&g.len())) {
        for (k, &i) in group.iter().enumerate() {
            for &j in &group[k + 1..] {
                let (x, y) = (&entries[i], &entries[j]);
                if truthy(&state.fields["notDupes"][pair_key(&x.id, &y.id)]) {
                    continue;
                }
                let (a, b) = (x.track.tags.duration, y.track.tags.duration);
                let same = x.plain == y.plain && x.artist == y.artist;
                if !related(a, b, same) {
                    continue;
                }
                all.join(i, j);
                if a > 0. && b > 0. && same && !x.artist.is_empty() {
                    let (r, s) = (sure.root(i), sure.root(j));
                    let range = (bounds[r].0.min(bounds[s].0), bounds[r].1.max(bounds[s].1));
                    if range.1 - range.0 <= 2. {
                        sure.join(r, s);
                        bounds[s] = range;
                    }
                }
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = vec![];
    let mut positions = HashMap::new();
    for i in 0..n {
        let pos = *positions.entry(all.root(i)).or_insert_with(|| {
            groups.push(vec![]);
            groups.len() - 1
        });
        groups[pos].push(i);
    }
    let mut out = vec![];
    for mut g in groups.into_iter().filter(|g| g.len() > 1) {
        rank_group(&mut g, &entries);
        let mut counts = HashMap::new();
        for &i in &g {
            *counts.entry(sure.root(i)).or_insert(0usize) += 1;
        }
        let mut best = 0;
        for i in 1..g.len() {
            if counts[&sure.root(g[i])] > counts[&sure.root(g[best])] {
                best = i;
            }
        }
        g[..=best].rotate_right(1);
        let mut labels = HashMap::new();
        let sets = g
            .iter()
            .map(|&i| {
                let n = labels.len();
                *labels.entry(sure.root(i)).or_insert(n)
            })
            .collect();
        out.push((
            entries[g[0]].title.clone(),
            DuplicateGroup {
                ids: g.iter().map(|&i| entries[i].id.clone()).collect(),
                sets,
                sure: labels.len() == 1,
            },
        ));
    }
    out.sort_by(|(ta, a), (tb, b)| b.sure.cmp(&a.sure).then_with(|| collate(ta, tb)));
    out.into_iter().map(|(_, g)| g).collect()
}
pub fn mark_not_same(state: &mut State, group: &DuplicateGroup, now_ms: i64) {
    let pairs = object(&mut state.fields, "notDupes");
    for (i, (a, sa)) in group.ids.iter().zip(&group.sets).enumerate() {
        for (b, sb) in group.ids.iter().zip(&group.sets).skip(i + 1) {
            if sa != sb {
                pairs.insert(pair_key(a, b), now_ms.into());
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct CopyToMerge {
    pub keep_id: String,
    pub drop_id: String,
    /// Pass this path to the platform recycle operation.
    pub path: String,
}
impl CopyToMerge {
    pub fn new(keep: &Track, drop: &Track) -> Option<Self> {
        let (keep_id, drop_id) = (keep.id(), drop.id());
        (keep_id != drop_id).then(|| Self {
            keep_id,
            drop_id,
            path: drop.path.clone(),
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Hide,
    Recycled,
}

/// For recycling, call only for paths the platform actually moved. Returns the
/// kept ID when playback should switch to it at the current time/play state.
pub fn merge_copy<'a>(
    state: &mut State,
    listening: &mut Listening,
    copy: &'a CopyToMerge,
    mode: Disposition,
    now_ms: i64,
) -> Option<&'a str> {
    let (keep, drop) = (copy.keep_id.as_str(), copy.drop_id.as_str());
    if keep == drop {
        return None;
    }
    for field in ["liked", "loops"] {
        if let Some(map) = state.fields.get_mut(field).and_then(Value::as_object_mut)
            && map.get(drop).is_some_and(truthy)
        {
            let old = map.remove(drop).unwrap();
            if !map.get(keep).is_some_and(truthy) {
                map.insert(keep.to_owned(), old);
            }
        }
    }
    let plays = object(&mut state.fields, "plays");
    if plays.get(drop).is_some_and(truthy) {
        let old = plays.remove(drop).unwrap();
        let a = plays.get(keep).unwrap_or(&Value::Null);
        let count = a[0]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(old[0].as_u64().unwrap_or(0));
        let last = a[1].as_i64().unwrap_or(0).max(old[1].as_i64().unwrap_or(0));
        plays.insert(keep.to_owned(), json!([count, last]));
    }
    for p in state
        .fields
        .get_mut("playlists")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(rows) = p["items"].as_array_mut()
            && rows.iter().any(|row| row["id"] == drop)
        {
            let mut seen = HashSet::new();
            rows.retain_mut(|row| {
                if row["id"] == drop {
                    row["id"] = keep.into();
                }
                seen.insert(row["id"].as_str().map(str::to_owned))
            });
            p["updated"] = now_ms.into();
        }
    }
    let recycled = mode == Disposition::Recycled;
    let switch = recycled && state.queue.current() == Some(drop);
    state.queue.replace_track_id(drop, keep, recycled);
    listening.merge(drop, keep);
    let hidden = object(&mut state.fields, "hidden");
    if recycled {
        hidden.remove(drop);
    } else {
        hidden.insert(drop.into(), now_ms.into());
    }
    switch.then_some(keep)
}
