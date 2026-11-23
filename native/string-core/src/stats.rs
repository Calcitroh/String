//! Listening seconds indexed by local date, clock hour, artist and song.

mod period;
pub use period::{Period, PeriodKind, Unit};

use crate::Fields;
use chrono::{NaiveDate, NaiveDateTime, Timelike};
use serde::{Deserialize, Deserializer, Serialize, de::Error};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Track(pub String, pub String, pub String);

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Day {
    pub s: f64,
    pub h: [f64; 24],
    pub a: BTreeMap<usize, f64>,
    pub t: BTreeMap<usize, f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p: Option<f64>,
    #[serde(flatten)]
    pub extra: Fields,
}
/// Import cursors travel with the seconds they describe in listening.json.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PhoneImport {
    pub legacy_after: i64,
    pub after: BTreeMap<String, i64>,
    pub plays: u64,
    #[serde(flatten)]
    pub extra: Fields,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Document {
    v: u32,
    tracks: Vec<Track>,
    artists: Vec<String>,
    days: BTreeMap<NaiveDate, Day>,
    #[serde(
        rename = "phoneImport",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    phone_import: Option<PhoneImport>,
    #[serde(flatten)]
    extra: Fields,
}

#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct Listening {
    doc: Document,
    #[serde(skip)]
    tracks: HashMap<String, usize>,
    #[serde(skip)]
    artists: HashMap<String, usize>,
}
impl Default for Listening {
    fn default() -> Self {
        Self {
            doc: Document {
                v: 1,
                tracks: vec![],
                artists: vec![],
                days: BTreeMap::new(),
                phone_import: None,
                extra: Fields::new(),
            },
            tracks: HashMap::new(),
            artists: HashMap::new(),
        }
    }
}
impl<'de> Deserialize<'de> for Listening {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let doc = Document::deserialize(d)?;
        if doc.v != 1 {
            return Err(D::Error::custom("unsupported listening version"));
        }
        for day in doc.days.values() {
            if day.a.keys().any(|i| *i >= doc.artists.len())
                || day.t.keys().any(|i| *i >= doc.tracks.len())
            {
                return Err(D::Error::custom("listening index outside dictionary"));
            }
        }
        let tracks = doc
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.0.clone(), i))
            .collect();
        let artists = doc
            .artists
            .iter()
            .enumerate()
            .map(|(i, s)| (s.clone(), i))
            .collect();
        Ok(Self {
            doc,
            tracks,
            artists,
        })
    }
}

#[derive(Debug)]
pub struct Range {
    pub total: f64,
    pub phone: f64,
    pub artists: Vec<(String, f64)>,
    pub tracks: Vec<(Track, f64)>,
}
pub(super) fn round1(s: f64) -> f64 {
    (s * 10.).round() / 10.
}

impl Listening {
    pub fn phone_import(&self) -> Option<&PhoneImport> {
        self.doc.phone_import.as_ref()
    }
    pub fn set_phone_import(&mut self, history: PhoneImport) {
        self.doc.phone_import = Some(history);
    }
    pub fn days(&self) -> &BTreeMap<NaiveDate, Day> {
        &self.doc.days
    }
    pub fn track_count(&self) -> usize {
        self.doc.tracks.len()
    }
    pub fn artist_count(&self) -> usize {
        self.doc.artists.len()
    }
    pub fn add(&mut self, t: &Track, secs: f64, when: NaiveDateTime) -> bool {
        self.record(t, secs, when, false)
    }
    pub fn add_phone(&mut self, t: &Track, secs: f64, when: NaiveDateTime) -> bool {
        self.record(t, secs, when, true)
    }
    fn record(&mut self, t: &Track, secs: f64, when: NaiveDateTime, phone: bool) -> bool {
        let limit = if phone { 14400. } else { 30. };
        if !(secs > 0. && secs <= limit) {
            return false;
        }
        let ti = *self.tracks.entry(t.0.clone()).or_insert_with(|| {
            self.doc.tracks.push(t.clone());
            self.doc.tracks.len() - 1
        });
        let row = &mut self.doc.tracks[ti];
        if !t.1.is_empty() && t.1 != row.1 {
            row.1.clone_from(&t.1);
        }
        if !t.2.is_empty() && t.2 != row.2 {
            row.2.clone_from(&t.2);
        }
        let ai = *self.artists.entry(t.2.clone()).or_insert_with(|| {
            self.doc.artists.push(t.2.clone());
            self.doc.artists.len() - 1
        });
        let day = self.doc.days.entry(when.date()).or_default();
        day.s += secs;
        day.h[when.hour() as usize] += secs;
        *day.a.entry(ai).or_default() += secs;
        *day.t.entry(ti).or_default() += secs;
        if phone {
            *day.p.get_or_insert(0.) += secs;
        }
        true
    }
    pub fn merge(&mut self, from: &str, to: &str) -> bool {
        let Some(&fi) = self.tracks.get(from) else {
            return false;
        };
        if from == to {
            return false;
        }
        if let Some(&ti) = self.tracks.get(to) {
            for d in self.doc.days.values_mut() {
                if let Some(s) = d.t.remove(&fi) {
                    *d.t.entry(ti).or_default() += s;
                }
            }
        } else {
            self.doc.tracks[fi].0 = to.into();
            self.tracks.remove(from);
            self.tracks.insert(to.into(), fi);
        }
        true
    }
    pub fn first_day(&self) -> Option<NaiveDate> {
        self.doc
            .days
            .iter()
            .find(|(_, d)| d.s > 0.)
            .map(|(k, _)| *k)
    }
    pub fn streak(&self, today: NaiveDate) -> usize {
        let heard = |d: &NaiveDate| self.doc.days.get(d).is_some_and(|day| day.s >= 60.);
        let start = if heard(&today) {
            Some(today)
        } else {
            today.pred_opt()
        };
        start
            .into_iter()
            .flat_map(|d| std::iter::successors(Some(d), |d| d.pred_opt()))
            .take_while(heard)
            .count()
    }
    pub fn range(&self, from: NaiveDate, to: NaiveDate) -> Range {
        let mut total = 0.;
        let mut phone = 0.;
        let mut artists = Ranking::default();
        let mut tracks = Ranking::default();
        if from <= to {
            for day in self.doc.days.range(from..=to).map(|(_, d)| d) {
                total += day.s;
                phone += day.p.unwrap_or(0.);
                artists.add(&day.a);
                tracks.add(&day.t);
            }
        }
        Range {
            total: round1(total),
            phone: round1(phone),
            artists: artists.sorted(&self.doc.artists),
            tracks: tracks.sorted(&self.doc.tracks),
        }
    }
}
#[derive(Default)]
struct Ranking {
    indices: HashMap<usize, usize>,
    rows: Vec<(usize, f64)>,
}
impl Ranking {
    fn add(&mut self, values: &BTreeMap<usize, f64>) {
        for (&i, &s) in values {
            let k = *self.indices.entry(i).or_insert_with(|| {
                self.rows.push((i, 0.));
                self.rows.len() - 1
            });
            self.rows[k].1 += s;
        }
    }
    fn sorted<T: Clone>(mut self, names: &[T]) -> Vec<(T, f64)> {
        self.rows.sort_by(|x, y| y.1.total_cmp(&x.1));
        self.rows
            .into_iter()
            .map(|(i, s)| (names[i].clone(), round1(s)))
            .collect()
    }
}
