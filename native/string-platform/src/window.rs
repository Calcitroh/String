//! String's main window: the dark title bar, no white flash at start, bringing it forward, and its place on screen in
//! `window.json` (App.cs's format, so String 1.x and this read each other's).
//!
//! # No white flash at start
//!
//! The room this runs in is dark: the window must never show a white frame, title bar or edge. In order:
//!
//! 1. Make the window hidden (winit: `with_visible(false)`; Slint: don't `show()` yet).
//! 2. While it's hidden: [`style_title_bar`] (DWM draws the caption dark from the very first frame; set after showing,
//!    the caption flashes light once), [`set_background`] with [`BACKGROUND`] (whatever Windows paints before the app
//!    does, at first and while resizing, is near-black instead of white), and [`restore_placement`].
//! 3. Clear every frame to [`BACKGROUND`] (egui: `Visuals::panel_fill` and the renderer's clear colour; Slint: the
//!    `Window`'s `background`).
//! 4. Show it only once the first frame is drawn. A toolkit that can't draw while hidden: show it cloaked
//!    ([`cloak`] on, show, draw the first frame, [`cloak`] off). DWM then shows nothing until that frame is there.
//! 5. Show it maximised in one step ([`show`] with `true`), not normal and then maximised: that is two frames, the
//!    first at the wrong size.
//! 6. A web view inside (none planned) would need its own background set before it loads (App.cs set WebView2's
//!    `DefaultBackgroundColor`).

use crate::Hwnd;
use std::{fs, io, path::Path};
use windows::Win32::Foundation::{COLORREF, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_CLOAK, DWMWA_TEXT_COLOR,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWINDOWATTRIBUTE, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE, GetDpiForSystem,
    SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GCLP_HBRBACKGROUND, GWL_EXSTYLE, GetCursorPos, GetSystemMetrics, GetWindowLongPtrW,
    GetWindowPlacement, HWND_NOTOPMOST, HWND_TOPMOST, IsIconic, IsWindowVisible, IsZoomed,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_RESTORE,
    SW_SHOW, SW_SHOWMAXIMIZED, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER,
    SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetClassLongPtrW, SetForegroundWindow, SetWindowPos,
    ShowWindow, WINDOWPLACEMENT, WS_EX_TOOLWINDOW,
};

/// A colour, red, green and blue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    fn colorref(self) -> COLORREF {
        // COLORREF is 0x00BBGGRR
        COLORREF(self.0 as u32 | (self.1 as u32) << 8 | (self.2 as u32) << 16)
    }
}

/// String's near-black, behind everything (App.cs's window and WebView2 background).
pub const BACKGROUND: Rgb = Rgb(0x05, 0x05, 0x06);
/// The title bar's text (App.cs's).
pub const CAPTION_TEXT: Rgb = Rgb(0x8e, 0x8e, 0x96);

/// App.cs's title bar: dark mode (DWMWA_USE_IMMERSIVE_DARK_MODE), and on Windows 11 the caption and border in
/// [`BACKGROUND`] with [`CAPTION_TEXT`]. Call before the window is first shown.
pub fn style_title_bar(hwnd: Hwnd) -> io::Result<()> {
    set_dark_title_bar(hwnd, true)?;
    // Windows 10 doesn't know these three and says so: the dark mode above is all it gets
    let _ = dwm_set(hwnd, DWMWA_CAPTION_COLOR, BACKGROUND.colorref().0);
    let _ = dwm_set(hwnd, DWMWA_BORDER_COLOR, BACKGROUND.colorref().0);
    let _ = dwm_set(hwnd, DWMWA_TEXT_COLOR, CAPTION_TEXT.colorref().0);
    Ok(())
}

/// The title bar's dark mode alone (DWMWA_USE_IMMERSIVE_DARK_MODE).
pub fn set_dark_title_bar(hwnd: Hwnd, on: bool) -> io::Result<()> {
    dwm_set(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, on as u32)
}

/// Hides the window from the screen while it stays "shown" (DWMWA_CLOAK): see "No white flash at start".
pub fn cloak(hwnd: Hwnd, on: bool) -> io::Result<()> {
    dwm_set(hwnd, DWMWA_CLOAK, on as u32)
}

/// What Windows paints the window with before the app draws (its class's background brush), at first and in the strip
/// a resize uncovers. Every window of the same class gets it. Call once: the brush lives as long as the app.
pub fn set_background(hwnd: Hwnd, colour: Rgb) {
    unsafe {
        let brush = CreateSolidBrush(colour.colorref());
        SetClassLongPtrW(hwnd.get(), GCLP_HBRBACKGROUND, brush.0 as isize);
    }
}

