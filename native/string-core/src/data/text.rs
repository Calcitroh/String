//! Preserve isolated UTF-16 code units from JSON inside Rust strings.
//! Quote each such unit with NUL plus a private-use character, and double each literal NUL.
//! This leaves ordinary text untouched and cannot alias another Windows filename.
use std::{borrow::Cow, io::Write};

fn marker(unit: u16) -> char {
    char::from_u32(0xe000 + u32::from(unit - 0xd800)).unwrap()
}

pub(crate) fn from_units(units: &[u16]) -> String {
    let mut out = String::new();
    for ch in char::decode_utf16(units.iter().copied()) {
        match ch {
            Ok('\0') => out.push_str("\0\0"),
            Ok(ch) => out.push(ch),
            Err(e) => {
                out.push('\0');
                out.push(marker(e.unpaired_surrogate()));
            }
        }
    }
    out
}

pub(crate) fn units(text: &str) -> Vec<u16> {
    let mut chars = text.chars().peekable();
    let mut out = Vec::new();
    while let Some(ch) = chars.next() {
        if ch == '\0' {
            match chars.peek().copied() {
                Some('\0') => {
                    chars.next();
                }
                Some(c @ '\u{e000}'..='\u{e7ff}') => {
                    chars.next();
                    out.push(0xd800 + (c as u16 - 0xe000));
                    continue;
                }
                _ => {}
            }
        }
        out.extend_from_slice(ch.encode_utf16(&mut [0; 2]));
    }
    out
}

fn escape(bytes: &[u8]) -> Option<u16> {
    let digits = bytes.strip_prefix(br"\u")?.get(..4)?;
    if !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u16::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
}

pub(super) fn read_json(bytes: &[u8]) -> Cow<'_, [u8]> {
    let mut out = Vec::new();
    let mut copied = 0;
    let mut i = 0;
    while let Some(offset) = bytes[i..].iter().position(|&b| b == b'\\') {
        i += offset;
        let Some(unit) = escape(&bytes[i..]) else {
            i = (i + 2).min(bytes.len());
            continue;
        };
        if (0xd800..=0xdbff).contains(&unit)
            && escape(&bytes[i + 6..]).is_some_and(|u| (0xdc00..=0xdfff).contains(&u))
        {
            i += 12;
            continue;
        }
        if unit == 0 || (0xd800..=0xdfff).contains(&unit) {
            out.extend_from_slice(&bytes[copied..i]);
            out.extend_from_slice(br"\u0000");
            if unit == 0 {
                out.extend_from_slice(br"\u0000");
            } else {
                out.extend_from_slice(marker(unit).encode_utf8(&mut [0; 4]).as_bytes());
            }
            copied = i + 6;
        }
        i += 6;
    }
    if copied == 0 {
        return Cow::Borrowed(bytes);
    }
    out.extend_from_slice(&bytes[copied..]);
    Cow::Owned(out)
}

pub(super) fn write_json(bytes: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut copied = 0;
    let mut i = 0;
    while let Some(offset) = bytes[i..].iter().position(|&b| b == b'\\') {
        i += offset;
        if !bytes[i..].starts_with(br"\u0000") {
            i = (i + 2).min(bytes.len());
            continue;
        }
        let rest = &bytes[i + 6..];
        out.extend_from_slice(&bytes[copied..i]);
        if rest.starts_with(br"\u0000") {
            out.extend_from_slice(br"\u0000");
            i += 12;
        } else if let Some(c @ '\u{e000}'..='\u{e7ff}') = rest
            .get(..3)
            .and_then(|s| std::str::from_utf8(s).ok())
            .and_then(|s| s.chars().next())
        {
            write!(out, r"\u{:04x}", 0xd800 + (c as u16 - 0xe000)).unwrap();
            i += 9;
        } else {
            out.extend_from_slice(br"\u0000");
            i += 6;
        }
        copied = i;
    }
    if copied == 0 {
        return bytes;
    }
    out.extend_from_slice(&bytes[copied..]);
    out
}
