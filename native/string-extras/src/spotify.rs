//! Importing a Spotify listening history ("Extended streaming history").
pub mod export;

pub use export::{ImportError, Source, Summary, find_history, import, is_history};
