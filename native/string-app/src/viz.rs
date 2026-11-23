use std::f32::consts::TAU;

use egui::emath::Rot2;
use egui::epaint::Vertex;
use egui::{Color32, Mesh, Painter, Pos2, Shape, TextureId, Vec2, pos2, vec2};
use string_audio::BANDS;

use crate::theme::Tint;

const POINTS: usize = 128;
const STEPS: usize = 3;

struct Spark {
    angle: f32,
    radius: f32,
    speed: f32,
    life: f32,
    decay: f32,
    size: f32,
}

pub struct Viz {
    pub tint: Tint,
    pub level: [f32; BANDS],
    slow: [f32; BANDS],
    energy: f32,
    pulse: f32,
    average: f32,
    last_beat: f32,
    spin: f32,
    velocity: f32,
    time: f32,
    sparks: Vec<Spark>,
    rng: u32,
    unit: [Vec2; POINTS],
}

impl Default for Viz {
    fn default() -> Self {
        Self {
            tint: Tint::Mono,
            level: [0.0; BANDS],
            slow: [0.0; BANDS],
            energy: 0.0,
            pulse: 0.0,
            average: 0.2,
            last_beat: 0.0,
            spin: 0.0,
            velocity: 0.0,
            time: 0.0,
            sparks: Vec::with_capacity(229),
            rng: 0x5232ad41,
            unit: std::array::from_fn(|i| Vec2::angled(TAU * i as f32 / POINTS as f32)),
        }
    }
}

fn random(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
    (*seed >> 8) as f32 / 16777216.0
}

impl Viz {
    pub fn step(&mut self, raw: &[f32; BANDS], playing: bool, dt: f32) {
        let k = (dt * 60.0).min(1.0);
        self.time += dt;
        self.energy = 0.0;
        for (i, (&input, level)) in raw.iter().zip(&mut self.level).enumerate() {
            let target = if playing {
                input.powf(1.5) * (1.0 + i as f32 / 64.0 * 0.55)
            } else {
                0.0
            };
            *level += (target - *level) * if target > *level { 0.55 } else { 0.11 } * k;
            self.slow[i] += (*level - self.slow[i]) * 0.06 * k;
            self.energy += *level / BANDS as f32;
        }
        let bass = self.level[..5].iter().sum::<f32>() / 5.0;
        self.average += (bass - self.average) * 0.02 * k;
        if playing && bass > 0.28 && bass > self.average * 1.32 && self.time - self.last_beat > 0.23
        {
            self.last_beat = self.time;
            self.pulse = 1.0;
            for _ in 0..9 {
                let mut next = || random(&mut self.rng);
                self.sparks.push(Spark {
                    angle: next() * TAU,
                    radius: 1.02 + next() * 0.05,
                    speed: 0.18 + next() * 0.35,
                    life: 1.0,
                    decay: 0.45 + next() * 0.5,
                    size: 0.6 + next() * 1.5,
                });
            }
            self.sparks.drain(..self.sparks.len().saturating_sub(220));
        }
        self.sparks.retain_mut(|s| {
            s.radius += s.speed * dt;
            s.life -= s.decay * dt;
            s.angle += 0.08 * dt;
            s.life > 0.0
        });
        self.pulse = (self.pulse - dt * 3.2).max(0.0);
        let (target, drag) = if playing { (40.0, 1.6) } else { (0.0, 0.9) };
        self.velocity += (target - self.velocity) * (dt * drag).min(1.0);
        self.spin = (self.spin + self.velocity * dt) % 360.0;
    }

    /// The record's angle in degrees: the Now Playing record and the sidebar's logo turn together.
    pub fn spin(&self) -> f32 {
        self.spin
    }

    /// Still turning: playing, or coasting to a stop after a pause.
    pub fn turning(&self) -> bool {
        self.velocity > 0.5
    }

    pub fn settling(&self) -> bool {
        self.energy > 0.002 || self.pulse > 0.0 || !self.sparks.is_empty() || self.velocity > 0.5
    }

    fn ring(&self, c: Pos2, radius: f32, outer: bool) -> Vec<Pos2> {
        let t = self.time;
        let (base, amp, rot, wob, src) = if outer {
            (1.08, 0.55, -t * 0.035, 2.0, &self.slow)
        } else {
            (1.045, 0.42, t * 0.05, 0.0, &self.level)
        };
        let rotation = Rot2::from_angle(rot);
        let knots: [Pos2; POINTS] = std::array::from_fn(|i| {
            let a = TAU * i as f32 / POINTS as f32;
            let b = mirrored(i, POINTS);
            let v = 0.85 * src[b] + 0.15 * src[(b + 1).min(BANDS - 1)];
            let wobble =
                0.012 * (3.0 * a + 0.9 * t + wob).sin() + 0.008 * (5.0 * a - 0.6 * t).sin();
            c + rotation * self.unit[i] * (radius * (base * (1.0 + wobble) + amp * v))
        });
        // The cubic controls from the canvas path. Sample each short section as a single continuous stroke.
        let mut points = Vec::with_capacity(POINTS * STEPS);
        for i in 0..POINTS {
            let p = knots[i];
            let q = knots[(i + 1) % POINTS];
            let a = p + (q - knots[(i + POINTS - 1) % POINTS]) / 6.0;
            let b = q - (knots[(i + 2) % POINTS] - p) / 6.0;
            points.push(p);
            for j in 1..STEPS {
                let u = j as f32 / STEPS as f32;
                let v = 1.0 - u;
                points.push(
                    p + (a - p) * (3.0 * v * v * u)
                        + (b - p) * (3.0 * v * u * u)
                        + (q - p) * (u * u * u),
                );
            }
        }
        points
    }

