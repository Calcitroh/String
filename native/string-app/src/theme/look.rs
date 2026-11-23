//! The person's own look: `<data>\look\look.css` with the pictures beside it, the same folder and file String 1.x
//! loads after its own styles. 2.0 draws with egui, not CSS, so it reads the parts of the starter's examples
//! (`src\wwwroot\look-starter.css`) that it can draw, with the meaning they have in 1.x:
//!
//! - the tint: `--hi: r g b` on `:root` (or `html`, `body`, `*`), a custom tint over Mono, Arc and Ember. As in 1.x,
//!   without `!important` it wins over Mono only, since Arc and Ember's own rules are more specific;
//! - the logo: a picture on `.brand` (`background` or `background-image`, with its size, position, repeat and
//!   `content-box`), `.brand`'s `height` and `padding`, and `.brand > *` (or just its `svg` or `b`) hidden;
//! - a picture behind the pages: `#main`'s background.
//!
//! Left out: the dark surfaces and text colours (`--bg`, `--panel`, `--text`, …), which are constants every screen
//! draws with, and fonts, as egui stops the program on a font it can't read and checking one first needs another
//! library. Everything 2.0 doesn't draw is skipped, so a look made for 1.x still works here, minus those.
//!
//! Only files inside the look folder are read, never through a link (a symbolic link or junction anywhere below the
//! folder, as 1.x refuses), each read whole and closed at once, so an editor's save never finds one held open. A
//! missing or broken look.css is no look; a missing or broken picture is no picture. While the look is on, a worker
//! looks at the files it uses four times a second (once a second while there's no look.css) and reads them again once
//! a change has held still for one look.
use super::Tint;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use egui::{
    Color32, ColorImage, Context, Painter, Pos2, Rect, TextureHandle, TextureOptions, pos2, vec2,
};
use image::RgbaImage;
use std::{
    collections::HashMap,
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, SystemTime},
};

/// The starter look.css a new look folder gets: 1.x's, every example switched off.
pub const STARTER: &str = include_str!("../../../../src/wwwroot/look-starter.css");
/// The biggest look file String reads, as in 1.x.
const MAX_FILE: u64 = 64 << 20;
/// A picture is kept at most this big on its longest side; the screen gets a copy at the size it's drawn.
const MAX_SIDE: u32 = 3840;
/// How often the worker looks at the look's files.
const POLL: Duration = Duration::from_millis(250);
/// The pictures 2.0 can decode. 1.x also takes .avif, .svg and .ico, which draw nothing here.
const PICTURES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];
/// The sidebar's logo box in String's own look (string.css `.brand`): 62 px tall, padded 20 20 18 20.
const BRAND_HEIGHT: f32 = 62.;
const BRAND_PAD: [f32; 4] = [20., 20., 18., 20.];

// ---------------------------------------------------------------------------------------------------------- reading

/// One `name: value` in a rule. Property names are lower case, except custom ones (`--hi`), which CSS keeps as written.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Decl {
    pub name: String,
    pub value: String,
    pub important: bool,
}
/// One `selectors { declarations }` rule. At-rules (`@media`, `@font-face`, `@import`, …) are skipped whole.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Rule {
    pub selectors: Vec<String>,
    pub decls: Vec<Decl>,
}

/// Comments out, strings kept (a "/*" inside quotes is text); an unclosed comment runs to the end, as in CSS.
fn uncomment(css: &str) -> String {
    let b = css.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let (mut i, mut quote) = (0, 0u8);
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            out.push(c);
            if c == b'\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 2;
                continue;
            }
            if c == quote || c == b'\n' {
                quote = 0;
            }
            i += 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            match css[i + 2..].find("*/") {
                Some(n) => {
                    out.push(b' ');
                    i += n + 4;
                }
                None => break,
            }
        } else {
            if c == b'"' || c == b'\'' {
                quote = c;
            }
            out.push(c);
            i += 1;
        }
    }
    // only whole characters were dropped (comments start and end on ASCII), so this is still UTF-8
    String::from_utf8(out).unwrap_or_default()
}

