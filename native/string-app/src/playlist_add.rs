use crate::{add_work::Location, platform::Start, playback::Session};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashSet},
    io,
    path::PathBuf,
    time::Duration,
};
use string_core::library::track_id;
use string_platform::instance::{Claim, Names};

pub(crate) fn claim(
    names: &Names,
    name: Option<&str>,
    paths: &[PathBuf],
    wait: Duration,
    open_input: impl FnOnce() -> io::Result<crate::add_work::Input>,
) -> io::Result<Start> {
    if paths.is_empty() {
        return Ok(Start::Done(0));
    }
    let owner = names.claim_for_add(|| {
        let input = open_input()?;
        input.startup(name, paths)?;
        Ok(move |name, paths, bytes| input.submit(name, paths, bytes))
    })?;
    Ok(match owner {
        Claim::First(instance) => Start::Run(instance),
        Claim::AlreadyRunning => {
            let taken = match name {
                Some(name) => names.send_playlist_add(name, paths, wait),
                None => names.send_paths(paths, wait),
            };
            Start::Done(if taken { 0 } else { 1 })
        }
    })
}

/// Reserve ordinary playlist entries immediately. The playlist view already hides IDs until the scan finds them.
/// A trailing separator reserves a directory's position; no song ID can end in a directory separator.
pub(crate) fn reserve(s: &mut Session, name: &str, paths: &[Location]) {
    if paths.is_empty() {
        return;
    }
    let folded = name.trim().to_lowercase();
    let key = s
        .models
        .playlists
        .iter()
        .find(|p| p.name.trim().to_lowercase() == folded)
        .map(|p| p.id.clone());
    let mut seen: HashSet<_> = key
        .iter()
        .flat_map(|k| s.playlist_entries(k))
        .map(|(_, id, _)| id)
        .collect();
    let ids: Vec<_> = paths
        .iter()
        .filter_map(|path| {
            let id = track_id(path.path.to_str()?);
            if path.directory {
                Some(format!("{}\\", id.trim_end_matches(['/', '\\'])))
            } else {
                Some(id)
            }
        })
        .filter(|id| seen.insert(id.clone()))
        .collect();
    if ids.is_empty() {
        return;
    }
    if let Some(key) = key {
        s.add_to_playlist(&key, ids.into());
    } else {
        s.add_playlist(name.trim().to_owned(), &ids);
    }
}

fn item_id(item: &Value) -> Option<&str> {
    item.as_str().or_else(|| item["id"].as_str())
}

/// Expand folder positions only after the normal library scan accepts the current roots and files.
/// Unfinished positions survive closing and reopening in the same playlist document as file positions.
pub(crate) fn resolve_folders(s: &mut Session, complete_folders: &HashSet<String>) {
    let Some(lists) = s
        .state
        .fields
        .get_mut("playlists")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let mut library: Option<BTreeSet<String>> = None;
    for list in lists {
        let Some(items) = list.get_mut("items").and_then(Value::as_array_mut) else {
            continue;
        };
        let complete = |id: &str| {
            id.strip_suffix('\\')
                .is_some_and(|f| complete_folders.contains(f))
        };
        if !items.iter().filter_map(item_id).any(complete) {
            continue;
        }
        let tracks =
            library.get_or_insert_with(|| s.library.tracks.iter().map(|t| t.id()).collect());
        let mut seen = HashSet::new();
        let mut from_folder = HashSet::new();
        for item in std::mem::take(items) {
            match item_id(&item) {
                Some(folder) if complete(folder) => {
                    for id in tracks
                        .range(folder.to_owned()..)
                        .take_while(|id| id.starts_with(folder))
                    {
                        if seen.insert(id.clone()) {
                            items.push(json!({"id": id, "at": item["at"]}));
                            from_folder.insert(id.clone());
                        }
                    }
                }
                Some(id) if from_folder.contains(id) => {}
                Some(id) => {
                    seen.insert(id.to_owned());
                    items.push(item);
                }
                None => items.push(item),
            }
        }
    }
    if library.is_some() {
        s.models.refresh_lists(&s.state);
        s.generation += 1;
        s.state_dirty = true;
    }
}

#[cfg(test)]
mod tests;
