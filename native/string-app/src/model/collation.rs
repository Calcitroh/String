//! Sort keys from Windows' ICU, using the library's numeric, base-sensitivity comparison.
use std::{collections::HashMap, ptr::NonNull};
use windows::{Win32::Globalization::*, core::PCSTR};

struct Collator(NonNull<UCollator>);

impl Collator {
    /// None when Windows' collator can't be opened or set up; the lists then sort plainly instead of String closing.
    fn new() -> Option<Self> {
        let mut status = U_ZERO_ERROR;
        // A null locale selects ICU's default for the current user.
        let raw = unsafe { ucol_open(PCSTR::null(), &mut status) };
        let collator = Self(NonNull::new(raw)?);
        if status.0 > 0 {
            return None;
        }
        unsafe {
            ucol_setStrength(raw, UCOL_PRIMARY);
            ucol_setAttribute(raw, UCOL_CASE_LEVEL, UCOL_OFF, &mut status);
            ucol_setAttribute(raw, UCOL_NUMERIC_COLLATION, UCOL_ON, &mut status);
            ucol_setAttribute(raw, UCOL_NORMALIZATION_MODE, UCOL_ON, &mut status);
        }
        (status.0 <= 0).then_some(collator)
    }

    fn key(&self, text: &str) -> Box<[u8]> {
        let source: Vec<u16> = text.encode_utf16().collect();
        let mut key = vec![0; source.len() + 8];
        loop {
            // ICU returns the byte count including its terminator, even when the buffer is short.
            let needed = unsafe {
                ucol_getSortKey(
                    self.0.as_ptr(),
                    source.as_ptr(),
                    source.len() as i32,
                    key.as_mut_ptr(),
                    key.len() as i32,
                )
            } as usize;
            if needed <= key.len() {
                key.truncate(needed);
                return key.into_boxed_slice();
            }
            key.resize(needed, 0);
        }
    }
}

impl Drop for Collator {
    fn drop(&mut self) {
        unsafe { ucol_close(self.0.as_ptr()) };
    }
}

pub(super) fn keys<'a>(values: impl Iterator<Item = &'a str>) -> HashMap<String, Box<[u8]>> {
    let collator = Collator::new();
    let mut keys = HashMap::new();
    for value in values {
        if !keys.contains_key(value) {
            let key = match &collator {
                Some(c) => c.key(value),
                None => value.to_lowercase().into_bytes().into_boxed_slice(),
            };
            keys.insert(value.to_owned(), key);
        }
    }
    keys
}
