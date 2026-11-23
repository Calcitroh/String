//! Poll the Settings worker even when the window is on another page.
use crate::{
    playback::Session,
    screens::settings::{self, Updates},
    services::{self, Task, UpdateInfo},
    view::Views,
};
use egui::Context;
use serde_json::json;
use std::time::{Duration, Instant};

const FIRST: Duration = Duration::from_secs(60);
const INTERVAL: Duration = Duration::from_secs(6 * 3600);
const DAY_MS: i64 = 86_400_000;

pub(crate) struct Daily {
    started: Instant,
    next: Instant,
    quiet: bool,
}
impl Default for Daily {
    fn default() -> Self {
        Self::new(Instant::now())
    }
}
impl Daily {
    fn new(started: Instant) -> Self {
        Self {
            started,
            next: started + FIRST,
            quiet: false,
        }
    }

    fn due(&mut self, now: Instant, wall_ms: i64, session: &Session) -> bool {
        if now < self.next {
            return false;
        }
        // The six-hour interval starts at launch. Waking after several missed
        // ticks runs one check and keeps the original cadence.
        let interval = INTERVAL.as_secs();
        let elapsed = now.duration_since(self.started).as_secs();
        self.next = self.started + Duration::from_secs((elapsed / interval + 1) * interval);
        let last = session.field("update")["at"].as_f64().unwrap_or(0.);
        session.setting("updates") != false && wall_ms as f64 - last >= DAY_MS as f64
    }
}

pub fn tick(ctx: &Context, session: &mut Session, views: &mut Views, smoke: bool) {
    let mut settings = settings::state();
    let u = &mut settings.updates;
    if let Some(result) = u.checking.as_ref().and_then(Task::poll) {
        u.checking = None;
        if let Some(message) = finish(u, session, result, chrono::Utc::now().timestamp_millis()) {
            views.toast.set(message);
        }
    }
    if smoke {
        return;
    }
    let now = Instant::now();
    if now >= u.daily.next
        && should_start(
            u,
            session,
            now,
            chrono::Utc::now().timestamp_millis(),
            || services::get().update_info(),
        )
    {
        u.check(ctx);
    }
    ctx.request_repaint_after(u.daily.next.saturating_duration_since(now));
}

fn should_start(
    u: &mut Updates,
    session: &Session,
    now: Instant,
    wall_ms: i64,
    info: impl FnOnce() -> UpdateInfo,
) -> bool {
    if !u.daily.due(now, wall_ms, session) || u.checking.is_some() || u.busy {
        return false;
    }
    let info = info();
    u.daily.quiet = info.installed && info.published;
    u.daily.quiet
}

