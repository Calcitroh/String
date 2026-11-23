//! The taskbar buttons' glyphs (previous, play, pause, next) as App.cs drew them: shapes on a 16-unit square with
//! smoothed edges, in one ink (light on a dark taskbar, dark on a light one), made into icons at the screen's size.

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Glyph {
    Previous,
    Play,
    Pause,
    Next,
}

/// App.cs's ink: near-white on the dark taskbar, near-black on a light one.
pub(crate) fn ink(light_taskbar: bool) -> (u8, u8, u8) {
    if light_taskbar {
        (0x1a, 0x1a, 0x1c)
    } else {
        (0xf2, 0xf2, 0xf4)
    }
}

/// Convex shapes on the 16-unit square: App.cs's path data.
fn shapes(g: Glyph) -> &'static [&'static [(f32, f32)]] {
    const BAR_A: &[(f32, f32)] = &[(4.0, 2.5), (7.0, 2.5), (7.0, 13.5), (4.0, 13.5)];
    const BAR_B: &[(f32, f32)] = &[(9.0, 2.5), (12.0, 2.5), (12.0, 13.5), (9.0, 13.5)];
    match g {
        Glyph::Play => &[&[(5.0, 2.5), (13.5, 8.0), (5.0, 13.5)]],
        Glyph::Pause => &[BAR_A, BAR_B],
        Glyph::Previous => &[
            &[(2.75, 2.5), (4.75, 2.5), (4.75, 13.5), (2.75, 13.5)],
            &[(13.25, 2.5), (13.25, 13.5), (5.25, 8.0)],
        ],
        Glyph::Next => &[
            &[(11.25, 2.5), (13.25, 2.5), (13.25, 13.5), (11.25, 13.5)],
            &[(2.75, 2.5), (2.75, 13.5), (10.75, 8.0)],
        ],
    }
}

/// How much of each pixel the glyph covers (0 to 255), `size` by `size`, rows from the top.
pub(crate) fn coverage(g: Glyph, size: u32) -> Vec<u8> {
    const SAMPLES: u32 = 4; // 4 by 4 per pixel
    let unit = 16.0 / size as f32;
    let mut out = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut hits = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let p = (
                        (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) * unit,
                        (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) * unit,
                    );
                    if shapes(g).iter().any(|s| inside(s, p)) {
                        hits += 1;
                    }
                }
            }
            out.push((hits * 255 / (SAMPLES * SAMPLES)) as u8);
        }
    }
    out
}

fn inside(shape: &[(f32, f32)], (px, py): (f32, f32)) -> bool {
    let mut sign = 0.0f32;
    for (i, &(ax, ay)) in shape.iter().enumerate() {
        let (bx, by) = shape[(i + 1) % shape.len()];
        let cross = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
        if cross != 0.0 {
            if sign != 0.0 && cross.signum() != sign {
                return false;
            }
            sign = cross.signum();
        }
    }
    true
}

/// The glyph as an icon (32-bit, its alpha the coverage). The caller destroys it.
pub(crate) fn icon(g: Glyph, size: u32, ink: (u8, u8, u8)) -> windows::core::Result<HICON> {
    let size = size.clamp(8, 256);
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: size as i32,
        biHeight: -(size as i32), // top-down rows
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO {
        bmiHeader: header,
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    unsafe {
        let colour = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (size * size) as usize);
        let (r, gr, b) = ink;
        for (p, a) in pixels.iter_mut().zip(coverage(g, size)) {
            // BGRA, straight (not premultiplied) alpha, as icons keep it
            *p = (a as u32) << 24 | (r as u32) << 16 | (gr as u32) << 8 | b as u32;
        }
        // an empty mask: the alpha channel alone says what shows
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let made = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: colour,
        });
        let _ = DeleteObject(colour.into());
        let _ = DeleteObject(mask.into());
        made
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;

    fn at(c: &[u8], size: u32, x: u32, y: u32) -> u8 {
        c[(y * size + x) as usize]
    }

    #[test]
    fn shapes_land_where_app_cs_drew_them() {
        let play = coverage(Glyph::Play, 16);
        assert_eq!(at(&play, 16, 7, 7), 255);
        assert_eq!(at(&play, 16, 0, 0), 0);
        assert_eq!(at(&play, 16, 14, 8), 0);
        assert!(play.iter().any(|&a| a > 0 && a < 255), "edges are smoothed");
        let pause = coverage(Glyph::Pause, 16);
        assert_eq!(at(&pause, 16, 5, 8), 255);
        assert_eq!(at(&pause, 16, 8, 8), 0, "the gap between the bars");
        assert_eq!(at(&pause, 16, 10, 8), 255);
        let prev = coverage(Glyph::Previous, 16);
        assert!(
            at(&prev, 16, 3, 8) > 0 && at(&prev, 16, 12, 8) == 255 && at(&prev, 16, 12, 3) < 255
        );
        let next = coverage(Glyph::Next, 16);
        assert!(at(&next, 16, 12, 8) > 0 && at(&next, 16, 3, 8) == 255);
        // at 150 % the same shape, bigger
        let big = coverage(Glyph::Play, 24);
        assert_eq!(big.len(), 24 * 24);
        assert_eq!(at(&big, 24, 11, 12), 255);
    }

    #[test]
    fn ink_follows_the_taskbar() {
        assert_eq!(ink(false), (0xf2, 0xf2, 0xf4));
        assert_eq!(ink(true), (0x1a, 0x1a, 0x1c));
    }

    #[test]
    fn icons_are_made() {
        for g in [Glyph::Previous, Glyph::Play, Glyph::Pause, Glyph::Next] {
            let i = icon(g, 20, ink(false)).unwrap();
            assert!(!i.is_invalid());
            unsafe { DestroyIcon(i).unwrap() };
        }
    }
}
