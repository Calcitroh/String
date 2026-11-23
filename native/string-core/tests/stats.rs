//! The civil-date, ranking, ingestion and migration cases from the listening model.

use chrono::{Datelike, NaiveDate, NaiveDateTime, Weekday};
use string_core::stats::{Listening, PeriodKind::*, Track};

#[test]
fn phone_cursor_round_trips_alongside_listening_seconds() {
    let mut m = Listening::default();
    assert!(m.phone_import().is_none());
    assert!(
        serde_json::to_value(&m)
            .unwrap()
            .get("phoneImport")
            .is_none()
    );
    let saved = serde_json::json!({"legacyAfter":7,"after":{"a":12,"b":20},"plays":9,"future":1});
    m.set_phone_import(serde_json::from_value(saved.clone()).unwrap());
    let m: Listening = serde_json::from_slice(&serde_json::to_vec(&m).unwrap()).unwrap();
    assert_eq!(m.phone_import().unwrap().after["b"], 20);
    assert_eq!(serde_json::to_value(m).unwrap()["phoneImport"], saved);
}
fn at(year: i32, month: u32, day: u32, hour: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(year, month, day)
        .unwrap()
        .and_hms_opt(hour, 0, 0)
        .unwrap()
}
fn track(id: &str, artist: &str) -> Track {
    Track(id.into(), id.to_uppercase(), artist.into())
}
fn sample() -> Listening {
    let mut m = Listening::default();
    for (id, artist, n, s, t) in [
        ("t1", "Artist A", 6, 20., at(2026, 11, 18, 15)),
        ("t2", "Artist B", 3, 20., at(2026, 11, 18, 9)),
        ("t1", "Artist A", 15, 20., at(2026, 11, 17, 22)),
        ("t3", "Artist C", 5, 20., at(2026, 11, 15, 10)),
        ("t1", "Artist A", 1, 25., at(2026, 11, 1, 0)),
        ("t1", "Artist A", 1, 25., at(2026, 11, 1, 23)),
        ("t2", "Artist B", 1, 20., at(2026, 10, 31, 23)),
        ("t2", "Artist B", 1, 20., at(2026, 10, 31, 1)),
        ("t4", "", 1, 30., at(2026, 1, 5, 12)),
        ("t2", "Artist B", 1, 20., at(2025, 12, 31, 23)),
    ] {
        for _ in 0..n {
            assert!(m.add(&track(id, artist), s, t));
        }
    }
    m
}
#[test]
fn fractional_ticks_and_local_dates() {
    let mut m = Listening::default();
    let t = at(2026, 4, 1, 12);
    for _ in 0..240 {
        m.add(&track("s", "A"), 0.25, t);
    }
    assert_eq!(m.range(t.date(), t.date()).total, 60.);
    assert_eq!(
        (at(2026, 3, 9, 0).date() - at(2026, 3, 8, 23).date()).num_days(),
        1
    );
    for secs in [0., -3., 400., f64::NAN] {
        assert!(!m.add(&track("s", "A"), secs, t));
    }
}
#[test]
fn calendar_periods_and_previous_totals() {
    let m = sample();
    let now = at(2026, 11, 18, 15);
    for (kind, week, want, prev, units, cur) in [
        (Day, Weekday::Sun, 180., Some(300.), 24, 15),
        (Week, Weekday::Sun, 580., Some(0.), 7, 3),
        (Week, Weekday::Mon, 480., Some(100.), 7, 2),
        (Month, Weekday::Sun, 630., Some(40.), 30, 17),
        (Year, Weekday::Sun, 700., Some(20.), 12, 10),
        (All, Weekday::Sun, 720., None, 12, 11),
    ] {
        let p = m.period(kind, now, week).unwrap();
        assert_eq!((p.units.len(), p.current), (units, Some(cur)));
        assert_eq!(m.range(p.from, p.to).total, want);
        assert_eq!(p.previous.map(|(a, b)| m.range(a, b).total), prev);
        let series = m.series(&p);
        assert_eq!(series.iter().sum::<f64>(), want);
        match kind {
            Day => {
                assert_eq!((series[9], series[15]), (60., 120.));
            }
            Week if week == Weekday::Sun => {
                assert_eq!(series, [100., 0., 300., 180., 0., 0., 0.]);
                assert_eq!(p.from.day(), 15);
            }
            Week => assert_eq!(p.from.day(), 16),
            Month => assert_eq!((series[0], series[16], series[17]), (50., 300., 180.)),
            Year => assert_eq!(series, [30., 0., 0., 0., 0., 0., 0., 0., 0., 40., 630., 0.]),
            All => assert_eq!(p.from, at(2025, 12, 31, 0).date()),
        }
    }
}
#[test]
fn month_boundaries_leap_year_and_long_histories() {
    let mut m = sample();
    for (date, len, prev) in [
        (at(2026, 12, 31, 23), 31, at(2026, 11, 30, 0)),
        (at(2026, 1, 10, 12), 31, at(2025, 12, 31, 0)),
        (at(2024, 2, 29, 12), 29, at(2024, 1, 31, 0)),
    ] {
        let p = m.period(Month, date, Weekday::Mon).unwrap();
        assert_eq!(p.units.len(), len);
        assert_eq!(p.previous.unwrap().1, prev.date());
    }
    m.add(&track("old", ""), 1., at(2010, 1, 1, 0));
    let p = m.period(All, at(2026, 11, 18, 12), Weekday::Mon).unwrap();
    assert_eq!(p.units.len(), 36);
    assert_eq!(p.from.year(), 2010);
    assert_eq!(p.units[0].from, at(2023, 12, 1, 0).date());
}
#[test]
fn rankings_streaks_and_loaded_names() {
    let m = sample();
    let r = m.range(at(2026, 11, 1, 0).date(), at(2026, 11, 30, 0).date());
    assert_eq!(
        r.artists,
        [
            ("Artist A".into(), 470.),
            ("Artist C".into(), 100.),
            ("Artist B".into(), 60.)
        ]
    );
    assert_eq!(r.tracks[0], (track("t1", "Artist A"), 470.));
    assert_eq!(
        m.range(at(2026, 1, 1, 0).date(), at(2026, 1, 31, 0).date())
            .artists,
        [("".into(), 30.)]
    );
    for (day, want) in [(18, 2), (19, 2), (20, 0)] {
        assert_eq!(m.streak(at(2026, 11, day, 8).date()), want);
    }
    let dir = tempfile::tempdir().unwrap();
    let mut file = string_core::data::DataFolder::new(dir.path(), dir.path().join("backup"))
        .read::<Listening>()
        .unwrap();
    file.value = m.clone();
    file.save().unwrap();
    let bytes = std::fs::read(dir.path().join("listening.json")).unwrap();
    assert!(bytes.len() < 1200);
    let mut again: Listening = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        again
            .range(at(2026, 1, 1, 0).date(), at(2026, 12, 31, 0).date())
            .total,
        700.
    );
    again.add(&track("t1", "Renamed Artist"), 10., at(2026, 11, 18, 16));
    assert_eq!(again.track_count(), m.track_count());
    assert_eq!(
        again
            .range(at(2026, 11, 1, 0).date(), at(2026, 11, 30, 0).date())
            .tracks[0],
        (track("t1", "Renamed Artist"), 480.)
    );
}
#[test]
fn merged_tracks_keep_day_hour_and_artist_time() {
    let mut m = Listening::default();
    let a = at(2026, 11, 1, 12);
    let b = at(2026, 11, 2, 12);
    for (id, artist, s, t) in [
        ("keep", "A", 20., a),
        ("copy", "A", 10., a),
        ("copy", "A", 5., b),
        ("lone", "B", 7., b),
    ] {
        m.add(&track(id, artist), s, t);
    }
    assert!(m.merge("copy", "keep"));
    let r = m.range(a.date(), b.date());
    assert_eq!(r.total, 42.);
    assert_eq!(
        r.tracks,
        [(track("keep", "A"), 35.), (track("lone", "B"), 7.)]
    );
    assert!(m.merge("lone", "fresh"));
    m.add(&track("fresh", "B"), 3., b);
    assert_eq!(
        m.range(a.date(), b.date()).tracks[1],
        (track("fresh", "B"), 10.)
    );
    assert!(!m.merge("absent", "keep"));
    assert!(!m.merge("keep", "keep"));
}
#[test]
fn phone_part_is_included_in_all_buckets() {
    let mut m = Listening::default();
    let pc = at(2026, 11, 3, 9);
    let phone = at(2026, 11, 3, 21);
    m.add(&track("pc", "A"), 20., pc);
    assert!(m.add_phone(&track("ph", "B"), 214.5, phone));
    let r = m.range(pc.date(), pc.date());
    assert_eq!((r.total, r.phone), (234.5, 214.5));
    assert_eq!(r.artists[0], ("B".into(), 214.5));
    assert_eq!(r.tracks[0].0.0, "ph");
    assert_eq!(m.days()[&pc.date()].h[21], 214.5);
    assert_eq!(
        m.range(at(2026, 11, 4, 0).date(), at(2026, 11, 4, 0).date())
            .phone,
        0.
    );
    for s in [0., 99999., f64::NAN] {
        assert!(!m.add_phone(&track("x", ""), s, pc));
    }
    assert!(!m.add(&track("x", ""), 200., pc));
}
