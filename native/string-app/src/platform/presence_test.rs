//! Exercise the shell's actual transport publisher through Discord IPC, including the idle
//! period when nobody sends another playback update. The server has one local pipe and no network.
use super::*;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Write},
    os::windows::io::FromRawHandle,
    thread,
    time::Instant,
};
use string_audio::Status;
use string_core::library::Library;
use windows::{
    Win32::{
        Foundation::{ERROR_NO_DATA, ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING},
        Storage::FileSystem::PIPE_ACCESS_DUPLEX,
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_NOWAIT, PIPE_REJECT_REMOTE_CLIENTS,
        },
    },
    core::HSTRING,
};

struct Server {
    file: File,
    buffer: Vec<u8>,
}
impl Server {
    fn new(name: &str) -> Self {
        let path = HSTRING::from(format!(r"\\.\pipe\{name}"));
        let handle = unsafe {
            CreateNamedPipeW(
                &path,
                PIPE_ACCESS_DUPLEX,
                PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                8192,
                8192,
                0,
                None,
            )
        };
        assert!(!handle.is_invalid(), "{}", io::Error::last_os_error());
        // File owns the handle even if an assertion later fails.
        let file = unsafe { File::from_raw_handle(handle.0) };
        if let Err(e) = unsafe { ConnectNamedPipe(handle, None) } {
            assert!(
                [
                    ERROR_PIPE_LISTENING.to_hresult(),
                    ERROR_PIPE_CONNECTED.to_hresult()
                ]
                .contains(&e.code()),
                "{e}"
            );
        }
        Self {
            file,
            buffer: Vec::new(),
        }
    }
    fn read(&mut self, opcode: u32) -> Value {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if self.buffer.len() >= 8 {
                assert_eq!(
                    u32::from_le_bytes(self.buffer[..4].try_into().unwrap()),
                    opcode
                );
                let len = u32::from_le_bytes(self.buffer[4..8].try_into().unwrap()) as usize;
                if self.buffer.len() >= 8 + len {
                    let value = serde_json::from_slice(&self.buffer[8..8 + len]).unwrap();
                    self.buffer.drain(..8 + len);
                    return value;
                }
            }
            let mut chunk = [0; 4096];
            match self.file.read(&mut chunk) {
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.raw_os_error(), Some(n) if n == ERROR_NO_DATA.0 as i32 || n == ERROR_PIPE_LISTENING.0 as i32) =>
                    {}
                other => panic!("pipe read: {other:?}"),
            }
            assert!(Instant::now() < deadline, "no IPC frame {opcode}");
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn ready(&mut self) {
        let body = br#"{"cmd":"DISPATCH","evt":"READY","data":{"user":{"username":"fixture"}}}"#;
        let mut frame = 1u32.to_le_bytes().to_vec();
        frame.extend((body.len() as u32).to_le_bytes());
        frame.extend(body);
        self.file.write_all(&frame).unwrap();
    }
    fn activity(&mut self) -> Value {
        let f = self.read(1);
        assert_eq!(f["cmd"], "SET_ACTIVITY");
        assert_eq!(f["args"]["pid"], std::process::id());
        f["args"]["activity"].clone()
    }
}

#[test]
fn discord_wire_follows_the_queue_pause_and_expiry() {
    string_platform::presence::with_test_pipe(|pipe| {
        let mut server = Server::new(pipe);
        let dir = tempfile::tempdir().unwrap();
        let log = ErrorLog::new(dir.path());
        let ctx = Context::default();
        let mut library = Library::default();
        for title in ["First", "Middle", "Last"] {
            library.tracks.push(
                serde_json::from_value(json!({"path":format!("{title}.wav"),"title":title}))
                    .unwrap(),
            );
        }
        let mut s = Session::new(Default::default(), Default::default(), library);
        let ids = ["first.wav", "middle.wav", "last.wav"].map(str::to_string);
        assert!(
            s.state
                .queue
                .play_ids(&ids, 0, Value::Null, &s.models.catalog)
        );
        s.set_setting("discord", true.into());
        s.set_setting("discordId", "123456789012345678".into());
        let mut transport = Transport::default();
        let mut publish = |s: &Session, playing, duration| {
            let st = Status {
                key: Some(s.state.queue.at().unwrap() as u64 + 1),
                playing,
                position: 0.,
                duration: Some(duration),
            };
            transport.update(s, st, None, None, dir.path(), &log, &ctx);
        };
        publish(&s, true, 180.);
        assert_eq!(server.read(0)["client_id"], "123456789012345678");
        let earliest_send = Instant::now();
        server.ready();
        assert_eq!(server.activity()["details"], "First");
        for i in [1, 2] {
            assert!(s.state.queue.jump(i, &s.models.catalog));
            publish(&s, true, 180.);
        }
        assert_eq!(server.activity()["details"], "Last");
        // The first send cannot precede READY. This bound tolerates a late server read.
        assert!(earliest_send.elapsed() >= Duration::from_millis(4200));
        publish(&s, false, 180.);
        assert!(
            server.activity().is_null(),
            "pause clears a previously visible activity"
        );
        publish(&s, true, 6.);
        assert_eq!(server.activity()["details"], "Last");
        assert!(
            server.activity().is_null(),
            "the last song expires without another publish"
        );
    });
}
