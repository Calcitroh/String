//! Import per-account Navidrome history into the shared listening document.
use crate::object;
use chrono::{Local, TimeZone};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    env, fs, io,
    path::{Path, PathBuf},
    time::Duration,
};
use string_core::{
    data::Profile,
    stats::{PhoneImport, Track},
};

const PAGE: usize = 5000;
/// Automatic checks start eight seconds after load, repeat every five minutes,
/// and also run when the window is shown again.
pub const STARTUP_DELAY: Duration = Duration::from_secs(8);
pub const SYNC_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub fn database_path() -> PathBuf {
    let programs = env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| r"C:\Program Files".into());
    let data = env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| r"C:\ProgramData".into());
    database_from_ini(
        &programs.join("Navidrome/navidrome.ini"),
        &data.join("Navidrome"),
    )
}
pub fn database_from_ini(ini: &Path, fallback: &Path) -> PathBuf {
    let mut folder = fallback.to_owned();
    if let Ok(bytes) = fs::read(ini) {
        for line in crate::text::host_text(&bytes).lines() {
            if let Some((k, v)) = line.split_once('=')
                && k.trim().eq_ignore_ascii_case("DataFolder")
            {
                let value = v
                    .trim()
                    .trim_start_matches(['\'', '"'])
                    .trim_end_matches(['\'', '"']);
                if !value.is_empty() {
                    folder = value.into();
                }
            }
        }
    }
    folder.join("navidrome.db")
}