/// Where the block opened at `open` closes: the index of its '}', or the end of the text (which closes every block).
fn block_end(b: &[u8], open: usize) -> usize {
    let (mut depth, mut quote, mut i) = (0usize, 0u8, open);
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == quote || c == b'\n' {
                quote = 0;
            }
        } else {
            match c {
                b'"' | b'\'' => quote = c,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return i;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    b.len()
}

/// `s` cut at each `sep` outside quotes, brackets and parentheses.
fn split_top(s: &str, sep: u8) -> Vec<&str> {
    let b = s.as_bytes();
    let (mut out, mut start, mut depth, mut quote, mut i) = (Vec::new(), 0, 0i32, 0u8, 0);
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == quote {
                quote = 0;
            }
        } else {
            match c {
                b'"' | b'\'' => quote = c,
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth -= 1,
                _ if c == sep && depth <= 0 => {
                    out.push(&s[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    out.push(&s[start.min(s.len())..]);
    out
}

/// The rules of a stylesheet, in file order. Whatever doesn't parse is skipped, as a browser does.
pub(crate) fn parse(css: &str) -> Vec<Rule> {
    let css = uncomment(css.trim_start_matches('\u{feff}'));
    let b = css.as_bytes();
    let mut rules = Vec::new();
    let mut i = 0;
    while i < b.len() {
        // the prelude runs to a '{' (a rule) or, for a statement at-rule such as @import, a ';'
        let (start, mut quote, mut bad) = (i, 0u8, false);
        while i < b.len() {
            let c = b[i];
            if quote != 0 {
                if c == b'\\' {
                    i += 1;
                } else if c == quote || c == b'\n' {
                    quote = 0;
                }
            } else if c == b'"' || c == b'\'' {
                quote = c;
            } else if c == b'{' || (c == b';' && css[start..i].trim_start().starts_with('@')) {
                break;
            } else if c == b'}' {
                bad = true;
            }
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        if b[i] == b';' {
            i += 1;
            continue;
        }
        let end = block_end(b, i);
        let prelude = css[start..i].trim();
        let body = &css[i + 1..end];
        i = end + 1;
        if bad || prelude.is_empty() || prelude.starts_with('@') {
            continue;
        }
        // one selector it can't read spoils the whole rule, as in CSS
        let Some(selectors) = split_top(prelude, b',')
            .into_iter()
            .map(selector)
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        rules.push(Rule {
            selectors,
            decls: declarations(body),
        });
    }
    rules
}

/// One selector with its spaces made regular: "aside  .brand > *" is "aside .brand>*".
fn selector(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() || s.contains([';', '{', '}']) {
        return None;
    }
    let mut out = String::new();
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if matches!(c, '>' | '+' | '~') {
            space = false;
            out.push(c);
            continue;
        }
        if space && !out.is_empty() && !out.ends_with(['>', '+', '~']) {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    Some(out)
}

fn declarations(body: &str) -> Vec<Decl> {
    let b = body.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut depth, mut quote, mut i) = (0, 0i32, 0u8, 0);
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == quote || c == b'\n' {
                quote = 0;
            }
        } else {
            match c {
                b'"' | b'\'' => quote = c,
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth -= 1,
                // a nested rule: skipped with the selector before it
                b'{' => {
                    i = (block_end(b, i) + 1).min(b.len());
                    start = i;
                    continue;
                }
                b';' if depth <= 0 => {
                    out.extend(decl(&body[start..i]));
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    if start < b.len() {
        out.extend(decl(&body[start..]));
    }
    out
}

fn decl(piece: &str) -> Option<Decl> {
    let (name, value) = piece.split_once(':')?;
    let name = name.trim();
    if name.is_empty()
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c >= 0x80)
    {
        return None;
    }
    let mut value = value.trim();
    let mut important = false;
    if let Some(bang) = value.rfind('!')
        && value[bang + 1..].trim().eq_ignore_ascii_case("important")
    {
        important = true;
        value = value[..bang].trim_end();
    }
    Some(Decl {
        name: if name.starts_with("--") {
            name.to_owned()
        } else {
            name.to_ascii_lowercase()
        },
        value: value.to_owned(),
        important,
    })
}

// -------------------------------------------------------------------------------------------------------- selectors

/// One compound selector, such as `aside#side` or `:root.tint-arc`. `odd` marks what this reader doesn't follow
/// (attributes, pseudo-elements, `:not(…)`), which never matches.
#[derive(Default, Debug)]
struct Compound<'a> {
    tag: Option<&'a str>,
    ids: Vec<&'a str>,
    classes: Vec<&'a str>,
    pseudo: Vec<&'a str>,
    odd: bool,
}
fn compound(s: &str) -> Compound<'_> {
    let b = s.as_bytes();
    let name_end = |mut j: usize| {
        while j < b.len()
            && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'-' | b'_') || b[j] >= 0x80)
        {
            j += 1;
        }
        j
    };
    let mut c = Compound::default();
    let mut i = 0;
    if b.first() == Some(&b'*') {
        c.tag = Some("*");
        i = 1;
    } else {
        let e = name_end(0);
        if e > 0 {
            c.tag = Some(&s[..e]);
            i = e;
        }
    }
    while i < b.len() {
        match b[i] {
            b'#' | b'.' => {
                let e = name_end(i + 1);
                if e == i + 1 {
                    c.odd = true;
                    break;
                }
                if b[i] == b'#' {
                    c.ids.push(&s[i + 1..e]);
                } else {
                    c.classes.push(&s[i + 1..e]);
                }
                i = e;
            }
            b':' if b.get(i + 1) != Some(&b':') => {
                let e = name_end(i + 1);
                c.pseudo.push(&s[i + 1..e]);
                if e == i + 1 || b.get(e) == Some(&b'(') {
                    c.odd = true;
                    break;
                }
                i = e;
            }
            _ => {
                c.odd = true;
                break;
            }
        }
    }
    c
}
impl Compound<'_> {
    /// CSS specificity: ids, then classes and pseudo-classes, then element names.
    fn specificity(&self) -> (u16, u16, u16) {
        (
            self.ids.len() as u16,
            (self.classes.len() + self.pseudo.len()) as u16,
            u16::from(self.tag.is_some_and(|t| t != "*")),
        )
    }
    fn tag_is(&self, name: &str) -> bool {
        self.tag.is_some_and(|t| t.eq_ignore_ascii_case(name))
    }
    fn only_tag(&self, name: &str) -> bool {
        self.tag_is(name)
            && self.ids.is_empty()
            && self.classes.is_empty()
            && self.pseudo.is_empty()
            && !self.odd
    }
    /// The page itself (`:root`, `html`, `.tint-arc`, …): `Some(the tint it needs)`.
    fn root(&self) -> Option<Option<Tint>> {
        if self.odd
            || !self.ids.is_empty()
            || self.tag.is_some_and(|t| !t.eq_ignore_ascii_case("html"))
            || self.pseudo.iter().any(|p| !p.eq_ignore_ascii_case("root"))
            || (self.tag.is_none() && self.pseudo.is_empty() && self.classes.is_empty())
        {
            return None;
        }
        let mut when = None;
        for c in &self.classes {
            let t = match *c {
                "tint-arc" => Tint::Arc,
                "tint-ember" => Tint::Ember,
                // a state of the page 2.0 doesn't keep
                _ => return None,
            };
            if when.is_some_and(|w| w != t) {
                return None;
            }
            when = Some(t);
        }
        Some(when)
    }
    /// Something the page's own elements sit inside, with no condition: the page, the body or the sidebar.
    fn ancestor(&self) -> bool {
        self.root() == Some(None)
            || self.only_tag("body")
            || (!self.odd
                && self.classes.is_empty()
                && self.pseudo.is_empty()
                && (self.tag.is_none() || self.tag_is("aside"))
                && (self.ids == ["side"] || (self.ids.is_empty() && self.tag_is("aside"))))
    }
    fn brand(&self) -> bool {
        !self.odd
            && self.ids.is_empty()
            && self.pseudo.is_empty()
            && self.classes == ["brand"]
            && (self.tag.is_none() || self.tag_is("div"))
    }
    /// The drawn logo: the wire between its pegs (`svg.string-logo`).
    fn wire(&self) -> bool {
        !self.odd
            && self.ids.is_empty()
            && self.pseudo.is_empty()
            && (self.tag.is_none() || self.tag_is("svg"))
            && (self.classes == ["string-logo"] || (self.classes.is_empty() && self.tag_is("svg")))
    }
    fn main(&self) -> bool {
        !self.odd
            && self.classes.is_empty()
            && self.pseudo.is_empty()
            && ((self.ids == ["main"] && (self.tag.is_none() || self.tag_is("main")))
                || (self.ids.is_empty() && self.tag_is("main")))
    }
}

/// What a selector reaches, of what 2.0 draws.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    /// The page itself, while the tint is the one given (if any).
    Root(Option<Tint>),
    /// The body, or every element: what the whole page inherits.
    Body(Option<Tint>),
    Brand,
    /// What's inside the logo box: everything, the wire, or the name.
    Kids,
    Wire,
    Name,
    Main,
}
fn target(sel: &str) -> Option<(Target, (u16, u16, u16))> {
    if sel.contains(['+', '~', '[', '(']) {
        return None;
    }
    let cs: Vec<Compound> = sel
        .split([' ', '>'])
        .filter(|s| !s.is_empty())
        .map(compound)
        .collect();
    let spec = cs.iter().fold((0, 0, 0), |a, c| {
        let s = c.specificity();
        (a.0 + s.0, a.1 + s.1, a.2 + s.2)
    });
    let (last, before) = cs.split_last()?;
    if let Some(k) = cs.iter().position(Compound::brand) {
        if !cs[..k].iter().all(Compound::ancestor) {
            return None;
        }
        let t = match &cs[k + 1..] {
            [] => Target::Brand,
            [x] if x.only_tag("*") => Target::Kids,
            [x] if x.wire() => Target::Wire,
            [x] if x.only_tag("b") => Target::Name,
            _ => return None,
        };
        return Some((t, spec));
    }
    if before.iter().all(Compound::ancestor) {
        if last.wire() && last.classes == ["string-logo"] {
            return Some((Target::Wire, spec));
        }
        if last.main() {
            return Some((Target::Main, spec));
        }
    }
    // the tint: on the page, or on the body or every element, maybe only while Arc or Ember is picked
    let mut when = None;
    for a in before {
        let w = match a.root() {
            Some(w) => w,
            None if a.only_tag("body") => None,
            None => return None,
        };
        if w.is_some() {
            if when.is_some_and(|x| Some(x) != w) {
                return None;
            }
            when = w;
        }
    }
    if let Some(w) = last.root() {
        if !before.is_empty() || (w.is_some() && when.is_some_and(|x| Some(x) != w)) {
            return None;
        }
        return Some((Target::Root(w.or(when)), spec));
    }
    if last.only_tag("body") || last.only_tag("*") {
        return Some((Target::Body(when), spec));
    }
    None
}

// ------------------------------------------------------------------------------------------------------- the values

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Len {
    Px(f32),
    Pct(f32),
}
/// `background-size`: `Sized(None, None)` is `auto`, the picture's own size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Fit {
    Cover,
    Contain,
    Sized(Option<Len>, Option<Len>),
}
/// One background picture and how it's laid out, as CSS does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Layer {
    pub url: String,
    pub fit: Fit,
    pub at: [Len; 2],
    pub repeat: [bool; 2],
    /// `background-origin: content-box`: placed inside the padding.
    pub content: bool,
    /// `background-clip: content-box`: cut at the padding.
    pub clip_content: bool,
}
/// A `--hi` the look sets for the whole page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Hi {
    pub rgb: [u8; 3],
    pub important: bool,
    pub spec: (u16, u16, u16),
    /// Only while this tint is picked (`:root.tint-arc`).
    pub when: Option<Tint>,
    /// Set on the body or every element, so the whole page inherits it whatever the page itself says.
    pub body: bool,
}
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Brand {
    pub picture: Option<Layer>,
    pub height: Option<f32>,
    /// Top, right, bottom, left.
    pub padding: [Option<f32>; 4],
    /// `.brand { display: none }`: no logo box at all.
    pub hidden: bool,
    pub hide_wire: bool,
    pub hide_name: bool,
}
/// What a look.css asks for, of what 2.0 draws.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Spec {
    pub hi: Vec<Hi>,
    pub brand: Brand,
    pub main: Option<Layer>,
}
impl Spec {
    /// The look's tint while String's own is `base`, if a `--hi` of the look's wins over String's own, as the
    /// cascade decides in 1.x: important beats normal, then the more specific selector, then the later one (the
    /// look loads after String's styles, so it wins a tie).
    pub fn tint_over(&self, base: Tint) -> Option<[u8; 3]> {
        let applies = |h: &&Hi| h.when.is_none_or(|w| w == base);
        let rank = |h: &&Hi| (h.important, h.spec);
        if let Some(h) = self
            .hi
            .iter()
            .filter(applies)
            .filter(|h| h.body)
            .max_by_key(rank)
        {
            return Some(h.rgb);
        }
        let own = if matches!(base, Tint::Arc | Tint::Ember) {
            (0, 2, 0)
        } else {
            (0, 1, 0)
        };
        let h = self
            .hi
            .iter()
            .filter(applies)
            .filter(|h| !h.body)
            .max_by_key(rank)?;
        (h.important || h.spec >= own).then_some(h.rgb)
    }
}

