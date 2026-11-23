//! Windows ships ICU, the same Unicode operations used by the desktop page.
use std::{
    borrow::Cow,
    cmp::Ordering,
    ffi::{c_char, c_void},
    ptr::{self, NonNull},
};

/// Same BOM detection as the host's text-file reader.
pub(crate) fn host_text(bytes: &[u8]) -> Cow<'_, str> {
    for (bom, decode) in [
        (b"\xff\xfe\0\0", u32::from_le_bytes as fn([u8; 4]) -> u32),
        (b"\0\0\xfe\xff", u32::from_be_bytes),
    ] {
        if let Some(bytes) = bytes.strip_prefix(bom) {
            return bytes
                .chunks(4)
                .map(|b| {
                    b.try_into()
                        .ok()
                        .map(decode)
                        .and_then(char::from_u32)
                        .unwrap_or(char::REPLACEMENT_CHARACTER)
                })
                .collect::<String>()
                .into();
        }
    }
    for (bom, decode) in [
        (b"\xff\xfe", u16::from_le_bytes as fn([u8; 2]) -> u16),
        (b"\xfe\xff", u16::from_be_bytes),
    ] {
        if let Some(bytes) = bytes.strip_prefix(bom) {
            let units = bytes
                .chunks(2)
                .map(|b| b.try_into().ok().map(decode).unwrap_or(0xfffd));
            return char::decode_utf16(units)
                .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect::<String>()
                .into();
        }
    }
    String::from_utf8_lossy(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes))
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CompareStringOrdinal(
        a: *const u16,
        alen: i32,
        b: *const u16,
        blen: i32,
        ignore_case: i32,
    ) -> i32;
}

pub(crate) fn same_windows_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    fn units(path: &std::path::Path) -> Vec<u16> {
        let mut result: Vec<_> = path.as_os_str().encode_wide().collect();
        while result.last() == Some(&92) {
            result.pop();
        }
        result
    }
    let (a, b) = (units(a), units(b));
    // Compare only these two slices, using Windows' ordinal case table.
    unsafe { CompareStringOrdinal(a.as_ptr(), len(a.len()), b.as_ptr(), len(b.len()), 1) == 2 }
}

#[link(name = "icu")]
unsafe extern "C" {
    fn uregex_open(
        pattern: *const u16,
        len: i32,
        flags: u32,
        parse: *mut c_void,
        status: *mut i32,
    ) -> *mut c_void;
    fn uregex_setText(regex: *mut c_void, text: *const u16, len: i32, status: *mut i32);
    fn uregex_replaceAll(
        regex: *mut c_void,
        replacement: *const u16,
        len: i32,
        out: *mut u16,
        capacity: i32,
        status: *mut i32,
    ) -> i32;
    fn uregex_close(regex: *mut c_void);
    fn unorm2_getNFKDInstance(status: *mut i32) -> *const c_void;
    fn unorm2_normalize(
        instance: *const c_void,
        input: *const u16,
        len: i32,
        out: *mut u16,
        capacity: i32,
        status: *mut i32,
    ) -> i32;
    fn ucol_open(locale: *const c_char, status: *mut i32) -> *mut c_void;
    fn ucol_strcollUTF8(
        coll: *const c_void,
        a: *const c_char,
        alen: i32,
        b: *const c_char,
        blen: i32,
        status: *mut i32,
    ) -> i32;
    fn ucol_close(coll: *mut c_void);
}

fn len(n: usize) -> i32 {
    n.try_into().expect("text exceeds ICU's length limit")
}

