//! Lofty's Ogg writer repaginates the headers and stops at the first unreadable
//! trailing page. Verify the complete stream before accepting its staged output.
use super::invalid;
use sha2::{Digest, Sha256};
use std::io::{self, BufRead};

const CRC: [u32; 256] = {
    let mut table = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut v = (i as u32) << 24;
        let mut j = 0;
        while j < 8 {
            v = if v & 0x80000000 != 0 {
                (v << 1) ^ 0x04c11db7
            } else {
                v << 1
            };
            j += 1;
        }
        table[i] = v;
        i += 1;
    }
    table
};
pub(super) fn packets(mut src: impl BufRead) -> io::Result<[u8; 32]> {
    let mut hash = Sha256::new();
    let mut sequence = 0u32;
    let mut serial = None;
    let mut finished = false;
    let mut packet = 0u64;
    let mut packet_len = 0u64;
    let mut page = Vec::new();
    while !src.fill_buf()?.is_empty() {
        let mut header = [0u8; 27];
        src.read_exact(&mut header)?;
        let flags = header[5];
        let number = u32::from_le_bytes(header[18..22].try_into().unwrap());
        let stream = u32::from_le_bytes(header[14..18].try_into().unwrap());
        if finished
            || &header[..5] != b"OggS\0"
            || number != sequence
            || (flags & 2 != 0) != serial.is_none()
            || (flags & 1 != 0) != (packet_len != 0)
            || *serial.get_or_insert(stream) != stream
        {
            return Err(invalid("Expected one complete Ogg stream"));
        }
        let mut laces = [0u8; 255];
        let laces = &mut laces[..header[26] as usize];
        src.read_exact(laces)?;
        page.resize(laces.iter().map(|&s| usize::from(s)).sum(), 0);
        src.read_exact(&mut page)?;
        let checksum = u32::from_le_bytes(header[22..26].try_into().unwrap());
        header[22..26].fill(0);
        let crc = header
            .iter()
            .chain(laces.iter())
            .chain(&page)
            .fold(0u32, |crc, &b| {
                (crc << 8) ^ CRC[((crc >> 24) as u8 ^ b) as usize]
            });
        if checksum != crc {
            return Err(invalid("The Ogg page checksum is invalid"));
        }
        let mut offset = 0;
        for &lace in laces.iter() {
            let end = offset + usize::from(lace);
            if packet != 1 {
                hash.update(&page[offset..end]);
            }
            packet_len += u64::from(lace);
            if lace < 255 {
                if packet != 1 {
                    hash.update(packet_len.to_le_bytes());
                }
                packet += 1;
                packet_len = 0;
            }
            offset = end;
        }
        sequence = sequence.wrapping_add(1);
        finished = flags & 4 != 0;
        if finished {
            hash.update(&header[6..14]);
        }
    }
    if !finished || packet_len != 0 || packet < 3 {
        return Err(invalid("The Ogg stream is incomplete"));
    }
    Ok(hash.finalize().into())
}