#[derive(Debug)]
pub struct Play {
    pub n: i64,
    pub at: i64,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub duration: f64,
}
#[derive(Debug, Default)]
pub struct Plays {
    pub found: bool,
    pub users: Vec<(String, String)>,
    pub user: Option<String>,
    pub plays: Vec<Play>,
}
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
}
/// Open a fresh read-only connection, including the live WAL, for each page.
pub fn navidrome_plays(db: &Path, after: i64, requested: &str) -> Result<Plays, Error> {
    if !db.try_exists()? {
        return Ok(Plays::default());
    }
    let con = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    con.busy_timeout(Duration::from_secs(5))?;
    let mut accounts = con.prepare("SELECT id,user_name FROM user ORDER BY user_name")?;
    let users = accounts
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let user = if requested.is_empty() && users.len() == 1 {
        Some(users[0].0.clone())
    } else {
        users
            .iter()
            .find(|(id, _)| id == requested)
            .map(|(id, _)| id.clone())
    };
    let mut page = Plays {
        found: true,
        users,
        user,
        plays: vec![],
    };
    let Some(id) = page.user.as_ref() else {
        return Ok(page);
    };
    let mut songs = con.prepare(
        "SELECT s.id, s.submission_time, COALESCE(m.path,''), COALESCE(l.path,''),
                COALESCE(m.title,''), COALESCE(m.artist,''), COALESCE(m.duration,0)
           FROM scrobbles s
           JOIN media_file m ON m.id = s.media_file_id
           LEFT JOIN library l ON l.id = m.library_id
          WHERE s.user_id = ?1 AND s.id > ?2 ORDER BY s.id LIMIT ?3",
    )?;
    page.plays = songs
        .query_map(params![id, after, PAGE as i64], |r| {
            let rel = r.get::<_, String>(2)?.replace('/', r"\");
            let library = r.get::<_, String>(3)?;
            let rel_path = Path::new(&rel);
            let path = if rel.is_empty() {
                library
            } else if library.is_empty()
                || rel_path.has_root()
                || matches!(
                    rel_path.components().next(),
                    Some(std::path::Component::Prefix(_))
                )
            {
                rel
            } else {
                Path::new(&library).join(rel).to_string_lossy().into_owned()
            };
            Ok(Play {
                n: r.get(0)?,
                at: r.get(1)?,
                path,
                title: r.get(4)?,
                artist: r.get(5)?,
                duration: r.get(6)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(page)
}

#[derive(Debug, Default)]
pub struct SyncResult {
    pub added: usize,
    pub users: Vec<(String, String)>,
}
/// Call on startup, on return to the window, and every five minutes, on a worker
/// owning the loaded profile. Saves each listening page before advancing state.
pub fn sync(profile: &mut Profile, db: &Path) -> Result<SyncResult, Error> {
    let state = &mut profile.state.value;
    if state.fields["set"]["phoneStats"] == false {
        return Ok(SyncResult::default());
    }
    if !state.fields.get("phone").is_some_and(Value::is_object) {
        state
            .fields
            .insert("phone".into(), json!({"after":0,"plays":0}));
    }
    let lookup: HashMap<_, _> = profile
        .library
        .value
        .tracks
        .iter()
        .map(|t| (t.id(), t))
        .collect();
    let mut history = profile
        .listening
        .value
        .phone_import()
        .cloned()
        .unwrap_or_else(|| PhoneImport {
            legacy_after: state.fields["phone"]["after"].as_i64().unwrap_or(0),
            plays: state.fields["phone"]["plays"].as_u64().unwrap_or(0),
            ..Default::default()
        });
    let mut report = SyncResult::default();
    let import = (|| -> Result<(), Error> {
        for _ in 0..40 {
            let selected = state.fields["set"]["phoneUser"].as_str().unwrap_or("");
            let after = *history.after.get(selected).unwrap_or(&history.legacy_after);
            let page = navidrome_plays(db, after, selected)?;
            report.users = page.users;
            object(&mut state.fields, "phone").insert("found".into(), page.found.into());
            let Some(user) = page.user else { break };
            object(&mut state.fields, "set").insert("phoneUser".into(), user.clone().into());
            let mut cursor = *history.after.get(&user).unwrap_or(&history.legacy_after);
            let status = object(&mut state.fields, "phone");
            status.insert("after".into(), cursor.into());
            status.insert("plays".into(), history.plays.into());
            let count = page.plays.len();
            if count == 0 {
                break;
            }
            let mut pending = profile.listening.value.clone();
            let mut added = 0;
            for p in page.plays {
                if p.n <= cursor {
                    continue;
                }
                let id = p.path.to_lowercase();
                let (names, seconds) = if let Some(t) = lookup.get(&id) {
                    let names = crate::edit::display_names(state, t);
                    (
                        Track(id, names.title, names.artist),
                        if t.tags.duration != 0. {
                            t.tags.duration
                        } else {
                            p.duration
                        },
                    )
                } else {
                    (Track(id, p.title, p.artist), p.duration)
                };
                if let Some(when) = Local.timestamp_opt(p.at, 0).single()
                    && pending.add_phone(&names, seconds, when.naive_local())
                {
                    added += 1;
                }
                cursor = p.n;
            }
            history.plays += added as u64;
            history.after.insert(user, cursor);
            pending.set_phone_import(history.clone());
            let old = std::mem::replace(&mut profile.listening.value, pending);
            if let Err(e) = profile.listening.save() {
                profile.listening.value = old;
                return Err(e.into());
            }
            let status = object(&mut state.fields, "phone");
            status.insert("after".into(), cursor.into());
            status.insert("plays".into(), history.plays.into());
            report.added += added;
            if count < PAGE {
                break;
            }
        }
        Ok(())
    })();
    let status = object(&mut state.fields, "phone");
    status.insert("at".into(), Local::now().timestamp_millis().into());
    status.insert(
        "error".into(),
        import
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default()
            .into(),
    );
    profile.state.save()?;
    import?;
    Ok(report)
}
pub fn choose(profile: &mut Profile, db: &Path, user: &str) -> Result<SyncResult, Error> {
    object(&mut profile.state.value.fields, "set").insert("phoneUser".into(), user.into());
    profile.state.save()?;
    sync(profile, db)
}