// All patterns are constants. Each compiled expression belongs to one thread.
struct Pattern(NonNull<c_void>);
impl Pattern {
    fn new(pattern: &str) -> Self {
        let pattern = pattern.replace(r"\s", r"[\p{Zs}\u0009-\u000D\u2028\u2029\uFEFF]");
        let utf16: Vec<_> = pattern.encode_utf16().collect();
        let mut status = 0;
        // ICU copies the pattern. It owns the resulting expression until close.
        let re = unsafe {
            uregex_open(
                utf16.as_ptr(),
                len(utf16.len()),
                0,
                ptr::null_mut(),
                &mut status,
            )
        };
        assert!(
            status <= 0,
            "invalid built-in expression {pattern}: {status}"
        );
        Self(NonNull::new(re).unwrap())
    }
    fn replace(&self, input: &str, replacement: &[u16]) -> String {
        if input.is_empty() {
            return String::new();
        }
        let input: Vec<_> = input.encode_utf16().collect();
        // Every pattern consumes at least as many code units as it inserts.
        let mut output = vec![0; input.len() + 1];
        let mut status = 0;
        static EMPTY: [u16; 1] = [0];
        // Both slices survive the call. Detach the subject before its storage drops.
        let count = unsafe {
            uregex_setText(
                self.0.as_ptr(),
                input.as_ptr(),
                len(input.len()),
                &mut status,
            );
            let count = uregex_replaceAll(
                self.0.as_ptr(),
                replacement.as_ptr(),
                len(replacement.len()),
                output.as_mut_ptr(),
                len(output.len()),
                &mut status,
            );
            let mut reset = 0;
            uregex_setText(self.0.as_ptr(), EMPTY.as_ptr(), 0, &mut reset);
            count
        };
        assert!(status <= 0, "ICU replacement failed: {status}");
        String::from_utf16(&output[..count as usize]).unwrap()
    }
}
impl Drop for Pattern {
    fn drop(&mut self) {
        unsafe { uregex_close(self.0.as_ptr()) }
    }
}
macro_rules! sub {
    ($text:expr, $expr:expr, $repl:expr) => {{
        thread_local! { static RE: Pattern = Pattern::new($expr); }
        RE.with(|r| r.replace($text, $repl))
    }};
}
const SPACE: &[u16] = &[32];
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c == '\u{feff}' || (c.is_whitespace() && c != '\u{85}'))
}
pub(crate) fn file_title(path: &str) -> String {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let stem = sub!(name, r"\.[^.]+$", &[]);
    let stem = sub!(
        &stem,
        r"\s*[\[(](?:[A-Za-z0-9_-]{11}|[0-9]{6,})[\])]\s*$",
        &[]
    );
    let stem = sub!(&stem, r"\s*\([0-9]\)$", &[]);
    let stem = sub!(&stem, r"^\s*[0-9]{1,3}\s*[-.)_]\s+(?=\S)", &[]);
    let stem = sub!(&stem.replace('_', " "), r"\s{2,}", SPACE);
    let stem = js_trim(&stem);
    if stem.is_empty() {
        name.to_owned()
    } else {
        stem.to_owned()
    }
}
fn nfkd(s: &str) -> String {
    if s.is_ascii() {
        return s.to_owned();
    }
    let input: Vec<_> = s.encode_utf16().collect();
    let mut status = 0;
    // ICU owns this immutable singleton. On overflow it returns the required length.
    let norm = unsafe { unorm2_getNFKDInstance(&mut status) };
    assert!(status <= 0);
    let mut output = vec![0; input.len() + 16];
    loop {
        let n = unsafe {
            unorm2_normalize(
                norm,
                input.as_ptr(),
                len(input.len()),
                output.as_mut_ptr(),
                len(output.len()),
                &mut status,
            )
        };
        if status == 15 {
            // U_BUFFER_OVERFLOW_ERROR
            output.resize(n as usize + 1, 0);
            status = 0;
        } else {
            assert!(status <= 0);
            return String::from_utf16(&output[..n as usize]).unwrap();
        }
    }
}
pub(crate) fn norm(s: &str) -> String {
    let mut s = nfkd(s);
    s.retain(|c| !('\u{0300}'..='\u{036f}').contains(&c));
    sub!(&s.to_lowercase(), r"[^\p{L}\p{N}]+", SPACE)
        .trim()
        .into()
}
pub(crate) fn titles(s: &str) -> (String, String) {
    // JS word boundaries for these ASCII keywords use ASCII letters, digits and underscore.
    let plain = sub!(
        s,
        r"\s*[(\[【〔「][^)\]】〕」]*?(?<![a-zA-Z0-9_])(?i:official|lyrics?|audio|video|visuali[sz]er|mv|m/v|hd|hq|4k|1080p|720p|amv|music\s+video|full\s+version|clip\s+officiel)(?![a-zA-Z0-9_])[^)\]】〕」]*[)\]】〕」]",
        SPACE
    );
    let core = sub!(s, r"[(\[{【〔「][^)\]}】〕」]*[)\]}】〕」]", SPACE);
    let core = sub!(&core, r"\s+[-–—|｜]\s+.*$", &[]);
    let core = sub!(&core, r"\s(?i:feat|ft)\.?\s.*$", &[]);
    (norm(&plain), norm(&core))
}

struct Collator(NonNull<c_void>);
impl Drop for Collator {
    fn drop(&mut self) {
        unsafe { ucol_close(self.0.as_ptr()) }
    }
}
pub(crate) fn collate(a: &str, b: &str) -> Ordering {
    thread_local! { static COLL: Collator = {
        let mut status = 0;
        // A null locale selects the user's default, as with localeCompare().
        let raw = unsafe { ucol_open(ptr::null(), &mut status) };
        assert!(status <= 0);
        Collator(NonNull::new(raw).unwrap())
    }; }
    COLL.with(|coll| {
        let mut status = 0;
        let ord = unsafe {
            ucol_strcollUTF8(
                coll.0.as_ptr(),
                a.as_ptr().cast(),
                len(a.len()),
                b.as_ptr().cast(),
                len(b.len()),
                &mut status,
            )
        };
        assert!(status <= 0);
        ord.cmp(&0)
    })
}
