use crate::route::Route;
use egui::{Context, Id, TextureId, cache::FramePublisher};
use std::collections::HashMap;
use windows::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};

#[derive(Clone)]
struct Page {
    route: Route,
    entered: Option<f64>,
    animate: bool,
}
#[derive(Clone, Default)]
struct Loaded {
    first_seen: HashMap<TextureId, f64>,
    next_sweep: usize,
}

fn animations_enabled() -> bool {
    let mut enabled = 1i32;
    unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut enabled).cast()),
            Default::default(),
        )
        .is_err()
            || enabled != 0
    }
}

#[derive(Clone, Copy)]
pub struct Motion {
    enabled: bool,
    now: f64,
    entered: Option<f64>,
}
#[derive(Clone, Copy)]
struct Hover {
    on: bool,
    at: f64,
    from: [f32; 3],
}
impl Hover {
    fn sample(self, now: f64) -> [f32; 3] {
        let elapsed = (now - self.at) as f32;
        let target = f32::from(self.on);
        let weight =
            |duration: f32, curve: fn(f32) -> f32| curve((elapsed / duration).clamp(0., 1.));
        [
            egui::lerp(self.from[0]..=target, weight(0.25, ease)),
            egui::lerp(self.from[1]..=target, weight(0.35, glide)),
            egui::lerp(self.from[2]..=target, weight(0.6, glide)),
        ]
    }
}
impl Motion {
    pub fn for_page(ctx: &Context, route: &Route) -> Self {
        let now = ctx.time();
        ctx.data_mut(|d| {
            let page = d.get_temp_mut_or_insert_with(Id::unique("library motion"), || Page {
                route: route.clone(),
                entered: None,
                animate: animations_enabled(),
            });
            if &page.route != route {
                page.route = route.clone();
                page.entered = Some(now);
                page.animate = animations_enabled();
            }
            Self {
                enabled: page.animate,
                now,
                entered: page.entered,
            }
        })
    }

    pub fn page_progress(self, ctx: &Context) -> f32 {
        self.entered
            .map_or(1., |at| self.progress(ctx, self.now - at, 0.42, glide))
    }

    // egui evicts absent keys after a frame. Long sessions never retain a history of visited cards.
    pub fn age(self, ctx: &Context, key: Id) -> f64 {
        if !self.enabled {
            return f64::INFINITY;
        }
        ctx.memory_mut(|m| {
            let visible = m.caches.cache::<FramePublisher<Id, f64>>();
            let start = visible.get(&key).copied().unwrap_or(self.now);
            visible.set(key, start);
            self.now - start
        })
    }
    pub fn reveal(self, ctx: &Context, tex: TextureId, duration: f64) -> f32 {
        if !self.enabled {
            return 1.;
        }
        let slot = Id::unique("loaded library covers");
        let start = ctx.data_mut(|d| {
            d.get_temp_mut_or_default::<Loaded>(slot)
                .first_seen
                .get(&tex)
                .copied()
        });
        let start = start.unwrap_or_else(|| {
            let manager = ctx.tex_manager();
            ctx.data_mut(|d| {
                let loaded = d.get_temp_mut_or_default::<Loaded>(slot);
                if loaded.first_seen.len() >= loaded.next_sweep {
                    let live = manager.read();
                    loaded.first_seen.retain(|id, _| live.meta(*id).is_some());
                    loaded.next_sweep = loaded.first_seen.len() + 64;
                }
                loaded.first_seen.insert(tex, self.now);
            });
            self.now
        });
        self.progress(ctx, self.now - start, duration, ease)
    }
    pub fn card(self, ctx: &Context, group_age: f64, index: usize, tile: bool) -> f32 {
        let delay = if tile {
            0.
        } else {
            index.min(14) as f64 * 0.028
        };
        self.progress(ctx, group_age - delay, 0.5, glide)
    }
    pub fn hover(self, ctx: &Context, id: Id, on: bool, tile: bool) -> [f32; 3] {
        let settled = [f32::from(on); 3];
        if !self.enabled {
            return settled;
        }
        let transition = ctx.memory_mut(|m| {
            let active = m.caches.cache::<FramePublisher<Id, Hover>>();
            let state = match active.get(&id).copied() {
                Some(previous) if previous.on != on => Hover {
                    on,
                    at: self.now,
                    from: previous.sample(self.now),
                },
                Some(previous) => previous,
                None if on => Hover {
                    on,
                    at: self.now,
                    from: [0.; 3],
                },
                None => return None,
            };
            let moving = self.now - state.at < if tile { 0.25 } else { 0.6 };
            if moving || on {
                active.set(id, state);
            }
            moving.then_some(state)
        });
        if let Some(state) = transition {
            ctx.request_repaint();
            state.sample(self.now)
        } else {
            settled
        }
    }
    fn progress(self, ctx: &Context, elapsed: f64, duration: f64, curve: fn(f32) -> f32) -> f32 {
        if !self.enabled || elapsed >= duration {
            return 1.;
        }
        if elapsed < 0. {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(-elapsed));
            return 0.;
        }
        ctx.request_repaint();
        curve((elapsed / duration) as f32)
    }
}

