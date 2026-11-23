//! Puts String's icon (`assets/String.ico`) into String.exe, so Explorer, the Start menu and the taskbar show it.
//! The icon goes in as a compiled resource file (.res) written here byte by byte: one RT_ICON per picture in the .ico
//! and an RT_GROUP_ICON listing them. The MSVC linker takes a .res file as input, so no resource compiler is needed.
use std::{env, fs, path::PathBuf};

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const EN_US: u16 = 0x0409;

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
/// One resource: its header (type and name as numbers) and its data, padded to four bytes.
fn resource(out: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(32u32.to_le_bytes());
    out.extend([0xffff, kind, 0xffff, id].iter().flat_map(|w: &u16| w.to_le_bytes()));
    out.extend(0u32.to_le_bytes());
    out.extend(flags.to_le_bytes());
    out.extend(EN_US.to_le_bytes());
    out.extend([0u8; 8]);
    out.extend(data);
    out.resize(out.len().next_multiple_of(4), 0);
}

fn main() {
    let ico_path = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets/String.ico");
    println!("cargo:rerun-if-changed={}", ico_path.display());
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let ico = fs::read(&ico_path).expect("assets/String.ico");
    assert!(u16_at(&ico, 2) == 1, "assets/String.ico is not an icon file");
    let count = u16_at(&ico, 4);
    // the empty resource every .res file starts with
    let mut res = vec![0, 0, 0, 0, 32, 0, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0];
    res.extend([0u8; 16]);
    let mut group = Vec::new();
    group.extend([0u16, 1, count].iter().flat_map(|w| w.to_le_bytes()));
    for i in 0..count {
        let e = 6 + usize::from(i) * 16;
        let (size, offset) = (u32_at(&ico, e + 8) as usize, u32_at(&ico, e + 12) as usize);
        resource(&mut res, RT_ICON, i + 1, 0x1010, &ico[offset..offset + size]);
        // the group entry: the .ico entry's first 12 bytes, then the picture's resource number
        group.extend(&ico[e..e + 12]);
        group.extend((i + 1).to_le_bytes());
    }
    resource(&mut res, RT_GROUP_ICON, 1, 0x1030, &group);
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("string-icon.res");
    fs::write(&out, res).unwrap();
    println!("cargo:rustc-link-arg-bin=String={}", out.display());
}