/// Important, specificity, then file order: the bigger wins.
type Rank = (bool, (u16, u16, u16), usize);
/// The winning value of each property for one element.
#[derive(Default)]
struct Props(HashMap<String, (Rank, String)>);
const BACKGROUND: [&str; 6] = [
    "background-image",
    "background-position",
    "background-size",
    "background-repeat",
    "background-origin",
    "background-clip",
];
const PADDING: [&str; 4] = [
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
];
impl Props {
    fn set(&mut self, name: &str, value: &str, rank: Rank) {
        if self.0.get(name).is_none_or(|(old, _)| *old <= rank) {
            self.0.insert(name.to_owned(), (rank, value.to_owned()));
        }
    }
    fn add(&mut self, d: &Decl, rank: Rank) {
        match d.name.as_str() {
            "background" => {
                for (name, value) in BACKGROUND.iter().zip(background(&d.value)) {
                    self.set(name, &value, rank);
                }
            }
            "padding" => {
                let w: Vec<&str> = words(&d.value);
                let pick: &[usize] = match w.len() {
                    1 => &[0, 0, 0, 0],
                    2 => &[0, 1, 0, 1],
                    3 => &[0, 1, 2, 1],
                    4 => &[0, 1, 2, 3],
                    _ => return,
                };
                for (name, &k) in PADDING.iter().zip(pick) {
                    self.set(name, w[k], rank);
                }
            }
            n => self.set(n, &d.value, rank),
        }
    }
    fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(|(_, v)| v.trim())
    }
    fn hidden(&self) -> bool {
        self.get("display")
            .is_some_and(|v| v.eq_ignore_ascii_case("none"))
            || self.get("visibility").is_some_and(|v| {
                v.eq_ignore_ascii_case("hidden") || v.eq_ignore_ascii_case("collapse")
            })
            || self.get("opacity").and_then(|v| v.parse::<f32>().ok()) == Some(0.)
    }
    fn layer(&self) -> Option<Layer> {
        let images = split_top(self.get("background-image")?, b',');
        let idx = images.iter().position(|v| url_of(v).is_some())?;
        // the other lists repeat to cover every picture, as in CSS
        let pick = |name: &str, default: &'static str| -> String {
            let list = self
                .get(name)
                .map(|v| split_top(v, b','))
                .unwrap_or_default();
            if list.is_empty() {
                default.to_owned()
            } else {
                list[idx % list.len()].trim().to_ascii_lowercase()
            }
        };
        let clip = pick("background-clip", "border-box");
        Some(Layer {
            url: url_of(images[idx])?,
            fit: fit(&pick("background-size", "auto")),
            at: position(&pick("background-position", "0% 0%")),
            repeat: repeat(&pick("background-repeat", "repeat")),
            content: pick("background-origin", "padding-box") == "content-box",
            clip_content: clip == "content-box",
        })
    }
}

