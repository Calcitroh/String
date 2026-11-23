//! Listening seconds indexed by local date, clock hour, artist and song.

mod period;
pub use period::{Period, PeriodKind, Unit};

use crate::{
    Fields,
    data::{array, object, take},
};
use chrono::{NaiveDate, NaiveDateTime, Timelike};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error, ser::SerializeSeq};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Track(pub String, pub String, pub String);

#[derive(Debug, Clone)]
struct SavedTrack {
    names: Track,
    extra: Vec<Value>,
}
impl Serialize for SavedTrack {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(3 + self.extra.len()))?;
        for name in [&self.names.0, &self.names.1, &self.names.2] {
            seq.serialize_element(name)?;
        }
        for cell in &self.extra {
            seq.serialize_element(cell)?;
        }
        seq.end()
    }
}

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

#[derive(Debug, Default, Clone, Serialize)]
struct Days {
    #[serde(flatten)]
    dated: BTreeMap<NaiveDate, Day>,
    // Keep keys an older importer could write (including invalid phone timestamps) for round trips.
    #[serde(flatten)]
    other: Fields,
}

#[derive(Debug, Clone, Serialize)]
struct Document {
    v: u32,
    tracks: Vec<SavedTrack>,
    artists: Vec<String>,
    days: Days,
    #[serde(
        rename = "phoneImport",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    phone_import: Option<Value>,
    #[serde(flatten)]
    extra: Fields,
}

#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct Listening {
    doc: Document,
    #[serde(skip)]
    phone: Option<PhoneImport>,
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
                days: Days::default(),
                phone_import: None,
                extra: Fields::new(),
            },
            tracks: HashMap::new(),
            artists: HashMap::new(),
            phone: None,
        }
    }
}
impl<'de> Deserialize<'de> for Listening {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut fields = object(Value::deserialize(d)?);
        if take(&mut fields, "v").as_f64().is_some_and(|v| v != 1.) {
            return Err(D::Error::custom("unsupported listening version"));
        }
        // Keep dictionary slots stable: dropping one would credit every following song to its neighbor.
        let saved_artists: Vec<_> = array(take(&mut fields, "artists"))
            .into_iter()
            .map(string)
            .collect();
        let saved_tracks: Vec<_> = array(take(&mut fields, "tracks"))
            .into_iter()
            .map(|row| {
                let mut cells = array(row).into_iter();
                let mut name = || cells.next().map(string).unwrap_or_default();
                SavedTrack {
                    names: Track(name(), name(), name()),
                    extra: cells.collect(),
                }
            })
            .collect();
        let mut days = Days::default();
        for (date, value) in object(take(&mut fields, "days")) {
            let Some(date) = date
                .parse::<NaiveDate>()
                .ok()
                .filter(|parsed| parsed.to_string() == date)
            else {
                days.other.insert(date, value);
                continue;
            };
            let mut row = object(value);
            let mut day = Day::default();
            day.s = take(&mut row, "s").as_f64().unwrap_or_default();
            for (hour, value) in day.h.iter_mut().zip(array(take(&mut row, "h"))) {
                *hour = value.as_f64().unwrap_or_default();
            }
            day.p = take(&mut row, "p").as_f64();
            day.a = seconds(take(&mut row, "a"), saved_artists.len());
            day.t = seconds(take(&mut row, "t"), saved_tracks.len());
            day.extra = row;
            days.dated.insert(date, day);
        }
        let phone_import = fields.remove("phoneImport");
        let phone = phone_import
            .as_ref()
            .filter(|v| !v.is_null())
            .map(import_history);
        let doc = Document {
            v: 1,
            tracks: saved_tracks,
            artists: saved_artists,
            days,
            phone_import,
            extra: fields,
        };
        let tracks = doc
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.names.0.clone(), i))
            .collect();
        let artists = doc
            .artists
            .iter()
            .enumerate()
            .map(|(i, s)| (s.clone(), i))
            .collect();
        Ok(Self {
            doc,
            phone,
            tracks,
            artists,
        })
    }
}

