#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Route {
    #[default]
    Home,
    Songs,
    Albums,
    Artists,
    Album(String),
    Artist(String),
    Playlist(String),
    Liked,
    Search,
    Stats,
    Settings,
    Dupes,
}
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn decode(s: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut bs = s.bytes();
    while let Some(b) = bs.next() {
        if b == b'%' {
            let n = |v: u8| char::from(v).to_digit(16).map(|d| d as u8);
            out.push(16 * n(bs.next()?)? + n(bs.next()?)?);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}
impl Route {
    pub fn parse(s: &str) -> Self {
        if let Some((kind, key)) = s.split_once('/')
            && let Some(key) = decode(key)
        {
            return match kind {
                "album" => Self::Album(key),
                "artist" => Self::Artist(key),
                "pl" => Self::Playlist(key),
                _ => Self::Home,
            };
        }
        match s {
            "songs" => Self::Songs,
            "albums" => Self::Albums,
            "artists" => Self::Artists,
            "liked" => Self::Liked,
            "search" => Self::Search,
            "stats" => Self::Stats,
            "settings" => Self::Settings,
            "dupes" => Self::Dupes,
            _ => Self::Home,
        }
    }
    pub fn path(&self) -> String {
        match self {
            Self::Album(k) => format!("album/{}", encode(k)),
            Self::Artist(k) => format!("artist/{}", encode(k)),
            Self::Playlist(k) => format!("pl/{}", encode(k)),
            v => v.title().to_lowercase(),
        }
    }
    pub fn title(&self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Songs => "Songs",
            Self::Albums => "Albums",
            Self::Artists => "Artists",
            Self::Album(_) => "Album",
            Self::Artist(_) => "Artist",
            Self::Playlist(_) => "Playlist",
            Self::Liked => "Liked",
            Self::Search => "Search",
            Self::Stats => "Stats",
            Self::Settings => "Settings",
            Self::Dupes => "Dupes",
        }
    }
}
#[derive(Default)]
pub struct History {
    pub current: Route,
    back: Vec<Route>,
    forward: Vec<Route>,
}
impl History {
    pub fn go(&mut self, next: Route) {
        if next != self.current {
            self.back.push(std::mem::replace(&mut self.current, next));
            self.forward.clear()
        }
    }
    pub fn back(&mut self) {
        if let Some(r) = self.back.pop() {
            self.forward.push(std::mem::replace(&mut self.current, r));
        }
    }
    pub fn forward(&mut self) {
        if let Some(r) = self.forward.pop() {
            self.back.push(std::mem::replace(&mut self.current, r));
        }
    }
    pub fn has_back(&self) -> bool {
        !self.back.is_empty()
    }
    pub fn has_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}