/// Shows the window for the first time, maximised (as `window.json` asked) or normal, in one step.
pub fn show(hwnd: Hwnd, maximized: bool) {
    unsafe {
        let _ = ShowWindow(
            hwnd.get(),
            if maximized {
                SW_SHOWMAXIMIZED
            } else {
                SW_SHOWNORMAL
            },
        );
    }
}

/// App.cs's BringToFront, for a second launch: out of the taskbar if minimised (maximised again if it was), shown,
/// active, and above every other window once (topmost on and straight off again).
pub fn bring_to_front(hwnd: Hwnd) {
    let h = hwnd.get();
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        } else if !IsWindowVisible(h).as_bool() {
            let _ = ShowWindow(h, SW_SHOW);
        }
        let _ = SetForegroundWindow(h);
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW;
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, flags);
        let _ = SetWindowPos(h, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags);
    }
}

fn dwm_set(hwnd: Hwnd, attribute: DWMWINDOWATTRIBUTE, value: u32) -> io::Result<()> {
    unsafe { DwmSetWindowAttribute(hwnd.get(), attribute, &value as *const u32 as *const _, 4) }?;
    Ok(())
}

/* ---------- window.json ---------- */

pub const MIN_SIZE: (f64, f64) = (960., 600.);

// WPF runs system-DPI-aware. Ask Windows for the same virtualized desktop coordinates and use
// that coordinate space when positioning the per-monitor-aware native window, too.
struct WpfCoordinates(DPI_AWARENESS_CONTEXT);
impl WpfCoordinates {
    fn enter() -> Self {
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE) })
    }
}
impl Drop for WpfCoordinates {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}

/// The window's place as `window.json` keeps it: WPF's `RestoreBounds` (the outer edges of the window when neither
/// maximised nor minimised) in device-independent pixels (1/96 inch at the system's scale), and whether it was
/// maximised. On disk: `{"Left":100,"Top":50,"Width":1400,"Height":880,"Max":false}`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    pub max: bool,
}

impl Placement {
    /// A missing number counts as 0 (and then fails [`Placement::fits`]), as in App.cs; anything else odd is `None`.
    pub fn from_json(text: &str) -> Option<Placement> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let o = v.as_object()?;
        let n = |key: &str| match o.get(key) {
            None => Some(0.0),
            Some(x) => x.as_f64(),
        };
        let max = match o.get("Max") {
            None => false,
            Some(b) => b.as_bool()?,
        };
        Some(Placement {
            left: n("Left")?,
            top: n("Top")?,
            width: n("Width")?,
            height: n("Height")?,
            max,
        })
    }

    pub fn to_json(&self) -> String {
        format!(
            r#"{{"Left":{},"Top":{},"Width":{},"Height":{},"Max":{}}}"#,
            number(self.left),
            number(self.top),
            number(self.width),
            number(self.height),
            self.max
        )
    }

    /// `window.json`; `None` if it's missing or broken.
    pub fn load(path: &Path) -> Option<Placement> {
        Placement::from_json(&fs::read_to_string(path).ok()?)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        fs::write(path, self.to_json())
    }

    /// Where the window is now; its normal place even while it's maximised or minimised. The saved flag reflects its current state.
    pub fn of_window(hwnd: Hwnd) -> Option<Placement> {
        Self::read_bounds(|| normal_bounds(hwnd))
    }

    fn read_bounds(read: impl FnOnce() -> Option<(RECT, bool)>) -> Option<Self> {
        let _coordinates = WpfCoordinates::enter();
        let (r, max) = read()?;
        let s = scale();
        Some(Placement {
            left: r.left as f64 / s,
            top: r.top as f64 / s,
            width: (r.right - r.left) as f64 / s,
            height: (r.bottom - r.top) as f64 / s,
            max,
        })
    }

    /// App.cs's test before using a saved place: at least the minimum size, and the title bar (80 by 16 pixels in from
    /// its top left corner) inside the Windows virtual-screen rectangle.
    pub fn fits(&self, min_width: f64, min_height: f64) -> bool {
        if ![self.left, self.top, self.width, self.height]
            .iter()
            .all(|x| x.is_finite())
        {
            return false;
        }
        if self.width < min_width || self.height < min_height {
            return false;
        }
        let _coordinates = WpfCoordinates::enter();
        let s = scale();
        let x = (self.left + 80.) * s;
        let y = (self.top + 16.) * s;
        let metric = |index| unsafe { GetSystemMetrics(index) } as f64;
        let left = metric(SM_XVIRTUALSCREEN);
        let top = metric(SM_YVIRTUALSCREEN);
        x >= left
            && y >= top
            && x <= left + metric(SM_CXVIRTUALSCREEN)
            && y <= top + metric(SM_CYVIRTUALSCREEN)
    }

    /// Moves and sizes the window to this place (normal, not maximised; [`show`] maximises). Meant for the hidden
    /// window at start: it neither shows nor activates it.
    pub fn apply(&self, hwnd: Hwnd) -> io::Result<()> {
        self.position(|[x, y, w, h]| {
            let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
            unsafe { SetWindowPos(hwnd.get(), None, x, y, w, h, flags) }?;
            Ok(())
        })
    }

    fn position<T>(&self, place: impl FnOnce([i32; 4]) -> T) -> T {
        let _coordinates = WpfCoordinates::enter();
        let s = scale();
        place([self.left, self.top, self.width, self.height].map(|x| (x * s).round() as i32))
    }
}