/// The words of a value, with `url(…)` and other functions kept whole and each `/` a word of its own.
fn words(v: &str) -> Vec<&str> {
    let b = v.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut depth, mut quote) = (None::<usize>, 0i32, 0u8);
    for (i, &c) in b.iter().enumerate() {
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
            continue;
        }
        match c {
            b'"' | b'\'' => {
                quote = c;
                start.get_or_insert(i);
            }
            b'(' => {
                depth += 1;
                start.get_or_insert(i);
            }
            b')' => depth -= 1,
            _ if depth > 0 => {}
            _ if c.is_ascii_whitespace() => {
                if let Some(s) = start.take() {
                    out.push(&v[s..i]);
                }
            }
            b'/' => {
                if let Some(s) = start.take() {
                    out.push(&v[s..i]);
                }
                out.push("/");
            }
            _ => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        out.push(&v[s..]);
    }
    out
}
fn is_len(w: &str) -> bool {
    let n = w.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%');
    !n.is_empty() && n.parse::<f32>().is_ok_and(f32::is_finite)
}
/// A length in px, a percentage, or a bare 0. Other units (em, vw, …) are left out.
fn len(w: &str) -> Option<Len> {
    let w = w.trim().to_ascii_lowercase();
    let num = |n: &str| n.parse::<f32>().ok().filter(|v| v.is_finite());
    if let Some(n) = w.strip_suffix("px") {
        num(n).map(Len::Px)
    } else if let Some(n) = w.strip_suffix('%') {
        num(n).map(Len::Pct)
    } else {
        num(&w).filter(|v| *v == 0.).map(Len::Px)
    }
}
fn px(w: &str) -> Option<f32> {
    match len(w)? {
        Len::Px(v) => Some(v),
        Len::Pct(_) => None,
    }
}
fn is_image(w: &str) -> bool {
    let w = w.to_ascii_lowercase();
    w == "none"
        || [
            "url(",
            "image-set(",
            "-webkit-image-set(",
            "image(",
            "cross-fade(",
            "element(",
        ]
        .iter()
        .any(|f| w.starts_with(f))
        || w.contains("gradient(")
}
/// `background`'s parts for each picture, as the six longhands it sets (the colour and attachment aside).
fn background(value: &str) -> [String; 6] {
    let mut lists: [Vec<String>; 6] = Default::default();
    for layer in split_top(value, b',') {
        let (mut image, mut at, mut size, mut rep, mut boxes) =
            (vec![], vec![], vec![], vec![], vec![]);
        let mut sizing = false;
        for w in words(layer) {
            let l = w.to_ascii_lowercase();
            if w == "/" {
                sizing = true;
            } else if is_image(w) {
                image.push(w);
            } else if matches!(
                l.as_str(),
                "repeat" | "no-repeat" | "repeat-x" | "repeat-y" | "space" | "round"
            ) {
                rep.push(w);
            } else if matches!(l.as_str(), "border-box" | "padding-box" | "content-box") {
                boxes.push(w);
            } else if sizing
                && size.len() < 2
                && (is_len(&l) || matches!(l.as_str(), "auto" | "cover" | "contain"))
            {
                size.push(w);
            } else if is_len(&l)
                || matches!(l.as_str(), "left" | "right" | "top" | "bottom" | "center")
            {
                sizing = false;
                at.push(w);
            }
        }
        let or = |p: &[&str], d: &str| {
            if p.is_empty() {
                d.to_owned()
            } else {
                p.join(" ")
            }
        };
        lists[0].push(or(&image, "none"));
        lists[1].push(or(&at, "0% 0%"));
        lists[2].push(or(&size, "auto"));
        lists[3].push(or(&rep, "repeat"));
        lists[4].push(boxes.first().map_or("padding-box", |b| *b).to_owned());
        lists[5].push(
            boxes
                .get(1)
                .or(boxes.first())
                .map_or("border-box", |b| *b)
                .to_owned(),
        );
    }
    lists.map(|l| l.join(", "))
}
/// What's inside `url(…)`, quoted or not; `None` for anything else (a gradient, `none`).
fn url_of(v: &str) -> Option<String> {
    let v = v.trim();
    if v.len() < 5 || !v[..4].eq_ignore_ascii_case("url(") || !v.ends_with(')') {
        return None;
    }
    let inner = v[4..v.len() - 1].trim();
    let inner = ['"', '\'']
        .iter()
        .find_map(|q| inner.strip_prefix(*q).and_then(|s| s.strip_suffix(*q)))
        .unwrap_or(inner);
    (!inner.is_empty()).then(|| inner.to_owned())
}
fn fit(v: &str) -> Fit {
    match v {
        "cover" => Fit::Cover,
        "contain" => Fit::Contain,
        _ => {
            let w = words(v);
            let one = |w: Option<&&str>| w.and_then(|w| len(w));
            match w.len() {
                1 => Fit::Sized(one(w.first()), None),
                2 => Fit::Sized(one(w.first()), one(w.get(1))),
                _ => Fit::Sized(None, None),
            }
        }
    }
}
/// `background-position` with one or two values (keywords, px or %); the four-value form keeps its keywords only.
fn position(v: &str) -> [Len; 2] {
    let kw = |w: &str| match w {
        "left" | "top" => Some(Len::Pct(0.)),
        "center" => Some(Len::Pct(50.)),
        "right" | "bottom" => Some(Len::Pct(100.)),
        _ => len(w),
    };
    let vertical = |w: &str| matches!(w, "top" | "bottom");
    let horizontal = |w: &str| matches!(w, "left" | "right");
    let w = words(v);
    let center = Len::Pct(50.);
    match w.as_slice() {
        [a] if vertical(a) => [center, kw(a).unwrap_or(center)],
        [a] => [kw(a).unwrap_or(Len::Pct(0.)), center],
        [a, b] if vertical(a) || horizontal(b) => {
            [kw(b).unwrap_or(center), kw(a).unwrap_or(center)]
        }
        [a, b] => [kw(a).unwrap_or(Len::Pct(0.)), kw(b).unwrap_or(Len::Pct(0.))],
        _ => {
            let keys: Vec<&&str> = w.iter().filter(|w| !is_len(w)).collect();
            let x = keys.iter().find(|w| horizontal(w)).and_then(|w| kw(w));
            let y = keys.iter().find(|w| vertical(w)).and_then(|w| kw(w));
            [x.unwrap_or(center), y.unwrap_or(center)]
        }
    }
}
fn repeat(v: &str) -> [bool; 2] {
    let one = |w: &str| w != "no-repeat";
    match words(v).as_slice() {
        ["repeat-x"] => [true, false],
        ["repeat-y"] => [false, true],
        [a] => [one(a), one(a)],
        [a, b] => [one(a), one(b)],
        _ => [true, true],
    }
}
/// `r g b` (or with commas), each 0–255 or a percentage, as `rgb(var(--hi))` reads it.
fn rgb(v: &str) -> Option<[u8; 3]> {
    let parts: Vec<&str> = v
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|p| !p.is_empty())
        .collect();
    let [r, g, b] = parts.as_slice() else {
        return None;
    };
    let one = |p: &str| -> Option<u8> {
        let (n, scale) = p.strip_suffix('%').map_or((p, 1f32), |n| (n, 2.55));
        let x: f32 = n.parse().ok().filter(|x: &f32| x.is_finite())?;
        Some((x * scale).round().clamp(0., 255.) as u8)
    };
    Some([one(r)?, one(g)?, one(b)?])
}

