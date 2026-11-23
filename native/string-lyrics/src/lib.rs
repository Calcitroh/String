//! Local and opt-in online lyrics, and streaming translation on this PC.
//!
//! Keep one [`Lyrics`] and one [`Ollama`] for the application. Run their blocking calls
//! on workers and dispatch updates to the screen. Supply the shared data folder
//! and the displayed song metadata (including title/artist/album edits).
//! Call [`Lyrics::get`] with online access only after the same song has stayed in the
//! lyrics view for 1.2 seconds. An earlier call with online access off can show local
//! lyrics immediately. Use [`Ollama::stop`] on a song change and [`Ollama::shutdown`]
//! when the application closes.
//!
//! ```no_run
//! # use string_lyrics::{Lyrics, Ollama, Song, TranslationOptions, line_at, pick_model};
//! # fn on_worker(lyrics: &Lyrics, ollama: &Ollama, song: &Song<'_>, consent: bool,
//! #              seconds: f64, post_row: impl FnMut(usize, Option<&str>)) -> std::io::Result<()> {
//! let found = lyrics.get(song, consent, false);
//! if found.synced {
//!     let current = line_at(&found.lines, seconds);
//!     // Highlight current when it is Some(index).
//! }
//! let models = ollama.models()?;
//! if let Some(model) = pick_model(&models) {
//!     let lines: Vec<_> = found.lines.iter().map(|l| l.text.as_str()).collect();
//!     let options = TranslationOptions { lang: "English", model };
//!     let result = ollama.translate(song, &lines, options, post_row);
//!     // Ignore stale song updates. On completion, use result.lines and result.error.
//! }
//! # Ok(()) }
//! ```

mod cache;
mod lrc;
mod lrclib;
mod lyrics;
mod ollama;
mod translation;
mod unicode;

pub use lrc::{Line, ParsedLyrics, line_at, parse_lrc};
pub use lrclib::{Candidate, norm, pick_best};
pub use lyrics::{Lyrics, LyricsResult, Song, Source};
pub use ollama::Ollama;
pub use translation::{
    LANGUAGES, Plan, Translation, TranslationOptions, messages, parse_reply, pick_model, plan,
    thinks,
};

// ECMAScript trim and \s include the byte order mark and exclude NEXT LINE.
fn space(c: char) -> bool {
    c == '\u{feff}' || (c != '\u{85}' && c.is_whitespace())
}
fn trim(s: &str) -> &str {
    s.trim_matches(space)
}

fn loopback(uri: &ureq::http::Uri) -> bool {
    let host = uri.host().unwrap_or("").trim_matches(['[', ']']);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}
