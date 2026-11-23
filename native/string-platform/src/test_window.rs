//! A window for the tests that is never shown: no taskbar button, nothing on screen, nothing for the media overlay.

use crate::Hwnd;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WINDOW_EX_STYLE, WNDCLASSW,
    WS_OVERLAPPEDWINDOW,
};
use windows::core::w;

pub(crate) struct TestWindow {
    pub hwnd: Hwnd,
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl TestWindow {
    pub fn new() -> TestWindow {
        unsafe {
            let instance = GetModuleHandleW(None).unwrap();
            let class = w!("StringPlatformTestWindow");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc); // a second test registering it again is refused, harmlessly
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("String platform test"),
                WS_OVERLAPPEDWINDOW,
                -32000,
                -32000,
                800,
                600,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .unwrap();
            TestWindow {
                hwnd: Hwnd(hwnd.0 as isize),
            }
        }
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd.get());
        }
    }
}
