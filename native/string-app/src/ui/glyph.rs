use egui::{Color32, Painter, Rect, Shape, Stroke, pos2};
use serde::Deserialize;
use std::{collections::HashMap, sync::LazyLock};
#[derive(Deserialize)]
struct Path {
    points: Vec<[f32; 2]>,
    closed: bool,
    fill: bool,
    width: f32,
}
static GLYPHS: LazyLock<HashMap<String, Vec<Path>>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("glyphs.json")).expect("packaged HUD paths")
});
/// Flattened 24-unit HUD outlines; the destination is in logical points.
pub fn draw(p: &Painter, rect: Rect, name: &str, ink: Color32) {
    let scale = rect.size().min_elem() / 24.;
    let o = rect.center() - egui::Vec2::splat(12. * scale);
    for path in GLYPHS.get(name).unwrap_or(&GLYPHS["box"]) {
        let points: Vec<_> = path
            .points
            .iter()
            .map(|[x, y]| pos2(o.x + x * scale, o.y + y * scale))
            .collect();
        let stroke = Stroke::new(path.width * scale, ink);
        if points.len() < 2 {
            continue;
        }
        if path.fill {
            p.add(Shape::convex_polygon(points, ink, Stroke::NONE));
        } else if path.closed {
            p.add(Shape::closed_line(points, stroke));
        } else if points.len() == 2 && points[0].distance(points[1]) < 0.1 {
            p.circle_filled(points[0], stroke.width * 0.5, ink);
        } else {
            p.add(Shape::line(points, stroke));
        }
    }
}