/// What a stylesheet's rules ask for, of what 2.0 draws.
pub(crate) fn read(rules: &[Rule]) -> Spec {
    let (mut spec, mut order) = (Spec::default(), 0usize);
    let (mut brand, mut wire, mut name, mut main) = <(Props, Props, Props, Props)>::default();
    for rule in rules {
        for sel in &rule.selectors {
            let Some((t, s)) = target(sel) else {
                continue;
            };
            for d in &rule.decls {
                order += 1;
                let rank = (d.important, s, order);
                match t {
                    Target::Root(when) | Target::Body(when) => {
                        if d.name == "--hi"
                            && let Some(c) = rgb(&d.value)
                        {
                            spec.hi.push(Hi {
                                rgb: c,
                                important: d.important,
                                spec: s,
                                when,
                                body: matches!(t, Target::Body(_)),
                            });
                        }
                    }
                    Target::Brand => brand.add(d, rank),
                    Target::Kids => {
                        wire.add(d, rank);
                        name.add(d, rank);
                    }
                    Target::Wire => wire.add(d, rank),
                    Target::Name => name.add(d, rank),
                    Target::Main => main.add(d, rank),
                }
            }
        }
    }
    spec.brand = Brand {
        picture: brand.layer(),
        height: brand.get("height").and_then(px).map(|h| h.max(0.)),
        padding: PADDING.map(|p| brand.get(p).and_then(px).map(|v| v.max(0.))),
        hidden: brand.hidden(),
        hide_wire: wire.hidden(),
        hide_name: name.hidden(),
    };
    spec.main = main.layer();
    spec
}

// ------------------------------------------------------------------------------------------------------------ files

