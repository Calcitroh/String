//! String's core: the library, the saved data, the queue and listening time. No sound, no screens.

pub mod data;
pub mod library;
mod paths;
pub mod queue;
pub mod state;
pub mod stats;
pub mod tags;

use serde_json::{Map, Value};
pub type Fields = Map<String, Value>;
