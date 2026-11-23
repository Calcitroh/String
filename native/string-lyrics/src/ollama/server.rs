use std::{
    env,
    ffi::c_void,
    io,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    ptr,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use ureq::Agent;

// The job contains just our child and any model runners it spawns. Termination
// addresses the job handle and cannot hit a different server that took its port.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
    fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
    fn TerminateJobObject(job: *mut c_void, exit_code: u32) -> i32;
}
struct OwnedServer {
    child: Child,
    job: OwnedHandle,
}
impl OwnedServer {
    fn spawn(mut command: Command) -> io::Result<Self> {
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // CreateJobObjectW returned a fresh, non-null owned handle.
        let job = unsafe { OwnedHandle::from_raw_handle(handle) };
        let mut child = command
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) } == 0 {
            let err = io::Error::last_os_error();
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
        Ok(Self { child, job })
    }
}
impl Drop for OwnedServer {
    fn drop(&mut self) {
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 0) } == 0 {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

type Launcher = Box<dyn Fn() -> io::Result<Command> + Send + Sync>;
pub(super) struct Server {
    pub(super) base: String,
    launcher: Option<Launcher>,
    owned: Mutex<Option<OwnedServer>>,
    closed: AtomicBool,
}
impl Server {
    pub(super) fn installed() -> Self {
        let mut server = Self::external("http://127.0.0.1:11434");
        server.launcher = Some(Box::new(|| {
            let exe = env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .map(|p| p.join("Programs/Ollama/ollama.exe"))
                .filter(|p| p.is_file())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "Ollama isn't installed (it's free at ollama.com)",
                    )
                })?;
            let mut command = Command::new(&exe);
            command.arg("serve").current_dir(exe.parent().unwrap());
            Ok(command)
        }));
        server
    }
    pub(super) fn external(base: &str) -> Self {
        Self {
            base: base.into(),
            launcher: None,
            owned: Mutex::default(),
            closed: AtomicBool::new(false),
        }
    }
    fn up(&self, http: &Agent) -> bool {
        http.get(format!("{}/api/version", self.base))
            .config()
            .timeout_global(Some(Duration::from_millis(1500)))
            .build()
            .call()
            .is_ok_and(|r| r.status().is_success())
    }
    pub(super) fn ensure(&self, http: &Agent, stopped: impl Fn() -> bool) -> io::Result<()> {
        let check = || {
            if self.closed.load(Ordering::Acquire) || stopped() {
                Err(io::Error::other("The translation stopped"))
            } else {
                Ok(())
            }
        };
        {
            let mut owned = self.owned.lock().unwrap();
            check()?;
            if self.up(http) {
                return Ok(());
            }
            let launcher = self
                .launcher
                .as_ref()
                .ok_or_else(|| io::Error::other("Ollama isn't running"))?;
            check()?;
            if !matches!(
                owned.as_mut().map(|s| s.child.try_wait()).transpose()?,
                Some(None)
            ) {
                *owned = Some(OwnedServer::spawn(launcher()?)?);
            }
        }
        for _ in 0..60 {
            thread::sleep(Duration::from_millis(250));
            check()?;
            if self.up(http) {
                return Ok(());
            }
        }
        Err(io::Error::other("Ollama didn't start"))
    }
    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.owned.lock().unwrap().take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ollama::local_http;
    use std::net::TcpListener;

    #[test]
    fn only_the_starter_owns_the_fake_server() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let mut owner = Server::external(&format!("http://{addr}"));
        owner.launcher = Some(Box::new(move || {
            let mut cmd = Command::new("node");
            cmd.arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/service.cjs"))
                .args(["ollama", "tree"])
                .arg(addr.port().to_string());
            Ok(cmd)
        }));
        drop(listener);
        let http = local_http();
        owner.ensure(&http, || false).unwrap();
        let id = owner.owned.lock().unwrap().as_ref().unwrap().child.id();
        owner.ensure(&http, || false).unwrap();
        assert_eq!(owner.owned.lock().unwrap().as_ref().unwrap().child.id(), id);
        let mut guest = Server::external(&owner.base);
        guest.launcher = Some(Box::new(|| panic!("already running")));
        guest.ensure(&http, || false).unwrap();
        guest.close();
        assert!(owner.up(&http));
        let runner: u16 = http
            .get(format!("{}/_runner", owner.base))
            .call()
            .unwrap()
            .body_mut()
            .read_json()
            .unwrap();
        let runner = ("127.0.0.1", runner);
        assert!(std::net::TcpStream::connect(runner).is_ok());
        owner.close();
        assert!(!owner.up(&http));
        assert!(std::net::TcpStream::connect(runner).is_err());
        assert!(owner.ensure(&http, || false).is_err());
    }
}
