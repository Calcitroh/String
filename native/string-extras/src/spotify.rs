//! Importing a Spotify listening history ("Extended streaming history").
pub mod export;
pub mod matching;

pub use export::{Holds, ImportError, Source, Summary, find_history, holds, import, is_history};
pub use matching::{Matcher, Resolved, main_artist, normalize_title};