fn normal_bounds(hwnd: Hwnd) -> Option<(RECT, bool)> {
    let h = hwnd.get();
    let mut wp = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    unsafe { GetWindowPlacement(h, &mut wp) }.ok()?;
    let mut r = wp.rcNormalPosition;
    // the normal place is in workspace coordinates (beside the taskbar), except for tool windows: to the screen's
    let ex = unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) } as u32;
    if ex & WS_EX_TOOLWINDOW.0 == 0 {
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut mi) }
            .as_bool()
        {
            let (dx, dy) = (
                mi.rcWork.left - mi.rcMonitor.left,
                mi.rcWork.top - mi.rcMonitor.top,
            );
            r.left += dx;
            r.right += dx;
            r.top += dy;
            r.bottom += dy;
        }
    }
    Some((r, unsafe { IsZoomed(h).as_bool() }))
}

/// App.cs's RestorePlacement, for the hidden window at start: the place in `window.json` if it [`fits`](Placement::fits),
/// else `default_size` (device-independent pixels) in the middle of the screen the pointer is on. Returns whether to
/// show it maximised.
pub fn restore_placement(
    hwnd: Hwnd,
    path: &Path,
    default_size: (f64, f64),
    min_size: (f64, f64),
) -> bool {
    match Placement::load(path) {
        Some(p) if p.fits(min_size.0, min_size.1) && p.apply(hwnd).is_ok() => p.max,
        _ => {
            let _ = center(hwnd, default_size.0, default_size.1);
            false
        }
    }
}

/// App.cs's SavePlacement, when the window closes.
pub fn save_placement(hwnd: Hwnd, path: &Path) -> io::Result<()> {
    match Placement::of_window(hwnd) {
        Some(mut p) if p.width > 0.0 && p.height > 0.0 => {
            p.width = p.width.max(MIN_SIZE.0);
            p.height = p.height.max(MIN_SIZE.1);
            p.save(path)
        }
        _ => Ok(()),
    }
}

/// The window at `width` by `height` (device-independent pixels) in the middle of the work area of the screen the
/// pointer is on (WPF's CenterScreen).
pub fn center(hwnd: Hwnd, width: f64, height: f64) -> io::Result<()> {
    let _coordinates = WpfCoordinates::enter();
    let mut pt = POINT::default();
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = GetCursorPos(&mut pt);
        if !GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool() {
            return Err(io::Error::other("no screen"));
        }
    }
    let s = scale();
    let work = mi.rcWork;
    let (w, h) = ((width * s).round() as i32, (height * s).round() as i32);
    let (x, y) = (
        work.left + (work.right - work.left - w) / 2,
        work.top + (work.bottom - work.top - h) / 2,
    );
    let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
    unsafe { SetWindowPos(hwnd.get(), None, x, y, w, h, flags) }?;
    Ok(())
}

/// Device pixels per device-independent pixel, at the system's scale (what String 1.x's WPF window used).
fn scale() -> f64 {
    let dpi = unsafe { GetDpiForSystem() };
    if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 }
}

