use crate::{model::CoverSource, theme::Tint};
use crossbeam_channel::{Receiver, Sender, bounded};
use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use image::{DynamicImage, ImageReader, Rgba, RgbaImage, imageops::FilterType};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    sync::Arc,
    thread,
};
use windows::{
    Storage::{
        FileProperties::{ThumbnailMode, ThumbnailType},
        StorageFile,
        Streams::DataReader,
    },
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    core::HSTRING,
};

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub enum Size {
    Small,
    Medium,
    Large,
}
impl Size {
    fn px(self) -> u32 {
        match self {
            Self::Small => 176,
            Self::Medium => 420,
            Self::Large => 960,
        }
    }
    fn limit(self) -> usize {
        match self {
            Self::Small => 128,
            Self::Medium => 40,
            Self::Large => 4,
        }
    }
}
#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    source: Arc<CoverSource>,
    size: Size,
    tint: Option<Tint>,
}
struct Pixels {
    cover: ColorImage,
    record: Option<ColorImage>,
    ambient: Option<ColorImage>,
}
#[derive(Clone)]
pub struct Art {
    pub cover: TextureHandle,
    pub record: Option<TextureHandle>,
    pub ambient: Option<TextureHandle>,
}
pub struct CoverCache {
    tx: Sender<Key>,
    rx: Receiver<(Key, Pixels)>,
    pending: HashSet<Key>,
    entries: HashMap<Key, (Art, u64)>,
    clock: u64,
}
impl CoverCache {
    pub fn new(ctx: &Context) -> Self {
        let (tx, jobs) = bounded::<Key>(48);
        let (done, rx) = bounded(8);
        for _ in 0..2 {
            let jobs = jobs.clone();
            let done = done.clone();
            let ctx = ctx.clone();
            thread::spawn(move || {
                let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();
                while let Ok(key) = jobs.recv() {
                    let pixels = decode(&key);
                    if done.send((key, pixels)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
                if initialized {
                    unsafe { RoUninitialize() }
                }
            });
        }
        Self {
            tx,
            rx,
            pending: HashSet::new(),
            entries: HashMap::new(),
            clock: 0,
        }
    }
    /// Once per frame. `clock` counts frames, so a cover drawn this frame or the last one is never evicted: a screen
    /// showing more covers than the limit grows the cache instead of thrashing it (decode, evict, decode again).
    pub fn poll(&mut self, ctx: &Context) {
        self.clock += 1;
        for (key, pixels) in self.rx.try_iter() {
            self.pending.remove(&key);
            let upload = |p| ctx.load_texture("cover", p, TextureOptions::LINEAR);
            let art = Art {
                cover: upload(pixels.cover),
                record: pixels.record.map(upload),
                ambient: pixels.ambient.map(upload),
            };
            let recent = self.clock.saturating_sub(1);
            while self.entries.keys().filter(|k| k.size == key.size).count() >= key.size.limit() {
                let Some(old) = self
                    .entries
                    .iter()
                    .filter(|(k, (_, used))| k.size == key.size && *used < recent)
                    .min_by_key(|(_, (_, used))| *used)
                    .map(|(k, _)| k.clone())
                else {
                    break;
                };
                self.entries.remove(&old);
            }
            self.entries.insert(key, (art, self.clock));
        }
    }
    pub fn get(
        &mut self,
        source: &Arc<CoverSource>,
        size: Size,
        tint: Option<Tint>,
    ) -> Option<Art> {
        let key = Key {
            source: source.clone(),
            size,
            tint,
        };
        if let Some((art, last)) = self.entries.get_mut(&key) {
            *last = self.clock;
            return Some(art.clone());
        }
        if !self.pending.contains(&key) && self.tx.try_send(key.clone()).is_ok() {
            self.pending.insert(key);
        }
        None
    }
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
fn win_thumbnail(path: &str, px: u32) -> Option<Vec<u8>> {
    let f = StorageFile::GetFileFromPathAsync(&HSTRING::from(path))
        .ok()?
        .join()
        .ok()?;
    let thumb = f
        .GetThumbnailAsyncOverloadDefaultOptions(ThumbnailMode::MusicView, px)
        .ok()?
        .join()
        .ok()?;
    if thumb.Type().ok()? != ThumbnailType::Image {
        return None;
    }
    let n = u32::try_from(thumb.Size().ok()?)
        .ok()?
        .min(32 * 1024 * 1024);
    let r = DataReader::CreateDataReader(&thumb.GetInputStreamAt(0).ok()?).ok()?;
    let read = r.LoadAsync(n).ok()?.join().ok()?;
    let mut bytes = vec![0; read as usize];
    r.ReadBytes(&mut bytes).ok()?;
    Some(bytes)
}
fn image_bytes(bytes: Vec<u8>) -> Option<DynamicImage> {
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()
}
fn file_image(path: &str) -> Option<DynamicImage> {
    if path.is_empty() {
        return None;
    }
    ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()
}
fn embedded(s: &CoverSource, px: u32) -> Option<DynamicImage> {
    if s.embedded.is_empty() {
        return None;
    }
    if let Some((off, n)) = s.offset
        && n <= 32 * 1024 * 1024
    {
        let mut f = File::open(&s.embedded).ok()?;
        f.seek(SeekFrom::Start(off)).ok()?;
        let mut bytes = Vec::new();
        f.take(n).read_to_end(&mut bytes).ok()?;
        if let Some(im) = image_bytes(bytes) {
            return Some(im);
        }
    }
    // Opus and Ogg songs (the picture is base64 inside a comment) and anything the offsets missed; Windows' own
    // thumbnail last, since it has no reader for Opus covers.
    string_core::tags::picture(std::path::Path::new(&s.embedded))
        .ok()
        .flatten()
        .and_then(image_bytes)
        .or_else(|| win_thumbnail(&s.embedded, px).and_then(image_bytes))
}
fn pattern(seed: &str) -> DynamicImage {
    let h = seed
        .encode_utf16()
        .fold(2166136261u32, |a, b| (a ^ b as u32).wrapping_mul(16777619));
    let offset = (h >> 8) as f32 / u32::MAX as f32 * 256.;
    let f = (h % 17) as f32;
    DynamicImage::ImageRgba8(RgbaImage::from_fn(176, 176, |x, y| {
        let x = x as f32;
        let y = y as f32;
        let d = (x - 70. - offset * 30.).hypot(y - 80. + f);
        let line = match h % 5 {
            0 => (d / 7.).fract() < 0.13,
            1 => (x / 11.).fract() < 0.07 || (y / 11.).fract() < 0.07,
            2 => (x % 12. - 6.).hypot(y % 12. - 6.) < (4. - d / 48.).max(0.8),
            3 => ((x + y * 0.5 + f) / 9.).fract() < 0.26,
            _ => {
                (d / 10.).fract() < 0.12
                    && ((y - 80.).atan2(x - 80.) * 3. + (d / 10.).floor()).sin() > -0.3
            }
        };
        let v = if line { 61 } else { 12 };
        Rgba([v, v, v + 1, 255])
    }))
}
fn color(im: &RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([im.width() as usize, im.height() as usize], im.as_raw())
}
pub(crate) fn source_image(s: &CoverSource, px: u32) -> DynamicImage {
    file_image(&s.file)
        .or_else(|| embedded(s, px))
        .or_else(|| file_image(&s.fallback))
        .unwrap_or_else(|| pattern(&s.seed))
}
fn decode(key: &Key) -> Pixels {
    let px = key.size.px();
    let input = source_image(&key.source, px).into_rgba8();
    use fast_image_resize::{PixelType, ResizeOptions, Resizer, images::Image};
    let src = Image::from_vec_u8(
        input.width(),
        input.height(),
        input.into_raw(),
        PixelType::U8x4,
    )
    .expect("RGBA source buffer");
    let mut dst = Image::new(px, px, PixelType::U8x4);
    Resizer::new()
        .resize(
            &src,
            &mut dst,
            &ResizeOptions::new().fit_into_destination(None),
        )
        .expect("matching RGBA formats");
    let mut im = RgbaImage::from_raw(px, px, dst.into_vec()).expect("RGBA thumbnail buffer");
    if let Some(tint) = key.tint {
        let [r, g, b] = tint.rgb();
        for p in im.pixels_mut() {
            let v = (0.213 * p[0] as f32 + 0.715 * p[1] as f32 + 0.072 * p[2] as f32) / 242.;
            p.0 = [
                (v * r as f32) as u8,
                (v * g as f32) as u8,
                (v * b as f32) as u8,
                p[3],
            ];
        }
    }
    let mut result = Pixels {
        cover: color(&im),
        record: None,
        ambient: None,
    };
    if key.size == Size::Large {
        let mut bg = image::imageops::blur(
            &image::imageops::resize(&im, 160, 96, FilterType::Triangle),
            13.,
        );
        for (x, y, p) in bg.enumerate_pixels_mut() {
            let d = ((x as f32 / 159. - 0.5) / 0.7).hypot((y as f32 / 95. - 0.45) / 0.63);
            for c in &mut p.0[..3] {
                *c = (5. + *c as f32 * 0.144 * (1. - 0.78 * d.min(1.))) as u8;
            }
        }
        result.ambient = Some(color(&bg));
        for (x, y, p) in im.enumerate_pixels_mut() {
            let dx = (x as f32 + 0.5) * 2. / px as f32 - 1.;
            let dy = (y as f32 + 0.5) * 2. / px as f32 - 1.;
            let r = dx.hypot(dy);
            let groove = ((r * 49.).fract() - 0.78).abs();
            let edge = ((r - 0.65) / 0.35).clamp(0., 1.);
            let gain = (1. - 0.13 * (1. - (groove / 0.17).min(1.))) * (1. - 0.22 * edge * edge);
            let sheen = (2. * dy.atan2(dx) - 0.4).cos().max(0.).powi(4) * 7.;
            for c in &mut p.0[..3] {
                *c = (*c as f32 * gain + sheen) as u8;
            }
        }
        result.record = Some(color(&im));
    }
    result
}
