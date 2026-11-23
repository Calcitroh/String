use egui::{Color32, Context, FontData, FontDefinitions, FontFamily, FontId, RichText};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
pub mod look;
pub const BG: Color32 = Color32::from_rgb(5, 5, 6);
pub const BG2: Color32 = Color32::from_rgb(8, 8, 10);
pub const PANEL: Color32 = Color32::from_rgb(11, 11, 13);
pub const PANEL2: Color32 = Color32::from_rgb(16, 16, 19);
pub const RAISED: Color32 = Color32::from_rgb(22, 22, 26);
pub const TEXT: Color32 = Color32::from_rgb(233, 233, 236);
pub const SECONDARY: Color32 = Color32::from_rgb(201, 201, 207);
pub const MUTED: Color32 = Color32::from_rgb(142, 142, 150);
pub const FAINT: Color32 = Color32::from_rgb(90, 90, 98);
pub const GHOST: Color32 = Color32::from_rgb(58, 58, 65);
pub const HAIR: Color32 = Color32::from_rgba_premultiplied(18, 18, 18, 18);
pub const HAIR2: Color32 = Color32::from_rgba_premultiplied(31, 31, 31, 31);
pub const HAIR3: Color32 = Color32::from_rgba_premultiplied(56, 56, 56, 56);
pub const SIDE: f32 = 236.;
pub const BAR: f32 = 84.;
pub const ROW: f32 = 52.;
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tint {
    #[default]
    Mono,
    Arc,
    Ember,
    /// The person's own, from `--hi` in their look.css (see `look`).
    Custom([u8; 3]),
}
impl Tint {
    /// The tint to draw with for the `tint` setting: Mono, Arc or Ember, unless the person's look.css (while "Use
    /// my look.css" is on) sets a `--hi` that wins over it, as it would in 1.x.
    pub fn parse(value: Option<&str>) -> Self {
        let own = match value {
            Some("arc") => Self::Arc,
            Some("ember") => Self::Ember,
            _ => Self::Mono,
        };
        look::tint(own).map_or(own, Self::Custom)
    }
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Self::Mono => [236, 238, 242],
            Self::Arc => [120, 220, 255],
            Self::Ember => [255, 190, 112],
            Self::Custom(rgb) => rgb,
        }
    }
    pub fn ink(self, opacity: f32) -> Color32 {
        let [r, g, b] = self.rgb();
        Color32::from_rgba_unmultiplied(r, g, b, (opacity * 255.) as u8)
    }
    pub fn light(self, opacity: f32) -> Color32 {
        let [r, g, b] = self.rgb().map(|v| (v as f32 * opacity) as u8);
        Color32::from_rgb_additive(r, g, b)
    }
}
pub fn font(face: &str, size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(face.into()))
}
pub fn hud(text: &str, size: f32) -> RichText {
    RichText::new(text)
        .font(font("HUD", size))
        .extra_letter_spacing(size * 0.16)
}
pub fn mono(text: &str, size: f32) -> RichText {
    RichText::new(text).font(font("Mono", size))
}
/// The player bar and the sidebar: rgb(4 4 5) over the page, as in the web version.
pub const DEEP: Color32 = Color32::from_rgb(4, 4, 5);
/// The faint bright line a lit edge or a text glow uses: white at `alpha` (0–1), premultiplied.
pub fn white(alpha: f32) -> Color32 {
    Color32::from_white_alpha((alpha * 255.) as u8)
}
pub fn install(ctx: &Context) {
    use egui::epaint::text::{FontTweak, VariationCoords};
    let dir = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()))
        .join("Fonts");
    let mut defs = FontDefinitions::default();
    // Japanese (with its punctuation, such as 『』) and symbols come right after String's own fonts, so titles in
    // other scripts render as they do in the web version instead of as boxes.
    // Yu Gothic's letters land about a third of an em above the baseline of the text around them (measured in
    // String's own screenshots against Segoe UI and Cascadia Mono), so they're moved down by that much.
    let mut load = |name: &str, files: &[&str], drop: f32| {
        let bytes = files.iter().find_map(|f| std::fs::read(dir.join(f)).ok())?;
        let data = FontData::from_owned(bytes).tweak(FontTweak {
            y_offset_factor: drop,
            ..Default::default()
        });
        defs.font_data.insert(name.into(), Arc::new(data));
        Some(name.to_owned())
    };
    let jp = load("JP", &["YuGothM.ttc", "YuGothR.ttc", "msgothic.ttc"], 0.3);
    let jp_bold = load("JP-Bold", &["YuGothB.ttc"], 0.3).or_else(|| jp.clone());
    let symbols = load("Symbols", &["seguisym.ttf"], 0.);
    // Each face at the weight the web version asks for. Segoe UI Variable and Bahnschrift are variable fonts whose
    // default is regular (400); without the weight set, HUD labels drew thinner than in the web version. A file is read
    // once and shared by every weight drawn from it; older Windows without the variable fonts gets the static ones.
    let mut files: HashMap<&str, Option<egui::epaint::text::Blob>> = HashMap::new();
    let mut blob = |f: &'static str| {
        files
            .entry(f)
            .or_insert_with(|| {
                std::fs::read(dir.join(f))
                    .ok()
                    .map(|b| Arc::new(b) as egui::epaint::text::Blob)
            })
            .clone()
    };
    for (family, choices) in [
        ("UI", &[("SegUIVar.ttf", 400., 10.), ("segoeui.ttf", 0., 0.)][..]),
        ("Medium", &[("SegUIVar.ttf", 500., 10.), ("segoeui.ttf", 0., 0.)][..]),
        ("Strong", &[("SegUIVar.ttf", 600., 10.), ("seguisb.ttf", 0., 0.)][..]),
        ("Bold", &[("SegUIVar.ttf", 700., 36.), ("segoeuib.ttf", 0., 0.)][..]),
        ("HUD", &[("bahnschrift.ttf", 600., 0.), ("seguisb.ttf", 0., 0.)][..]),
        ("Mono", &[("CascadiaMono.ttf", 0., 0.), ("consola.ttf", 0., 0.)][..]),
    ] {
        let mut list = defs.families[&FontFamily::Proportional].clone();
        let fallback = if matches!(family, "Strong" | "Bold") {
            &jp_bold
        } else {
            &jp
        };
        for extra in [symbols.as_ref(), fallback.as_ref()].into_iter().flatten() {
            list.insert(0, extra.clone());
        }
        if let Some((bytes, weight, optical)) = choices
            .iter()
            .find_map(|&(f, w, o)| blob(f).map(|b| (b, w, o)))
        {
            let mut coords = VariationCoords::default();
            if weight > 0. {
                coords.push(b"wght", weight);
            }
            if optical > 0. {
                coords.push(b"opsz", optical);
            }
            let data = FontData::from_blob(bytes, 0).tweak(FontTweak {
                coords,
                ..Default::default()
            });
            defs.font_data.insert(family.into(), Arc::new(data));
            list.insert(0, family.into());
        }
        if family == "UI" {
            defs.families.insert(FontFamily::Proportional, list.clone());
        }
        defs.families.insert(FontFamily::Name(family.into()), list);
    }
    ctx.set_fonts(defs);
    ctx.set_theme(egui::Theme::Dark);
    ctx.global_style_mut(|s| {
        s.visuals.panel_fill = BG;
        s.visuals.window_fill = PANEL;
        s.visuals.extreme_bg_color = BG2;
        s.visuals.faint_bg_color = PANEL2;
        s.visuals.widgets.inactive.bg_fill = RAISED;
        s.visuals.widgets.noninteractive.bg_stroke.color = HAIR2;
        s.visuals.widgets.noninteractive.fg_stroke.color = TEXT;
        s.visuals.selection.bg_fill = GHOST;
        s.visuals.override_text_color = Some(TEXT);
        s.spacing.item_spacing = egui::vec2(8., 6.);
    });
}
pub fn time(sec: f64) -> String {
    let s = sec.max(0.) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
