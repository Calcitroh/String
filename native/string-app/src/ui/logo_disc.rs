//! String's logo as a small record in the sidebar's logo box. It turns with the Now Playing record while music plays
//! and coasts to a stop on pause (the record's angle, `Viz::spin`), so it only moves when the music does.
use egui::epaint::Vertex;
use egui::{
    Color32, Context, Mesh, Painter, Pos2, Shape, TextureHandle, TextureOptions, Vec2, emath::Rot2,
    pos2,
};
use std::{f32::consts::TAU, sync::Mutex};

const SEGMENTS: usize = 48;

/// The logo made at the disc's size in screen pixels (and made again when the screen's scale changes), so turning it
/// samples about one texel per pixel: a picture shrunk on the fly would shimmer as it turns.
static TEXTURE: Mutex<Option<(u32, TextureHandle)>> = Mutex::new(None);

fn texture(ctx: &Context, px: u32) -> Option<TextureHandle> {
    let mut cache = TEXTURE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((size, t)) = cache.as_ref()
        && *size == px
    {
        return Some(t.clone());
    }
    let logo = image::load_from_memory(include_bytes!("../../assets/String.png"))
        .ok()?
        .into_rgba8();
    let im = image::imageops::resize(&logo, px, px, image::imageops::FilterType::Lanczos3);
    let t = ctx.load_texture(
        "sidebar logo",
        egui::ColorImage::from_rgba_unmultiplied([px as usize; 2], im.as_raw()),
        TextureOptions::LINEAR,
    );
    *cache = Some((px, t.clone()));
    Some(t)
}

/// The disc at `c` with radius `r`: the logo cut round and turned `spin` degrees, with a hairline rim like the
/// record's (which also hides the fan's unsmoothed edge).
pub fn paint(ctx: &Context, p: &Painter, c: Pos2, r: f32, spin: f32, rim: Color32) {
    let px = ((2. * r * ctx.pixels_per_point()).ceil() as u32).clamp(8, 512);
    match texture(ctx, px) {
        Some(texture) => {
            // the same mapping as the Now Playing record, so both turn the same way
            let rot = Rot2::from_angle(-spin.to_radians());
            let mid = pos2(0.5, 0.5);
            let mut mesh = Mesh::with_texture(texture.id());
            mesh.vertices.push(Vertex {
                pos: c,
                uv: mid,
                color: Color32::WHITE,
            });
            for i in 0..SEGMENTS {
                let v = Vec2::angled(TAU * i as f32 / SEGMENTS as f32);
                mesh.vertices.push(Vertex {
                    pos: c + v * r,
                    uv: mid + rot * v * 0.5,
                    color: Color32::WHITE,
                });
                mesh.add_triangle(0, 1 + i as u32, 1 + ((i + 1) % SEGMENTS) as u32);
            }
            p.add(Shape::mesh(mesh));
        }
        None => {
            p.circle_filled(c, r, Color32::from_rgb(13, 13, 15));
        }
    }
    p.circle_stroke(c, r, (1., rim));
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_logo_decodes_and_shrinks_to_any_disc_size() {
        let ctx = egui::Context::default();
        for px in [8, 28, 42, 56, 512] {
            let t = super::texture(&ctx, px).expect("the built-in logo decodes");
            assert_eq!(t.size(), [px as usize; 2]);
        }
        // the same size again reuses the texture rather than making a new one
        let a = super::texture(&ctx, 42).unwrap();
        let b = super::texture(&ctx, 42).unwrap();
        assert_eq!(a.id(), b.id());
    }
}
