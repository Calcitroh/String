use crate::{model::CoverSource, ui::covers::source_image};
use crossbeam_channel::{Sender, bounded};
use egui::{
    Color32, ColorImage, Context, Id, Mesh, Rect, TextureHandle, TextureOptions, Vec2, pos2,
};
use std::{
    collections::VecDeque,
    sync::{Arc, OnceLock},
    thread,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};

// The two treatments share the decode, downscale and blur. Only the final colour filter differs.
type Images = [TextureHandle; 2];
type Slot = Arc<OnceLock<Images>>;
struct Job {
    source: Arc<CoverSource>,
    extent: Vec2,
    slot: Slot,
    ctx: Context,
}
#[derive(Clone)]
struct Cache {
    jobs: Sender<Job>,
    entries: VecDeque<(Arc<CoverSource>, Slot)>,
}
impl Default for Cache {
    fn default() -> Self {
        let (jobs, worker) = bounded::<Job>(1);
        thread::spawn(move || {
            let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();
            for job in worker {
                let src = source_image(&job.source, 176)
                    .thumbnail(128, 128)
                    .to_rgba8();
                let scale = ((job.extent.x + 80.) / src.width() as f32)
                    .max((job.extent.y + 80.) / src.height() as f32);
                let blur = image::imageops::blur(&src, 60. / scale);
                let images = [false, true].map(|mono| {
                    let mut pixels = blur.clone();
                    for p in pixels.pixels_mut() {
                        let luma =
                            0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
                        for c in &mut p.0[..3] {
                            *c = if mono {
                                luma * 0.26
                            } else {
                                (luma + *c as f32) * 0.15
                            }
                            .round() as u8;
                        }
                    }
                    let size = [pixels.width() as usize, pixels.height() as usize];
                    job.ctx.load_texture(
                        "hero glow",
                        ColorImage::from_rgba_unmultiplied(size, &pixels),
                        TextureOptions::LINEAR,
                    )
                });
                let _ = job.slot.set(images);
                job.ctx.request_repaint();
            }
            if initialized {
                unsafe { RoUninitialize() }
            }
        });
        Self {
            jobs,
            entries: VecDeque::new(),
        }
    }
}

pub fn texture(
    ctx: &Context,
    source: &Arc<CoverSource>,
    extent: Vec2,
    mono: bool,
) -> Option<TextureHandle> {
    ctx.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<Cache>(Id::unique("hero textures"));
        // Keep the current cover at the front during playback and scrolling.
        let found = cache
            .entries
            .iter()
            .position(|(s, _)| Arc::ptr_eq(s, source) || s == source);
        match found {
            Some(0) => {}
            Some(i) => {
                let entry = cache.entries.remove(i)?;
                cache.entries.push_front(entry);
            }
            None => {
                let slot = Slot::default();
                cache
                    .jobs
                    .try_send(Job {
                        source: source.clone(),
                        extent,
                        slot: slot.clone(),
                        ctx: ctx.clone(),
                    })
                    .ok()?;
                cache.entries.truncate(7);
                cache.entries.push_front((source.clone(), slot));
            }
        }
        cache
            .entries
            .front()?
            .1
            .get()
            .map(|pair| pair[usize::from(mono)].clone())
    })
}

// background-size: cover, centered in the -40px inset, then enlarged by 1.2.
fn uv_rect(extent: Vec2, image: Vec2) -> Rect {
    let scale = 1.2 * ((extent.x + 80.) / image.x).max((extent.y + 80.) / image.y);
    Rect::from_center_size(pos2(0.5, 0.5), extent / (image * scale))
}
pub fn mesh(r: Rect, img: &TextureHandle, opacity: f32) -> Mesh {
    let uv = uv_rect(r.size(), img.size_vec2());
    let mut mesh = Mesh::with_texture(img.id());
    // Compositing the page-coloured scrim over this glow is equivalent to reducing its alpha.
    for (stop, scrim) in [(0., 0.1), (0.6, 0.55), (1., 1.)] {
        let y = egui::lerp(r.y_range(), stop);
        let v = egui::lerp(uv.y_range(), stop);
        let color = Color32::from_white_alpha((255. * opacity * 0.9 * (1. - scrim)).round() as u8);
        for (x, u) in [(r.left(), uv.left()), (r.right(), uv.right())] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: pos2(x, y),
                uv: pos2(u, v),
                color,
            });
        }
    }
    mesh.indices
        .extend_from_slice(&[0, 1, 2, 1, 3, 2, 2, 3, 4, 3, 5, 4]);
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_covers_without_stretching() {
        for src in [
            Vec2::splat(128.),
            egui::vec2(128., 40.),
            egui::vec2(36., 128.),
        ] {
            let view = egui::vec2(1148., 334.);
            let uv = uv_rect(view, src);
            assert_eq!(uv.center(), pos2(0.5, 0.5));
            let sample = uv.size() * src;
            assert!((sample.x / sample.y - view.x / view.y).abs() < 0.0001);
            assert!(uv.left() > 0. && uv.top() > 0.);
        }
    }
}
