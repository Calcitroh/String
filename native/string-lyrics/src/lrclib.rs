use crate::{Song, space, unicode};
use serde::Deserialize;
use serde_json::Value;
use std::{fmt::Write, io, time::Duration};
use ureq::Agent;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Candidate {
    pub track_name: Option<String>,
    pub name: Option<String>,
    pub artist_name: Option<String>,
    pub duration: Option<f64>,
    pub synced_lyrics: Option<String>,
    pub plain_lyrics: Option<String>,
    pub instrumental: Option<bool>,
}
impl Candidate {
    pub(crate) fn text(&self) -> Option<&str> {
        self.synced_lyrics
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(self.plain_lyrics.as_deref().filter(|s| !s.is_empty()))
    }
}

pub fn norm(s: &str) -> String {
    const QUALIFIERS: &[&str] = &[
        "feat", "ft", "with", "prod", "remaster", "live", "version", "edit", "mix", "official",
        "lyric", "audio", "video", "hd", "hq",
    ];
    let lower = unicode::decompose(s)
        .replace(|c| ('\u{0300}'..='\u{036f}').contains(&c), "")
        .to_lowercase();
    let mut kept = String::new();
    let mut rest = lower.as_str();
    while let Some(i) = rest.find(['[', '(']) {
        kept.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        if QUALIFIERS.iter().any(|q| tail.starts_with(q)) {
            let Some(end) = tail.find([']', ')']) else {
                rest = &rest[i..];
                break;
            };
            kept.truncate(kept.trim_end_matches(space).len());
            rest = &tail[end + 1..];
        } else {
            kept.push(rest.as_bytes()[i] as char);
            rest = tail;
        }
    }
    kept.push_str(rest);
    kept.split(|c| !unicode::letter_or_number(c))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn pick_best<'a>(
    results: &'a [Candidate],
    title: &str,
    artist: &str,
    duration: f64,
) -> Option<&'a Candidate> {
    let title = norm(title);
    let artist = norm(artist);
    let mut best = None;
    let mut best_score = -1.;
    for r in results {
        if r.text().is_none() && r.instrumental != Some(true) {
            continue;
        }
        let rt = norm(
            r.track_name
                .as_deref()
                .filter(|s| !s.is_empty())
                .or(r.name.as_deref())
                .unwrap_or(""),
        );
        if rt.is_empty() || !(rt.contains(&title) || title.contains(&rt)) {
            continue;
        }
        let dd = r
            .duration
            .filter(|d| duration > 0. && *d > 0.)
            .map(|d| (duration - d).abs());
        if dd.is_some_and(|d| d > 4.) {
            continue;
        }
        let ra = norm(r.artist_name.as_deref().unwrap_or(""));
        let artist_ok =
            !artist.is_empty() && !ra.is_empty() && (artist.contains(&ra) || ra.contains(&artist));
        if !artist.is_empty() && !artist_ok && dd.is_none() {
            continue;
        }
        let mut score = dd.map_or(0., |d| 3. - d * 0.5);
        if rt == title {
            score += 3.;
        }
        if artist_ok {
            score += 4.;
        }
        if r.synced_lyrics.as_deref().is_some_and(|s| !s.is_empty()) {
            score += 2.;
        }
        if score > best_score {
            best = Some(r);
            best_score = score;
        }
    }
    best
}

pub(crate) struct Lrclib {
    base: String,
    http: Agent,
}
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("LRCLIB is busy")]
    Busy,
    #[error(transparent)]
    Http(#[from] ureq::Error),
    #[error(transparent)]
    Invalid(#[from] io::Error),
}

// Match encodeURIComponent (including its punctuation and %20 for space).
fn query(fields: &[(&str, &str)]) -> String {
    let mut q = String::new();
    for &(key, value) in fields.iter().filter(|(_, v)| !v.is_empty()) {
        if !q.is_empty() {
            q.push('&');
        }
        q.push_str(key);
        q.push('=');
        for b in value.bytes() {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                q.push(b as char);
            } else {
                write!(q, "%{b:02X}").unwrap();
            }
        }
    }
    q
}

impl Lrclib {
    pub(crate) fn new(base: &str) -> Self {
        let mut config = Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(10)));
        if base.parse().is_ok_and(|uri| crate::loopback(&uri)) {
            config = config.proxy(None);
        }
        Self {
            base: base.trim_end_matches('/').into(),
            http: config.build().into(),
        }
    }
    fn get(&self, path: &str) -> Result<Option<Value>, Error> {
        if !path.starts_with("/api/")
            || path.contains("..")
            || path.contains("://")
            || path.contains('\\')
            || path.split('?').next().unwrap_or("").contains('%')
        {
            return Err(io::Error::other("Not an LRCLIB address").into());
        }
        let mut res = self
            .http
            .get(format!("{}{path}", self.base))
            .header("User-Agent", "String v1.0 (personal music player)")
            .header("Lrclib-Client", "String v1.0")
            .call()?;
        match res.status().as_u16() {
            200 => Ok(Some(res.body_mut().read_json()?)),
            404 => Ok(None),
            429 => Err(Error::Busy),
            n => Err(io::Error::other(format!("LRCLIB answered {n}")).into()),
        }
    }
    pub(crate) fn lookup(&self, song: &Song<'_>) -> Result<Option<Candidate>, Error> {
        let duration = song.duration.round() as i64;
        if !song.artist.is_empty() {
            let seconds = if duration == 0 {
                String::new()
            } else {
                duration.to_string()
            };
            let q = query(&[
                ("track_name", song.title),
                ("artist_name", song.artist),
                ("album_name", song.album),
                ("duration", &seconds),
            ]);
            if let Some(hit) = self.get(&format!("/api/get?{q}"))?.filter(|j| !j.is_null()) {
                return Ok(Some(serde_json::from_value(hit).map_err(io::Error::other)?));
            }
        }
        let q = if song.artist.is_empty() {
            query(&[("q", song.title)])
        } else {
            query(&[("track_name", song.title), ("artist_name", song.artist)])
        };
        let list = self
            .get(&format!("/api/search?{q}"))?
            .unwrap_or(Value::Null);
        let list = match list {
            Value::Array(v) => v
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect::<Vec<_>>(),
            Value::Null => vec![],
            _ => return Err(io::Error::other("Invalid LRCLIB search response").into()),
        };
        Ok(pick_best(&list, song.title, song.artist, duration as f64).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_api_paths_before_connecting() {
        let api = Lrclib::new("http://127.0.0.1:0");
        for path in [
            "/other",
            "/api/../other",
            "https://example.invalid/api/get",
            "/api/%2e%2e/other",
            "/api/\\other",
        ] {
            assert!(
                matches!(api.get(path), Err(Error::Invalid(e)) if e.to_string()=="Not an LRCLIB address")
            );
        }
    }
}
