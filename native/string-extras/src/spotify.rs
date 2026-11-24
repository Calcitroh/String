//! Importing a Spotify listening history ("Extended streaming history").
pub mod export;
pub mod matching;

pub use export::{
    Holds, ImportError, Limits, Source, Summary, find_history, holds, import, import_within,
    is_history,
};
pub use matching::{LibrarySong, Matcher, Resolved, main_artist, normalize_title};