/// A number as System.Text.Json writes it: whole numbers without a fraction.
fn number(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

#[cfg(test)]
mod tests {
    // On a hidden window of the tests' own: nothing appears on screen.
    use super::*;
    use crate::test_window::TestWindow;
    use windows::Win32::Graphics::Dwm::DwmGetWindowAttribute;

    #[test]
    fn json_is_string_1x_format() {
        let p = Placement {
            left: 100.0,
            top: 50.0,
            width: 1400.0,
            height: 880.0,
            max: false,
        };
        assert_eq!(
            p.to_json(),
            r#"{"Left":100,"Top":50,"Width":1400,"Height":880,"Max":false}"#
        );
        assert_eq!(Placement::from_json(&p.to_json()), Some(p));
        let wpf = r#"{"Left":-7.5,"Top":0,"Width":1400.25,"Height":880,"Max":true}"#;
        let q = Placement::from_json(wpf).unwrap();
        assert_eq!(
            q,
            Placement {
                left: -7.5,
                top: 0.0,
                width: 1400.25,
                height: 880.0,
                max: true
            }
        );
        assert_eq!(q.to_json(), wpf);
    }

    #[test]
    fn odd_files_restore_nothing() {
        assert_eq!(Placement::from_json("null"), None);
        assert_eq!(Placement::from_json("{"), None);
        assert_eq!(Placement::from_json(r#"{"Left":"x"}"#), None);
        // missing numbers are 0, which then doesn't fit
        let p = Placement::from_json(r#"{"Max":true}"#).unwrap();
        assert!(p.max && !p.fits(960.0, 600.0));
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Placement::load(&dir.path().join("window.json")), None);
    }

    #[test]
    fn fits_needs_the_size_and_a_reachable_title_bar() {
        let on_screen = Placement {
            left: 0.0,
            top: 0.0,
            width: 1400.0,
            height: 880.0,
            max: false,
        };
        assert!(on_screen.fits(960.0, 600.0));
        assert!(
            !Placement {
                width: 900.0,
                ..on_screen
            }
            .fits(960.0, 600.0)
        );
        assert!(
            !Placement {
                height: 500.0,
                ..on_screen
            }
            .fits(960.0, 600.0)
        );
        assert!(
            !Placement {
                left: -50000.0,
                top: -50000.0,
                ..on_screen
            }
            .fits(960.0, 600.0)
        );
        assert!(
            !Placement {
                left: f64::NAN,
                ..on_screen
            }
            .fits(960.0, 600.0)
        );
    }

    #[test]
    fn a_place_applied_reads_back() {
        let w = TestWindow::new();
        let p = Placement {
            left: 120.0,
            top: 90.0,
            width: 1000.0,
            height: 700.0,
            max: false,
        };
        p.apply(w.hwnd).unwrap();
        let q = Placement::of_window(w.hwnd).unwrap();
        let close = |a: f64, b: f64| (a - b).abs() <= 1.0;
        assert!(
            close(q.left, p.left)
                && close(q.top, p.top)
                && close(q.width, p.width)
                && close(q.height, p.height)
        );
        assert!(!q.max);
        assert!(
            !unsafe { IsWindowVisible(w.hwnd.get()) }.as_bool(),
            "applying must not show the window"
        );
    }

    #[test]
    fn saved_then_restored() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("window.json");
        let w = TestWindow::new();
        Placement {
            left: 200.0,
            top: 150.0,
            width: 1100.0,
            height: 720.0,
            max: true,
        }
        .save(&file)
        .unwrap();
        assert!(restore_placement(
            w.hwnd,
            &file,
            (1400.0, 880.0),
            (960.0, 600.0)
        ));
        save_placement(w.hwnd, &file).unwrap();
        let p = Placement::load(&file).unwrap();
        assert!(
            (p.left - 200.0).abs() <= 1.0 && (p.width - 1100.0).abs() <= 1.0,
            "{p:?}"
        );
        // too small to use: centred at the default size instead
        Placement {
            left: 200.0,
            top: 150.0,
            width: 300.0,
            height: 200.0,
            max: true,
        }
        .save(&file)
        .unwrap();
        assert!(!restore_placement(
            w.hwnd,
            &file,
            (1400.0, 880.0),
            (960.0, 600.0)
        ));
        let q = Placement::of_window(w.hwnd).unwrap();
        assert!(
            (q.width - 1400.0).abs() <= 1.0 && (q.height - 880.0).abs() <= 1.0,
            "{q:?}"
        );
        assert!(!unsafe { IsWindowVisible(w.hwnd.get()) }.as_bool());
    }

    #[test]
    fn title_bar_goes_dark() {
        let w = TestWindow::new();
        style_title_bar(w.hwnd).unwrap();
        let mut on = 0u32;
        unsafe {
            DwmGetWindowAttribute(
                w.hwnd.get(),
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &mut on as *mut u32 as *mut _,
                4,
            )
        }
        .unwrap();
        assert_eq!(on, 1);
        cloak(w.hwnd, true).unwrap();
        cloak(w.hwnd, false).unwrap();
        set_background(w.hwnd, BACKGROUND);
    }

    #[test]
    fn mixed_scale_coordinates_are_read_in_the_legacy_context() {
        use windows::Win32::UI::HiDpi::{
            AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            GetThreadDpiAwarenessContext,
        };
        let _restore = WpfCoordinates(unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
        });
        // The injected monitor reports twice the dimensions to per-monitor callers.
        // Exercise both placement operations on the same window without changing Windows settings.
        let virtualized = || unsafe {
            AreDpiAwarenessContextsEqual(
                GetThreadDpiAwarenessContext(),
                DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
            )
            .as_bool()
        };
        let p = Placement::read_bounds(|| {
            let r = if virtualized() {
                RECT {
                    left: 2020,
                    top: 50,
                    right: 3020,
                    bottom: 650,
                }
            } else {
                RECT {
                    left: 2120,
                    top: 100,
                    right: 4120,
                    bottom: 1300,
                }
            };
            Some((r, false))
        })
        .unwrap();
        let system_scale = {
            let _wpf = WpfCoordinates::enter();
            scale()
        };
        assert_eq!(p.width, 1000. / system_scale);
        assert!(!virtualized());
        let physical = p.position(|[x, y, w, h]| {
            if virtualized() {
                [1920 + (x - 1920) * 2, y * 2, w * 2, h * 2]
            } else {
                [x, y, w, h]
            }
        });
        assert_eq!(physical, [2120, 100, 2000, 1200]);
        assert!(!virtualized());
    }

    #[test]
    fn system_aware_and_per_monitor_windows_share_bounds_on_each_display() {
        use windows::Win32::Foundation::{LPARAM, RECT};
        use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
        use windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2;
        use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
        use windows::core::BOOL;

        unsafe extern "system" fn collect(_: HMONITOR, _: HDC, r: *mut RECT, data: LPARAM) -> BOOL {
            unsafe { (*(data.0 as *mut Vec<RECT>)).push(*r) };
            BOOL(1)
        }
        let _restore = WpfCoordinates(unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
        });
        let native = TestWindow::new();
        let (legacy, screens, system_scale) = {
            let _coordinates = WpfCoordinates::enter();
            let mut screens: Vec<RECT> = vec![];
            let list = LPARAM((&mut screens as *mut Vec<RECT>) as isize);
            assert!(unsafe { EnumDisplayMonitors(None, None, Some(collect), list) }.as_bool());
            (TestWindow::new(), screens, scale())
        };
        assert!(!screens.is_empty());
        for screen in screens {
            // WPF creates its HWND at system-scale bounds. Compare the two actual outer
            // rectangles from the per-monitor-aware caller, including the secondary origin.
            let (x, y) = (screen.left + 32, screen.top + 40);
            let (w, h) = (
                (980. * system_scale).round() as i32,
                (610. * system_scale).round() as i32,
            );
            {
                let _coordinates = WpfCoordinates::enter();
                unsafe {
                    SetWindowPos(
                        legacy.hwnd.get(),
                        None,
                        x,
                        y,
                        w,
                        h,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    )
                }
                .unwrap();
            }
            let saved = Placement {
                left: x as f64 / system_scale,
                top: y as f64 / system_scale,
                width: w as f64 / system_scale,
                height: h as f64 / system_scale,
                max: false,
            };
            saved.apply(native.hwnd).unwrap();
            let rect = |win: &TestWindow| {
                let mut bounds = RECT::default();
                unsafe { GetWindowRect(win.hwnd.get(), &mut bounds) }.unwrap();
                bounds
            };
            assert_eq!(rect(&native), rect(&legacy));
            let back = Placement::of_window(native.hwnd).unwrap();
            for (a, b) in [
                (back.left, saved.left),
                (back.top, saved.top),
                (back.width, saved.width),
                (back.height, saved.height),
            ] {
                assert!((a - b).abs() <= 1., "{back:?} versus {saved:?}");
            }
        }
    }

    #[test]
    fn small_native_window_saves_a_size_the_web_version_accepts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("window.json");
        let w = TestWindow::new();
        let small = Placement {
            left: 40.,
            top: 40.,
            width: 830.,
            height: 540.,
            max: false,
        };
        small.apply(w.hwnd).unwrap();
        save_placement(w.hwnd, &path).unwrap();
        let saved = Placement::load(&path).unwrap();
        assert!(saved.fits(960., 600.), "{saved:?}");
    }

    #[test]
    fn colours_are_colorrefs() {
        assert_eq!(BACKGROUND.colorref().0, 0x0006_0505);
        assert_eq!(CAPTION_TEXT.colorref().0, 0x0096_8e8e);
    }
}
