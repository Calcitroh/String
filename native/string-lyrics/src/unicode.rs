//! Use the Unicode services included with Windows, including supplementary-plane characters.
use std::ptr;

#[link(name = "normaliz")]
unsafe extern "system" {
    fn NormalizeString(form: i32, src: *const u16, len: i32, dst: *mut u16, capacity: i32) -> i32;
}
#[link(name = "icu")]
unsafe extern "C" {
    fn u_charType(c: i32) -> i8;
}

pub(crate) fn decompose(s: &str) -> String {
    if s.is_ascii() {
        return s.to_owned();
    }
    let units: Vec<_> = s.encode_utf16().collect();
    let Ok(len) = i32::try_from(units.len()) else {
        return s.to_owned();
    };
    // NormalizationKD (6). Windows returns an estimate, or the negative required
    // capacity if the destination was too small. Both buffers have explicit lengths.
    let mut size = unsafe { NormalizeString(6, units.as_ptr(), len, ptr::null_mut(), 0) };
    while size > 0 {
        let mut out = vec![0; size as usize];
        let used = unsafe { NormalizeString(6, units.as_ptr(), len, out.as_mut_ptr(), size) };
        if used > 0 {
            return String::from_utf16_lossy(&out[..used as usize]);
        }
        if used >= -size {
            break;
        }
        size = -used;
    }
    s.to_owned()
}

pub(crate) fn letter_or_number(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric();
    }
    // ICU UCharCategory: Lu..Lo and Nd..No, the two groups used by lyrics.js.
    matches!(unsafe { u_charType(c as i32) }, 1..=5 | 9..=11)
}