fn finish(
    u: &mut Updates,
    session: &mut Session,
    result: Result<UpdateInfo, String>,
    wall_ms: i64,
) -> Option<String> {
    let quiet = std::mem::take(&mut u.daily.quiet);
    let info = match result {
        Ok(info) => info,
        Err(err) => {
            u.msg = format!("Couldn’t check: {err}");
            return (!quiet).then(|| u.msg.clone());
        }
    };
    let mut seen = session.field("update").clone();
    if !seen.is_object() {
        seen = json!({});
    }
    seen["at"] = wall_ms.into();
    let notice = if quiet && info.newer && seen["told"].as_str() != Some(&info.latest) {
        seen["told"] = json!(info.latest);
        Some(format!("String {} is out: Settings → Updates", info.latest))
    } else if !quiet && info.installed && info.published && !info.newer {
        Some("String is up to date".into())
    } else {
        None
    };
    session.set_field("update", seen);
    u.info = Some(info);
    notice
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };
    use string_extras::updates::Updater;

    fn release_feed() -> (String, thread::JoinHandle<()>) {
        let server = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let origin = format!("http://{}", server.local_addr().unwrap());
        let worker = thread::spawn(move || {
            for version in [Some("2.1.0"), None, Some("2.1.0"), Some("2.2.0")] {
                let mut socket = server.accept().unwrap().0;
                let headers = BufReader::new(&socket)
                    .lines()
                    .map(Result::unwrap)
                    .take_while(|s| !s.is_empty())
                    .collect::<Vec<_>>();
                assert_eq!(headers[0], "GET /releases/latest HTTP/1.1");
                assert!(
                    headers
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case("User-Agent: String-updater/1.0"))
                );
                let (status, body) = match version {
                    Some(v) => (
                        "200 OK",
                        json!({"tag_name":v, "assets":[
                            {"name":"String.zip","browser_download_url":"unused"},
                            {"name":"String.zip.sig","browser_download_url":"unused"}
                        ]})
                        .to_string(),
                    ),
                    None => ("503 Unavailable", String::new()),
                };
                write!(
                    socket,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (origin, worker)
    }

    #[test]
    fn manual_results_keep_the_daily_history_and_failures_are_retried() {
        let mut session = Session::new(Default::default(), Default::default(), Default::default());
        let mut updates = Updates::default();
        session.set_field("update", json!({"at": 42, "told": "2.1.0"}));
        let info = UpdateInfo {
            installed: true,
            published: true,
            ..Default::default()
        };
        assert_eq!(
            finish(&mut updates, &mut session, Ok(info), 100).as_deref(),
            Some("String is up to date")
        );
        assert_eq!(
            session.field("update"),
            &json!({"at": 100, "told": "2.1.0"})
        );
        updates.daily.quiet = true;
        assert!(finish(&mut updates, &mut session, Err("offline".into()), 200).is_none());
        assert_eq!(session.field("update")["at"], 100);
        assert!(finish(&mut updates, &mut session, Err("offline".into()), 300).is_some());
    }

    #[test]
    fn daily_checks_follow_the_clock_and_remember_releases_across_restarts() {
        let mut session = Session::new(Default::default(), Default::default(), Default::default());
        let mut updates = Updates::default();
        let start = updates.daily.started;
        let dir = tempfile::tempdir().unwrap();
        let (origin, server) = release_feed();
        let feed = Updater::for_test_feed(dir.path(), "2.0.0", &origin, "").unwrap();
        let mut checks = vec![];
        let mut notices = vec![];
        session.set_field("update", json!({"extra": 7}));
        let mut poll = |updates: &mut Updates, session: &mut Session, elapsed: Duration| {
            let wall_ms = 10 * DAY_MS + elapsed.as_millis() as i64;
            if should_start(updates, session, start + elapsed, wall_ms, || UpdateInfo {
                installed: feed.installed(),
                published: true,
                ..Default::default()
            }) {
                checks.push(elapsed);
                let result = feed
                    .check()
                    .map(|checked| UpdateInfo {
                        installed: checked.installed,
                        published: true,
                        newer: checked.newer,
                        latest: checked
                            .latest
                            .map(|r| r.version.to_string())
                            .unwrap_or_default(),
                        ..Default::default()
                    })
                    .map_err(|e| e.to_string());
                notices.extend(finish(updates, session, result, wall_ms));
            }
        };
        // A loose copy is ineligible even when the daily switch is on.
        poll(&mut updates, &mut session, FIRST);
        assert!(session.field("update")["at"].is_null());
        fs::create_dir(dir.path().join("setup")).unwrap();
        fs::write(
            dir.path().join("setup/installed.txt"),
            dir.path().to_str().unwrap(),
        )
        .unwrap();
        updates.daily = Daily::new(start);
        poll(&mut updates, &mut session, FIRST - Duration::from_millis(1));
        assert!(session.field("update")["at"].is_null());
        poll(&mut updates, &mut session, FIRST);
        let checked = session.field("update").clone();
        assert_eq!(checked["told"], "2.1.0");
        assert_eq!(checked["at"], 10 * DAY_MS + 60_000);
        assert_eq!(checked["extra"], 7);
        for n in 1..=4 {
            poll(&mut updates, &mut session, INTERVAL * n);
        }
        assert_eq!(session.field("update"), &checked);
        // At 30h the fetch fails. The previous success still governs retries.
        poll(&mut updates, &mut session, INTERVAL * 5);
        assert_eq!(session.field("update"), &checked);
        session.set_setting("updates", false.into());
        poll(&mut updates, &mut session, INTERVAL * 6);
        assert_eq!(session.field("update"), &checked);
        session.set_setting("updates", true.into());
        poll(&mut updates, &mut session, INTERVAL * 7 - FIRST);
        assert_eq!(session.field("update"), &checked);
        poll(&mut updates, &mut session, INTERVAL * 7);
        assert_eq!(
            session.field("update")["at"],
            10 * DAY_MS + 7 * 21_600_000i64
        );
        // A new process reads the same at/told fields as 1.x.
        let saved = dir.path().join("state.json");
        fs::write(&saved, serde_json::to_vec(&session.state).unwrap()).unwrap();
        session = Session::new(
            serde_json::from_slice(&fs::read(saved).unwrap()).unwrap(),
            Default::default(),
            Default::default(),
        );
        updates.daily = Daily::new(start + INTERVAL * 7);
        poll(&mut updates, &mut session, INTERVAL * 7 + FIRST);
        poll(&mut updates, &mut session, INTERVAL * 11);
        assert_eq!(checks, [FIRST, INTERVAL * 5, INTERVAL * 7, INTERVAL * 11]);
        assert_eq!(
            notices,
            [
                "String 2.1.0 is out: Settings → Updates",
                "String 2.2.0 is out: Settings → Updates"
            ]
        );
        server.join().unwrap();
    }
}
