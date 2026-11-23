use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};
use tempfile::NamedTempFile;

// art.hash and String.length both operate on UTF-16 code units.
pub(crate) fn hash(text: &str) -> u32 {
    text.encode_utf16()
        .fold(2166136261, |h, c| (h ^ u32::from(c)).wrapping_mul(16777619))
}

pub(crate) fn read_text(path: &Path) -> Option<String> {
    const MAX: u64 = 32 << 20;
    let file = fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX {
        return None;
    }
    // File.ReadAllText detects Unicode BOMs on sidecars and old cache files.
    for (bom, little) in [
        (&[0xff, 0xfe, 0, 0][..], true),
        (&[0, 0, 0xfe, 0xff][..], false),
    ] {
        if let Some(data) = bytes.strip_prefix(bom) {
            return Some(
                data.chunks(4)
                    .map(|part| {
                        let Ok(unit) = part.try_into() else {
                            return char::REPLACEMENT_CHARACTER;
                        };
                        char::from_u32(if little {
                            u32::from_le_bytes(unit)
                        } else {
                            u32::from_be_bytes(unit)
                        })
                        .unwrap_or(char::REPLACEMENT_CHARACTER)
                    })
                    .collect(),
            );
        }
    }
    for (bom, little) in [([0xff, 0xfe], true), ([0xfe, 0xff], false)] {
        if let Some(data) = bytes.strip_prefix(&bom) {
            let units: Vec<_> = data
                .chunks(2)
                .map(|part| match *part {
                    [a, b] if little => u16::from_le_bytes([a, b]),
                    [a, b] => u16::from_be_bytes([a, b]),
                    _ => 0xfffd,
                })
                .collect();
            return Some(String::from_utf16_lossy(&units));
        }
    }
    Some(
        String::from_utf8_lossy(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
            .into_owned(),
    )
}

pub(crate) fn read<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_str(&read_text(path)?).ok()
}

pub(crate) fn write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let dir = path.parent().expect("cache file in subdirectory");
    fs::create_dir_all(dir)?;
    let mut temp = NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut temp, value)?;
    temp.flush()?;
    temp.persist(path)?;
    Ok(())
}
