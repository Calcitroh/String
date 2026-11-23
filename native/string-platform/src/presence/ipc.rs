//! Discord's local pipe (`\\.\pipe\discord-ipc-N`): frames of an opcode and a length (little-endian u32s), then JSON.
//! Reads never block (PeekNamedPipe says what has arrived), so one thread both listens and writes: a blocking read on a
//! pipe opened this way would hold up every write until something came in.

use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::io::AsRawHandle;
use windows::Win32::Foundation::{ERROR_PIPE_BUSY, HANDLE};
use windows::Win32::System::Pipes::{PeekNamedPipe, WaitNamedPipeW};
use windows::core::HSTRING;

pub(crate) const HANDSHAKE: u32 = 0;
pub(crate) const FRAME: u32 = 1;
pub(crate) const CLOSE: u32 = 2;
pub(crate) const PING: u32 = 3;
pub(crate) const PONG: u32 = 4;

/// Frames longer than this are nonsense: the connection is dropped.
const LONGEST: usize = 1 << 20;

pub(crate) struct Frame {
    pub op: u32,
    pub raw: String,
    pub json: Value,
}

pub(crate) struct Pipe {
    file: File,
    buf: Vec<u8>,
}

impl Pipe {
    /// The first of `names` that answers (a busy one is given 0.4 s); `None` if none does.
    pub fn connect(names: &[String]) -> Option<Pipe> {
        for name in names {
            let path = format!(r"\\.\pipe\{name}");
            for attempt in 0..2 {
                match OpenOptions::new().read(true).write(true).open(&path) {
                    Ok(file) => {
                        return Some(Pipe {
                            file,
                            buf: Vec::new(),
                        });
                    }
                    Err(e)
                        if attempt == 0 && e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) =>
                    unsafe {
                        let _ = WaitNamedPipeW(&HSTRING::from(path.as_str()), 400);
                    },
                    Err(_) => break,
                }
            }
        }
        None
    }

    pub fn send(&mut self, op: u32, json: &str) -> io::Result<()> {
        let mut frame = Vec::with_capacity(8 + json.len());
        frame.extend_from_slice(&op.to_le_bytes());
        frame.extend_from_slice(&(json.len() as u32).to_le_bytes());
        frame.extend_from_slice(json.as_bytes());
        self.file.write_all(&frame)?;
        self.file.flush()
    }

    /// The whole frames that have arrived (none is fine); an error once Discord has gone or sends nonsense.
    pub fn poll(&mut self) -> io::Result<Vec<Frame>> {
        loop {
            let mut waiting = 0u32;
            let handle = HANDLE(self.file.as_raw_handle());
            unsafe { PeekNamedPipe(handle, None, 0, None, Some(&mut waiting), None) }?;
            if waiting == 0 {
                break;
            }
            let mut chunk = vec![0u8; waiting.min(64 * 1024) as usize];
            let n = self.file.read(&mut chunk)?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
        let mut frames = Vec::new();
        while self.buf.len() >= 8 {
            let op = u32::from_le_bytes(self.buf[0..4].try_into().unwrap());
            let len = u32::from_le_bytes(self.buf[4..8].try_into().unwrap()) as usize;
            if len > LONGEST {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "frame too large",
                ));
            }
            if self.buf.len() < 8 + len {
                break;
            }
            let raw = String::from_utf8_lossy(&self.buf[8..8 + len]).into_owned();
            self.buf.drain(..8 + len);
            let json = serde_json::from_str(&raw).unwrap_or(Value::Null);
            frames.push(Frame { op, raw, json });
        }
        Ok(frames)
    }
}
