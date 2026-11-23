//! ADTS and abbreviated MP4 configurations can both identify only the LC core. Symphonia
//! parses SBR fill elements but keeps that result private. When the platform decoder is
//! unavailable, recognize those elements without mistaking spectral or ancillary bits for SBR.
use super::{Error, codecs};
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::packet::PacketRef;
use symphonia::core::units::{Duration, Timestamp};

pub(super) struct ProfileCheck {
    parser: Box<dyn AudioDecoder>,
    // The LC parser reports these two unsupported syntax elements distinctly.
    coupling: &'static str,
    program_config: &'static str,
    prefix: Vec<u8>,
}

fn rejection(parser: &mut dyn AudioDecoder, bytes: &[u8]) -> Option<&'static str> {
    parser.reset();
    let packet = PacketRef::new(0, Timestamp::ZERO, Duration::ZERO, bytes);
    match parser.decode_ref(&packet) {
        Err(SymError::Unsupported(reason)) => Some(reason),
        _ => None,
    }
}

fn bits(data: &[u8], at: usize, width: usize) -> u8 {
    let i = at / 8;
    let word = u16::from_be_bytes([data[i], data.get(i + 1).copied().unwrap_or(0)]);
    ((word >> (16 - at % 8 - width)) & ((1 << width) - 1)) as u8
}

impl ProfileCheck {
    pub fn new(params: &AudioCodecParameters) -> Option<Self> {
        let mut parser = codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .ok()?;
        let coupling = rejection(parser.as_mut(), &[0b01000000, 0])?;
        let program_config = rejection(parser.as_mut(), &[0b10100000, 0])?;
        (coupling != program_config).then_some(Self {
            parser,
            coupling,
            program_config,
            prefix: Vec::new(),
        })
    }

    pub fn check(&mut self, data: &[u8]) -> Result<(), Error> {
        // ADTS's 13-bit frame size also exceeds the maximum mono/stereo LC access unit.
        if data.len() > 8191 {
            return Err(Error::Decode("cannot verify AAC profile".into()));
        }
        let end = data.len() * 8;
        for at in 0..end.saturating_sub(10) {
            if bits(data, at, 3) != 6 {
                continue;
            } // ID_FIL
            let mut count = usize::from(bits(data, at + 3, 4));
            let mut payload = at + 7;
            if count == 15 {
                if payload + 8 > end {
                    continue;
                }
                count = 14 + usize::from(bits(data, payload, 8));
                payload += 8;
            }
            if count == 0 || payload + 8 * count > end || !matches!(bits(data, payload, 4), 13 | 14)
            {
                continue;
            }
            // Ask the LC syntax parser whether this bit is an element boundary. Two
            // different terminal elements must both be recognized at the same prefix.
            // This reuses its Huffman/ICS parser and never interprets arbitrary matches
            // inside a channel element or an opaque data-stream element as extensions.
            let i = at / 8;
            let shift = 13 - at % 8;
            self.prefix.clear();
            self.prefix.extend_from_slice(&data[..i]);
            let keep = !(0xffu8 >> (at % 8));
            let before = u16::from(data[i] & keep) << 8;
            self.prefix
                .extend_from_slice(&(before | 2 << shift).to_be_bytes());
            if rejection(self.parser.as_mut(), &self.prefix) == Some(self.coupling) {
                self.prefix[i..].copy_from_slice(&(before | 5 << shift).to_be_bytes());
                if rejection(self.parser.as_mut(), &self.prefix) == Some(self.program_config) {
                    return Err(Error::Decode(
                        "HE-AAC requires the Windows media decoder".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}