// The importer selects SQLite row IDs strictly greater than the cursor. An unresolved
// cursor therefore has no eligible rows. Keep that bound in memory and its raw value on disk.
fn cursor(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .filter(|n| *n >= 0)
        .unwrap_or(i64::MAX)
}
fn import_history(value: &Value) -> PhoneImport {
    let mut row = object(value.clone());
    let legacy = row.remove("legacyAfter");
    let after = row.remove("after");
    let legacy_after = if !value.is_object() || after.as_ref().is_some_and(|v| !v.is_object()) {
        i64::MAX
    } else {
        legacy.as_ref().map(cursor).unwrap_or(0)
    };
    PhoneImport {
        legacy_after,
        plays: take(&mut row, "plays").as_u64().unwrap_or_default(),
        after: object(after.unwrap_or(Value::Null))
            .into_iter()
            .map(|(user, value)| (user, cursor(&value)))
            .collect(),
        extra: row,
    }
}

fn string(value: Value) -> String {
    match value {
        Value::String(s) => s,
        _ => String::new(),
    }
}
fn seconds(value: Value, dictionary_len: usize) -> BTreeMap<usize, f64> {
    object(value)
        .into_iter()
        .filter_map(|(key, v)| {
            let index = key.parse::<usize>().ok()?;
            let secs = v.as_f64()?;
            (index < dictionary_len).then_some((index, secs))
        })
        .collect()
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
    pub(crate) fn recover_legacy_cursor(&mut self, status: &Value) {
        // Older profiles kept the only cursor in state.json. A malformed one needs the
        // same protection as a per-account cursor before the importer first migrates it.
        if self.phone.is_none()
            && let Some(after) = status.get("after")
            && !after.as_i64().is_some_and(|n| n >= 0)
        {
            let raw = serde_json::json!({"legacyAfter":after,"after":{},"plays":status["plays"].as_u64().unwrap_or(0)});
            self.phone = Some(import_history(&raw));
            self.doc.phone_import = Some(raw);
        }
    }
    pub fn phone_import(&self) -> Option<&PhoneImport> {
        self.phone.as_ref()
    }
    pub fn set_phone_import(&mut self, history: PhoneImport) {
        let mut saved = serde_json::to_value(&history).expect("phone history is JSON");
        if let (Some(original), Some(previous)) = (&self.doc.phone_import, &self.phone) {
            if original.is_object() {
                if history.legacy_after == previous.legacy_after
                    && let Some(raw) = original.get("legacyAfter")
                {
                    saved["legacyAfter"] = raw.clone();
                }
                if let Some(after) = original.get("after") {
                    if history.after == previous.after {
                        saved["after"] = after.clone();
                    } else if let Some(raw) = after.as_object() {
                        for (user, value) in raw {
                            if history.after.get(user) == previous.after.get(user) {
                                saved["after"][user] = value.clone();
                            }
                        }
                    }
                }
            } else {
                saved = original.clone();
            }
        }
        self.doc.phone_import = Some(saved);
        self.phone = Some(history);
    }
    pub fn days(&self) -> &BTreeMap<NaiveDate, Day> {
        &self.doc.days.dated
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
            self.doc.tracks.push(SavedTrack {
                names: t.clone(),
                extra: vec![],
            });
            self.doc.tracks.len() - 1
        });
        let row = &mut self.doc.tracks[ti].names;
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
        let day = self.doc.days.dated.entry(when.date()).or_default();
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
            for d in self.doc.days.dated.values_mut() {
                if let Some(s) = d.t.remove(&fi) {
                    *d.t.entry(ti).or_default() += s;
                }
            }
        } else {
            self.doc.tracks[fi].names.0 = to.into();
            self.tracks.remove(from);
            self.tracks.insert(to.into(), fi);
        }
        true
    }
    pub fn first_day(&self) -> Option<NaiveDate> {
        self.doc
            .days
            .dated
            .iter()
            .find(|(_, d)| d.s > 0.)
            .map(|(k, _)| *k)
    }
    pub fn streak(&self, today: NaiveDate) -> usize {
        let heard = |d: &NaiveDate| self.doc.days.dated.get(d).is_some_and(|day| day.s >= 60.);
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
            for day in self.doc.days.dated.range(from..=to).map(|(_, d)| d) {
                total += day.s;
                phone += day.p.unwrap_or(0.);
                artists.add(&day.a);
                tracks.add(&day.t);
            }
        }
        Range {
            total: round1(total),
            phone: round1(phone),
            artists: artists.sorted(|i| self.doc.artists[i].clone()),
            tracks: tracks.sorted(|i| self.doc.tracks[i].names.clone()),
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
    fn sorted<T>(mut self, name: impl Fn(usize) -> T) -> Vec<(T, f64)> {
        self.rows.sort_by(|x, y| y.1.total_cmp(&x.1));
        self.rows
            .into_iter()
            .map(|(i, s)| (name(i), round1(s)))
            .collect()
    }
}