fn bezier(t: f32, a: f32, b: f32) -> f32 {
    let u = 1. - t;
    3. * u * t * (a * u + b * t) + t * t * t
}
// CSS advances on the curve's x axis. Both timing functions have a strictly positive x derivative.
fn css_curve(x: f32, x1: f32, y1: f32, x2: f32) -> f32 {
    if x <= 0. || x >= 1. {
        return x;
    }
    let mut t = x;
    for _ in 0..5 {
        let u = 1. - t;
        let dx = 3. * (u * u * x1 + 2. * u * t * (x2 - x1) + t * t * (1. - x2));
        t = (t - (bezier(t, x1, x2) - x) / dx).clamp(0., 1.);
    }
    bezier(t, y1, 1.)
}
fn glide(x: f32) -> f32 {
    css_curve(x, 0.22, 0.9, 0.25)
}
fn ease(x: f32) -> f32 {
    css_curve(x, 0.25, 0.1, 0.25)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stagger_and_late_texture_finish_without_an_idle_repaint() {
        let ctx = Context::default();
        let frame = |time, loaded| {
            let mut values = [0.; 4];
            let out = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ui| {
                    let ctx = ui.ctx();
                    let motion = Motion {
                        enabled: true,
                        now: ctx.time(),
                        entered: Some(0.),
                    };
                    let age = motion.age(ctx, Id::unique("grid"));
                    values = [
                        motion.page_progress(ctx),
                        motion.card(ctx, age, 42, false),
                        motion.card(ctx, age, 42, true),
                        if loaded {
                            motion.reveal(ctx, TextureId::Managed(42), 0.45)
                        } else {
                            0.
                        },
                    ];
                },
            );
            let next = out.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            out.drop_without_applying_deltas();
            (values, next)
        };
        frame(0., false);
        let (mid, _) = frame(0.35, false);
        assert!(mid[0] > 0. && mid[0] < 1.);
        assert_eq!(mid[1], 0.);
        assert!(mid[2] > 0. && mid[2] < 1.);
        assert_eq!(frame(1., true).0, [1., 1., 1., 0.]);
        let fade = frame(1.225, true).0[3];
        assert!((fade - ease(0.5)).abs() < 0.00001);
        for t in [1.5, 1.51, 1.52] {
            assert_eq!(frame(t, true).0, [1.; 4]);
        }
        frame(4., false);
        frame(5., false);
        let (revisited, next) = frame(10., true);
        assert_eq!(revisited[3], 1.);
        assert_eq!(next, std::time::Duration::MAX);
    }

    #[test]
    fn css_time_uses_the_horizontal_coordinate() {
        for (curve, a, b, c) in [
            (glide as fn(f32) -> f32, 0.22, 0.9, 0.25),
            (ease, 0.25, 0.1, 0.25),
        ] {
            for i in 0..=100 {
                let t = i as f32 / 100.;
                assert!((curve(bezier(t, a, c)) - bezier(t, b, 1.)).abs() < 0.00001);
            }
        }
    }
}