    pub fn paint(&self, p: &Painter, c: Pos2, r: f32, texture: TextureId) {
        let glow = 0.06 + self.energy * 0.22 + self.pulse * 0.08;
        let mut mesh = Mesh::default();
        for &v in &self.unit {
            mesh.colored_vertex(c + v * (r * 0.9), self.tint.light(glow));
            mesh.colored_vertex(c + v * (r * 1.9), self.tint.light(0.0));
        }
        stitch(&mut mesh, POINTS, 2);
        p.add(Shape::mesh(mesh));
        p.add(Shape::closed_line(
            self.ring(c, r, true),
            (1.0, self.tint.light(0.22)),
        ));
        let edge = self.ring(c, r, false);
        let mut mesh = Mesh::default();
        let fill = self.tint.light(0.05 + self.energy * 0.08);
        for (i, &v) in edge.iter().enumerate() {
            let tangent = edge[(i + 1) % edge.len()] - edge[(i + edge.len() - 1) % edge.len()];
            let normal = vec2(tangent.y, -tangent.x).normalized();
            mesh.colored_vertex(c, fill);
            mesh.colored_vertex(v, fill);
            // Linear interpolation between these samples approximates the canvas shadow blur.
            for (offset, alpha) in [
                (-13.0, 0.0),
                (-5.0, 0.032),
                (0.0, 0.14),
                (5.0, 0.032),
                (13.0, 0.0),
            ] {
                mesh.colored_vertex(v + normal * offset, self.tint.light(alpha));
            }
        }
        for i in 0..edge.len() {
            let a = (i * 7) as u32;
            let b = (((i + 1) % edge.len()) * 7) as u32;
            mesh.add_triangle(a, a + 1, b + 1);
            for j in 2..6 {
                quad(&mut mesh, a + j, b + j);
            }
        }
        p.add(Shape::mesh(mesh));
        p.add(Shape::closed_line(
            edge,
            (1.6, self.tint.light(0.55 + self.energy * 0.4)),
        ));
        for i in 0..96 {
            let v = self.level[mirrored(i, 96)];
            let dir = Vec2::angled(i as f32 * TAU / 96.0 - self.time * 0.0175);
            let start = c + dir * (1.62 * r);
            p.line_segment(
                [start, start + dir * (r * (0.025 + v * 0.16))],
                (
                    1.0,
                    self.tint
                        .light(if i % 8 == 0 { 0.4 } else { 0.14 + v * 0.45 }),
                ),
            );
        }
        for s in &self.sparks {
            p.circle_filled(
                c + Vec2::angled(s.angle) * (s.radius * r),
                s.size,
                self.tint.light(s.life * 0.8),
            );
        }
        self.disc(
            p,
            c,
            r * (1.0 + self.pulse * 0.022 + self.energy * 0.02),
            texture,
        );
    }

    fn disc(&self, p: &Painter, c: Pos2, r: f32, texture: TextureId) {
        // The sleeve casts a soft shadow across the luminous spectrum underneath it.
        let mut shadow = Mesh::default();
        let shadow_center = c + vec2(0.0, r * 0.12);
        for v in self.unit {
            for (radius, opacity) in [(0.86, 175), (1.02, 130), (1.15, 62), (1.32, 13), (1.5, 0)] {
                shadow.colored_vertex(
                    shadow_center + v * (r * radius),
                    Color32::from_black_alpha(opacity),
                );
            }
        }
        for i in 0..POINTS {
            let a = (i * 5) as u32;
            let b = (((i + 1) % POINTS) * 5) as u32;
            for j in 0..4 {
                quad(&mut shadow, a + j, b + j);
            }
        }
        p.add(Shape::mesh(shadow));
        let rot = Rot2::from_angle(-self.spin.to_radians());
        let mut mesh = Mesh::with_texture(texture);
        let mid = pos2(0.5, 0.5);
        mesh.vertices.push(Vertex {
            pos: c,
            uv: mid,
            color: Color32::WHITE,
        });
        for (i, &v) in self.unit.iter().enumerate() {
            mesh.vertices.push(Vertex {
                pos: c + v * r,
                uv: mid + rot * v * 0.5,
                color: Color32::WHITE,
            });
            mesh.add_triangle(0, 1 + i as u32, 1 + ((i + 1) % POINTS) as u32);
        }
        p.add(Shape::mesh(mesh));
        p.circle_stroke(c, r, (1.0, self.tint.ink(0.24)));
        p.circle(
            c,
            r * 0.13,
            Color32::from_rgb(13, 13, 15),
            (1.2, self.tint.ink(0.4)),
        );
        p.circle_stroke(c, r * 0.117, (2.5, self.tint.ink(0.055)));
        p.circle_filled(c, r * 0.04, Color32::BLACK);
    }
}

fn mirrored(i: usize, count: usize) -> usize {
    i.min(count - i) * 2 * (BANDS - 1) / count
}

fn quad(mesh: &mut Mesh, a: u32, b: u32) {
    mesh.add_triangle(a, b, a + 1);
    mesh.add_triangle(a + 1, b, b + 1);
}

fn stitch(mesh: &mut Mesh, n: usize, stride: usize) {
    for i in 0..n {
        quad(mesh, (i * stride) as u32, (((i + 1) % n) * stride) as u32);
    }
}