/// A file the look names, as a path inside `dir`: no scheme, drive or root, no way out with "..", no device name,
/// and a picture's extension. Whether a link is on the way is checked when it's read.
pub(crate) fn inside(dir: &Path, url: &str) -> Option<PathBuf> {
    let url = url.split(['?', '#']).next()?.trim();
    let url = unpercent(url)?;
    if url.is_empty() || url.contains([':', '\0']) || url.starts_with(['/', '\\']) {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for p in url.split(['/', '\\']) {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    const DEVICES: &[&str] = &["con", "prn", "aux", "nul", "conin$", "conout$"];
    for p in &parts {
        let stem = p
            .split('.')
            .next()
            .unwrap_or_default()
            .trim_end()
            .to_ascii_lowercase();
        let numbered = (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit();
        if DEVICES.contains(&stem.as_str()) || numbered {
            return None;
        }
    }
    let ext = Path::new(parts.last()?)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    PICTURES
        .contains(&ext.as_str())
        .then(|| parts.iter().fold(dir.to_path_buf(), |p, s| p.join(s)))
}
fn unpercent(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let (mut out, mut i) = (Vec::with_capacity(b.len()), 0);
    while i < b.len() {
        if b[i] == b'%' {
            out.push(u8::from_str_radix(s.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}
#[cfg(windows)]
fn is_link(m: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    // FILE_ATTRIBUTE_REPARSE_POINT: a symbolic link, a junction, or another kind of redirect
    m.file_attributes() & 0x400 != 0
}
#[cfg(not(windows))]
fn is_link(m: &fs::Metadata) -> bool {
    m.file_type().is_symlink()
}
/// True when `path`, or a folder between `dir` and it, is a link, or one of them can't be checked. `dir` itself is
/// whatever the data folder holds, as in 1.x.
fn through_link(dir: &Path, path: &Path) -> bool {
    let Ok(rest) = path.strip_prefix(dir) else {
        return true;
    };
    let mut p = dir.to_path_buf();
    for c in rest.components() {
        let Component::Normal(c) = c else {
            return true;
        };
        p.push(c);
        match fs::symlink_metadata(&p) {
            Ok(m) if is_link(&m) => return true,
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return false,
            Err(_) => return true,
        }
    }
    false
}
/// A whole look file, opened, read and closed at once (opened sharing read, write and delete, so an editor saving
/// it meanwhile never fails).
fn read_whole(dir: &Path, path: &Path) -> io::Result<Vec<u8>> {
    if through_link(dir, path) {
        return Err(io::Error::other("A link in the look folder"));
    }
    let f = fs::File::open(path)?;
    if !f.metadata()?.is_file() {
        return Err(io::Error::other("Not a file"));
    }
    let mut bytes = Vec::new();
    f.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(io::Error::other("Too big for a look file"));
    }
    Ok(bytes)
}
/// How a file looked: size, time and whether it's a link, or not there. A change in any is a change to read. (Two
/// saves of the same size within one tick of the file system's clock look alike, which no person's saves are.)
#[derive(Debug, Clone, PartialEq)]
struct Stamp(Option<(u64, Option<SystemTime>, bool)>);
fn stamp(p: &Path) -> Stamp {
    Stamp(
        fs::symlink_metadata(p)
            .ok()
            .map(|m| (m.len(), m.modified().ok(), is_link(&m))),
    )
}

/// A decoded picture: kept at most `MAX_SIDE` on its longest side, with the size CSS lays it out at.
pub(crate) struct Picture {
    pub rgba: RgbaImage,
    pub natural: [u32; 2],
}
fn decode(bytes: &[u8]) -> Option<Picture> {
    let mut r = image::ImageReader::new(io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(512 << 20);
    r.limits(limits);
    let rgba = r.decode().ok()?.into_rgba8();
    let natural = [rgba.width(), rgba.height()];
    if natural.contains(&0) {
        return None;
    }
    let long = natural[0].max(natural[1]);
    let rgba = if long > MAX_SIDE {
        let k = MAX_SIDE as f32 / long as f32;
        scale(
            &rgba,
            natural.map(|v| ((v as f32 * k).round() as u32).max(1)),
        )?
    } else {
        rgba
    };
    Some(Picture { rgba, natural })
}
/// `src` at `size`, with alpha taken into account so a logo's soft edges stay clean.
fn scale(src: &RgbaImage, size: [u32; 2]) -> Option<RgbaImage> {
    use fast_image_resize::{
        PixelType, Resizer,
        images::{Image, ImageRef},
    };
    if src.dimensions() == (size[0], size[1]) {
        return Some(src.clone());
    }
    let from = ImageRef::new(src.width(), src.height(), src.as_raw(), PixelType::U8x4).ok()?;
    let mut to = Image::new(size[0], size[1], PixelType::U8x4);
    Resizer::new().resize(&from, &mut to, None).ok()?;
    RgbaImage::from_raw(size[0], size[1], to.into_vec())
}

/// A look as read from its folder, with the files it used and how they looked before they were read.
pub(crate) struct Loaded {
    pub spec: Spec,
    pub logo: Option<Picture>,
    pub main: Option<Picture>,
    watch: Vec<(PathBuf, Stamp)>,
}
impl Loaded {
    fn changed(&self) -> Option<Vec<Stamp>> {
        let now: Vec<Stamp> = self.watch.iter().map(|(p, _)| stamp(p)).collect();
        (!self
            .watch
            .iter()
            .zip(&now)
            .all(|((_, was), now)| was == now))
        .then_some(now)
    }
}
/// Read the look in `dir`. Never fails: whatever can't be read is left out.
pub(crate) fn load(dir: &Path) -> Loaded {
    let css = dir.join("look.css");
    let mut watch = vec![(css.clone(), stamp(&css))];
    let spec = read_whole(dir, &css)
        .map(|b| read(&parse(&String::from_utf8_lossy(&b))))
        .unwrap_or_default();
    let mut picture = |l: &Option<Layer>| {
        let path = inside(dir, &l.as_ref()?.url)?;
        // stamped before it's read, so a save during the read is seen next time
        watch.push((path.clone(), stamp(&path)));
        decode(&read_whole(dir, &path).ok()?)
    };
    let logo = if spec.brand.hidden {
        None
    } else {
        picture(&spec.brand.picture)
    };
    let main = picture(&spec.main);
    Loaded {
        spec,
        logo,
        main,
        watch,
    }
}

/// Makes the look folder with the starter look.css the first time (never over anything already there), and says
/// where look.css is and whether it was just made.
pub(crate) fn make_folder(dir: &Path) -> io::Result<(PathBuf, bool)> {
    let css = dir.join("look.css");
    if fs::symlink_metadata(&css).is_ok() {
        return Ok((css, false));
    }
    fs::create_dir_all(dir)?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&css)
    {
        Ok(mut f) => {
            f.write_all(STARTER.as_bytes())?;
            Ok((css, true))
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok((css, false)),
        Err(e) => Err(e),
    }
}

// ----------------------------------------------------------------------------------------------------------- worker

enum Msg {
    On(bool),
    Reload,
}
/// Reads the look when it's turned on, then looks at its files every `POLL` and reads them again once a change has
/// held still for one look (so a save in progress is read whole). Asleep while the look is off.
fn worker(dir: PathBuf, rx: Receiver<Msg>, tx: Sender<Arc<Loaded>>, ctx: Context) {
    let (mut on, mut last, mut settling) = (false, None::<Arc<Loaded>>, None::<Vec<Stamp>>);
    loop {
        // without a look.css (most people), once a second is enough to see one appear
        let has_css = last
            .as_ref()
            .is_some_and(|l| l.watch.first().is_some_and(|(_, s)| s.0.is_some()));
        let msg = if on {
            rx.recv_timeout(if has_css { POLL } else { POLL * 4 })
        } else {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        let now = match msg {
            Ok(Msg::On(v)) => {
                on = v;
                v
            }
            Ok(Msg::Reload) => on,
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if !on {
            last = None;
            continue;
        }
        if !now && let Some(l) = &last {
            match l.changed() {
                None => {
                    settling = None;
                    continue;
                }
                Some(stamps) if settling.as_ref() != Some(&stamps) => {
                    settling = Some(stamps);
                    continue;
                }
                Some(_) => {}
            }
        }
        settling = None;
        let l = Arc::new(load(&dir));
        last = Some(l.clone());
        if tx.send(l).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

// --------------------------------------------------------------------------------------------------------- the view

struct Tex {
    handle: TextureHandle,
    size: [u32; 2],
}
struct Shown {
    look: Arc<Loaded>,
    logo: Option<Tex>,
    main: Option<Tex>,
}
#[derive(Default)]
struct Runtime {
    tx: Option<Sender<Msg>>,
    rx: Option<Receiver<Arc<Loaded>>>,
    on: bool,
    waited: bool,
    shown: Option<Shown>,
}
static RT: LazyLock<Mutex<Runtime>> = LazyLock::new(Default::default);
/// The look's tint over Mono, Arc and Ember, for `Tint::parse`.
static OVER: Mutex<[Option<[u8; 3]>; 3]> = Mutex::new([None; 3]);

/// The look folder: `<data>\look`. The data folder is read from the command line the way `main` reads it.
pub fn folder() -> Option<PathBuf> {
    static DIR: LazyLock<Option<PathBuf>> =
        LazyLock::new(|| crate::Args::parse().ok().map(|a| a.root.join("look")));
    DIR.clone()
}
/// The look's tint while String's own is `base`, when the look is on and sets one that wins.
pub fn tint(base: Tint) -> Option<[u8; 3]> {
    let i = match base {
        Tint::Mono => 0,
        Tint::Arc => 1,
        Tint::Ember => 2,
        Tint::Custom(_) => return None,
    };
    OVER.lock().ok()?[i]
}
/// Once a frame, before the shell draws: follows "Use my look.css" (`on`) and takes in what the worker has read,
/// starting the worker the first time.
pub fn frame(ctx: &Context, on: bool) {
    let mut rt = RT.lock().unwrap();
    if rt.tx.is_none() {
        let Some(dir) = folder() else {
            return;
        };
        let (tx, msgs) = unbounded();
        let (done, rx) = unbounded();
        let c = ctx.clone();
        if std::thread::Builder::new()
            .name("look".into())
            .spawn(move || worker(dir, msgs, done, c))
            .is_err()
        {
            return;
        }
        rt.tx = Some(tx);
        rt.rx = Some(rx);
    }
    if on != rt.on {
        rt.on = on;
        if let Some(tx) = &rt.tx {
            let _ = tx.send(Msg::On(on));
        }
        if !on {
            rt.shown = None;
            *OVER.lock().unwrap() = [None; 3];
            ctx.request_repaint();
        } else if !rt.waited {
            // the first look of the session: wait a moment for it, so String doesn't open in its own look and swap
            rt.waited = true;
            if let Some(l) = rt
                .rx
                .as_ref()
                .and_then(|rx| rx.recv_timeout(Duration::from_millis(300)).ok())
            {
                rt.show(ctx, l);
            }
        }
    }
    if let Some(l) = rt.rx.as_ref().and_then(|rx| rx.try_iter().last())
        && rt.on
    {
        rt.show(ctx, l);
    }
}
impl Runtime {
    fn show(&mut self, ctx: &Context, look: Arc<Loaded>) {
        *OVER.lock().unwrap() =
            [Tint::Mono, Tint::Arc, Tint::Ember].map(|t| look.spec.tint_over(t));
        // the pictures go to the screen when they're first drawn, at the size they're drawn
        self.shown = Some(Shown {
            look,
            logo: None,
            main: None,
        });
        ctx.request_repaint();
    }
}
/// Read the look again now (after the folder was made).
pub fn reload() {
    if let Some(tx) = &RT.lock().unwrap().tx {
        let _ = tx.send(Msg::Reload);
    }
}
/// Makes the look folder the first time (see `make_folder`) and reads it again.
pub fn open_folder() -> io::Result<(PathBuf, bool)> {
    let dir = folder().ok_or_else(|| io::Error::other("String doesn't know its data folder"))?;
    let r = make_folder(&dir)?;
    reload();
    Ok(r)
}
/// True once the look matches the switch: read and its pictures on screen when on, gone when off.
pub fn settled(on: bool) -> bool {
    let rt = RT.lock().unwrap();
    match (&rt.shown, on && rt.tx.is_some()) {
        (None, want) => !want,
        (Some(s), true) => {
            let b = &s.look.spec.brand;
            (s.look.logo.is_none() || b.hidden || s.logo.is_some())
                && (s.look.main.is_none() || s.main.is_some())
        }
        (Some(_), false) => false,
    }
}

/// The sidebar's logo box this frame: its height, its padding (top, right, bottom, left), and which of String's own
/// logo parts to draw in it.
pub struct BrandBox {
    pub height: f32,
    pub pad: [f32; 4],
    pub wire: bool,
    pub name: bool,
}
pub fn brand() -> BrandBox {
    let own = BrandBox {
        height: BRAND_HEIGHT,
        pad: BRAND_PAD,
        wire: true,
        name: true,
    };
    let rt = RT.lock().unwrap();
    let Some(s) = &rt.shown else {
        return own;
    };
    let b = &s.look.spec.brand;
    if b.hidden {
        return BrandBox {
            height: 0.,
            pad: [0.; 4],
            wire: false,
            name: false,
        };
    }
    let pad = [0, 1, 2, 3].map(|i| b.padding[i].unwrap_or(BRAND_PAD[i]).min(200.));
    // border-box, as String's styles size everything: the height counts the padding, and never less than it
    let height = b
        .height
        .unwrap_or(BRAND_HEIGHT)
        .clamp(pad[0] + pad[2], 240.);
    BrandBox {
        height,
        pad,
        wire: !b.hide_wire,
        name: !b.hide_name,
    }
}
/// The look's logo picture in the logo box `r` (from `brand`).
pub fn paint_brand(p: &Painter, r: Rect, pad: [f32; 4]) {
    let mut rt = RT.lock().unwrap();
    let Some(s) = &mut rt.shown else {
        return;
    };
    let (Some(layer), Some(pic)) = (&s.look.spec.brand.picture, &s.look.logo) else {
        return;
    };
    paint_layer(p, r, pad, layer, pic, &mut s.logo, "look logo");
}
/// The look's picture behind the pages, in `r` (`#main`: right of the sidebar, above the player bar).
pub fn paint_main(p: &Painter, r: Rect) {
    let mut rt = RT.lock().unwrap();
    let Some(s) = &mut rt.shown else {
        return;
    };
    let (Some(layer), Some(pic)) = (&s.look.spec.main, &s.look.main) else {
        return;
    };
    paint_layer(p, r, [0.; 4], layer, pic, &mut s.main, "look background");
}
/// Lays a picture out in a box as CSS's background does: origin and clip box, size, position and repeat.
fn paint_layer(
    p: &Painter,
    border: Rect,
    pad: [f32; 4],
    layer: &Layer,
    pic: &Picture,
    tex: &mut Option<Tex>,
    name: &str,
) {
    let content = Rect::from_min_max(
        pos2(border.left() + pad[3], border.top() + pad[0]),
        pos2(border.right() - pad[1], border.bottom() - pad[2]),
    );
    let area = if layer.content { content } else { border };
    let clip = if layer.clip_content { content } else { border };
    if !(area.width() > 0. && area.height() > 0. && clip.width() > 0. && clip.height() > 0.) {
        return;
    }
    let [w0, h0] = pic.natural.map(|v| v as f32);
    let of = |l: Len, full: f32| match l {
        Len::Px(v) => v,
        Len::Pct(v) => full * v / 100.,
    };
    let size = match layer.fit {
        Fit::Cover | Fit::Contain => {
            let (a, b) = (area.width() / w0, area.height() / h0);
            let k = if layer.fit == Fit::Cover {
                a.max(b)
            } else {
                a.min(b)
            };
            vec2(w0 * k, h0 * k)
        }
        Fit::Sized(Some(w), Some(h)) => vec2(of(w, area.width()), of(h, area.height())),
        Fit::Sized(Some(w), None) => {
            let w = of(w, area.width());
            vec2(w, w * h0 / w0)
        }
        Fit::Sized(None, Some(h)) => {
            let h = of(h, area.height());
            vec2(h * w0 / h0, h)
        }
        Fit::Sized(None, None) => vec2(w0, h0),
    };
    if !(size.x >= 1. && size.y >= 1. && size.x < 1e5 && size.y < 1e5) {
        return;
    }
    let min = area.min
        + vec2(
            of(layer.at[0], area.width() - size.x),
            of(layer.at[1], area.height() - size.y),
        );
    let tile = Rect::from_min_size(min, size);
    let (mut rect, mut uv) = (tile, Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)));
    // repeated: one rectangle over the whole clip box, the texture wrapping
    for axis in [0, 1] {
        if layer.repeat[axis] {
            let (lo, hi, start, len) = (clip.min[axis], clip.max[axis], tile.min[axis], size[axis]);
            if (hi - lo) / len > 4000. {
                return;
            }
            rect.min[axis] = lo;
            rect.max[axis] = hi;
            uv.min[axis] = (lo - start) / len;
            uv.max[axis] = (hi - start) / len;
        }
    }
    // a copy at the size it's drawn (never bigger than the picture), so a big picture drawn small stays smooth;
    // made again only when the size drawn moves by more than a fifth
    let ppp = p.ctx().pixels_per_point();
    let (iw, ih) = pic.rgba.dimensions();
    let want = [
        ((size.x * ppp).round() as u32).clamp(1, iw),
        ((size.y * ppp).round() as u32).clamp(1, ih),
    ];
    let far = |have: [u32; 2]| {
        have.iter()
            .zip(want)
            .any(|(&h, w)| (h as f32 - w as f32).abs() > (w as f32 * 0.2).max(2.))
    };
    if tex.as_ref().is_none_or(|t| far(t.size)) {
        let Some(im) = scale(&pic.rgba, want) else {
            return;
        };
        let opts = if layer.repeat.contains(&true) {
            TextureOptions::LINEAR_REPEAT
        } else {
            TextureOptions::LINEAR
        };
        let image =
            ColorImage::from_rgba_unmultiplied([want[0] as usize, want[1] as usize], im.as_raw());
        *tex = Some(Tex {
            handle: p.ctx().load_texture(name, image, opts),
            size: want,
        });
    }
    if let Some(t) = tex {
        p.with_clip_rect(clip.intersect(p.clip_rect())).image(
            t.handle.id(),
            rect,
            uv,
            Color32::WHITE,
        );
    }
}

#[cfg(test)]
mod tests;
