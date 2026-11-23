//! Inspect, round-trip or scan an explicitly selected data folder.

use std::{env, io, path::PathBuf, process::ExitCode};
use string_core::{
    data::{DataFolder, Document, Model},
    library::Library,
    state::State,
    stats::Listening,
};

const USAGE: &str = "usage: stringctl <inspect|roundtrip|scan> --data <dir> [--backup <dir>] [folders or loose files...]";

fn rewrite<T: Model>(folder: &DataFolder, doc: &mut Document<T>) -> io::Result<()> {
    if !doc.exists() {
        println!("{}.json: absent", T::NAME);
        return Ok(());
    }
    let result = doc.save()?;
    let reread = folder.read::<T>()?;
    if !reread.exists() || doc.json()? != reread.json()? {
        return Err(io::Error::other(format!(
            "{}.json: model changed on reload",
            T::NAME
        )));
    }
    let status = if result.bytes_changed {
        "bytes changed"
    } else {
        "bytes unchanged"
    };
    println!("{}.json: model unchanged, {status}", T::NAME);
    if let Some(e) = result.backup_error {
        return Err(io::Error::other(format!(
            "{}.json saved; backup failed: {e}",
            T::NAME
        )));
    }
    Ok(())
}
fn inspect<T: Model>(folder: &DataFolder, summary: impl FnOnce(&T) -> String) -> bool {
    match folder.read::<T>() {
        Ok(d) if d.exists() => {
            println!("{}.json: {}", T::NAME, summary(&d.value));
            true
        }
        Ok(_) => {
            println!("{}.json: absent", T::NAME);
            true
        }
        Err(e) => {
            eprintln!("{e}");
            false
        }
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let cmd = args
        .next()
        .and_then(|s| s.into_string().ok())
        .ok_or(USAGE)?;
    let mut data = None;
    let mut backup = None;
    let mut paths = vec![];
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--data") if data.is_none() => {
                data = Some(PathBuf::from(args.next().ok_or(USAGE)?))
            }
            Some("--backup") if backup.is_none() => {
                backup = Some(PathBuf::from(args.next().ok_or(USAGE)?))
            }
            Some("--") => {
                paths.extend(args.by_ref());
                break;
            }
            Some(s) if s.starts_with("--") => return Err(USAGE.into()),
            _ => paths.push(arg),
        }
    }
    let root = data.ok_or(USAGE)?;
    let folder = DataFolder::new(&root, backup.unwrap_or_else(|| root.join("backup")));
    match cmd.as_str() {
        "inspect" if paths.is_empty() => {
            let a = inspect::<State>(&folder, |s| {
                format!(
                    "{} queue items, {} playlists",
                    s.queue.items().len(),
                    s.fields
                        .get("playlists")
                        .and_then(|v| v.as_array())
                        .map_or(0, |a| a.len())
                )
            });
            let b = inspect::<Library>(&folder, |l| {
                for t in l.tracks.iter().filter(|t| t.read_error != 0) {
                    eprintln!("unread tags: {}", t.path);
                }
                format!(
                    "{} tracks, {} unread tags",
                    l.tracks.len(),
                    l.tracks.iter().filter(|t| t.read_error != 0).count()
                )
            });
            let c = inspect::<Listening>(&folder, |s| {
                format!(
                    "{} days, {} artists, {} songs",
                    s.days().len(),
                    s.artist_count(),
                    s.track_count()
                )
            });
            if !(a && b && c) {
                return Err("could not read every document".into());
            }
        }
        "roundtrip" if paths.is_empty() => {
            let mut p = folder.open()?;
            rewrite(&folder, &mut p.state)?;
            rewrite(&folder, &mut p.listening)?;
            rewrite(&folder, &mut p.library)?;
        }
        "scan" if !paths.is_empty() => {
            let mut p = folder.open()?;
            let mut roots = vec![];
            let mut loose = vec![];
            for arg in paths {
                let path = std::path::absolute(PathBuf::from(arg))?;
                if path.is_file() {
                    loose.push(path);
                } else {
                    roots.push(path);
                }
            }
            let r = p.library.value.scan(&roots, &loose);
            if (r.changed() || !p.library.exists())
                && let Some(error) = p.library.save()?.backup_error
            {
                return Err(format!("library.json saved; backup failed: {error}").into());
            }
            println!(
                "{} tracks, {} read, {} reused, {} removed",
                p.library.value.tracks.len(),
                r.read,
                r.reused,
                r.removed
            );
            for issue in &r.issues {
                eprintln!("{}: {}", issue.path.display(), issue.error);
            }
            if !r.issues.is_empty() {
                return Err(format!("{} scan issues", r.issues.len()).into());
            }
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
