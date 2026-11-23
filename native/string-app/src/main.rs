#![windows_subsystem = "windows"]
mod add_work;
pub mod app;
#[cfg(test)]
mod checks;
pub mod integration;
pub mod model;
pub mod platform;
pub mod playback;
pub mod player_bar;
mod playlist_add;
pub mod route;
pub mod scan;
pub mod screens;
pub mod services;
pub mod shell;
pub mod smoke;
pub mod storage;
pub mod theme;
pub mod ui;
pub mod view;
pub mod viz;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    },
};
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub struct Args {
    pub root: PathBuf,
    pub explicit: bool,
    pub smoke: Option<PathBuf>,
    pub quit: bool,
    /// Absolute path arguments. A first instance keeps the existing ones after its worker checks them.
    pub add: Option<Vec<PathBuf>>,
    pub playlist: Option<String>,
}
#[derive(Debug)]
struct PlaylistUsage;
impl std::fmt::Display for PlaylistUsage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Usage: String --add --playlist <non-empty name> <path>...")
    }
}
impl std::error::Error for PlaylistUsage {}
fn resolved(path: &Path) -> std::io::Result<PathBuf> {
    if path.exists() {
        return path.canonicalize();
    }
    let absolute = std::path::absolute(path)?;
    let parent = absolute.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    Ok(resolved(parent)?.join(
        absolute
            .file_name()
            .ok_or(std::io::ErrorKind::InvalidInput)?,
    ))
}
impl Args {
    fn parse() -> Result<Self, Error> {
        Self::options(std::env::args_os().skip(1))
    }
    /// The arguments of a first instance, after its worker has checked the locations.
    #[cfg(test)]
    pub(crate) fn parse_from(args: impl IntoIterator<Item = OsString>) -> Result<Self, Error> {
        let mut args = Self::options(args)?;
        if let Some(paths) = args.add.take() {
            args.add = Some(
                add_work::classify(paths)
                    .into_iter()
                    .map(|l| l.path)
                    .collect(),
            );
        }
        Ok(args)
    }
    fn options(args: impl IntoIterator<Item = OsString>) -> Result<Self, Error> {
        let mut args = args.into_iter().peekable();
        let (mut root, mut out, mut smoke) = (None, None, false);
        let mut quit = false;
        let mut add: Option<Vec<PathBuf>> = None;
        let mut playlist = None;
        while let Some(k) = args.next() {
            match k.to_str() {
                Some("--smoke-test") => smoke = true,
                Some("--quit") => quit = true,
                // every argument up to the next option: no absolute path starts with "--"
                Some("--add" | "--playlist") => {
                    if k == "--playlist" {
                        if add.is_none() || playlist.is_some() {
                            return Err(PlaylistUsage.into());
                        }
                        let name = args
                            .next()
                            .and_then(|n| n.into_string().ok())
                            .ok_or(PlaylistUsage)?;
                        let name = name.trim();
                        if name.is_empty() {
                            return Err(PlaylistUsage.into());
                        }
                        playlist = Some(name.to_owned());
                    }
                    let list = add.get_or_insert_with(Vec::new);
                    while let Some(p) = args.next_if(|a| !a.to_string_lossy().starts_with("--")) {
                        // one that can't be made absolute (an empty one) is skipped, as a missing one is
                        if !p.is_empty()
                            && let Ok(p) = std::path::absolute(PathBuf::from(p))
                        {
                            list.push(p);
                        }
                    }
                }
                Some("--data") => {
                    root = Some(PathBuf::from(
                        args.next().ok_or("Missing --data directory")?,
                    ))
                }
                Some("--out") => {
                    out = Some(PathBuf::from(args.next().ok_or("Missing --out directory")?))
                }
                _ => {
                    return Err(
                        "Usage: String [--quit] [--data <dir>] [--smoke-test --out <dir>] [--add [--playlist <name>] <path>...]"
                            .into(),
                    );
                }
            }
        }
        if playlist.is_some() && add.as_ref().is_none_or(Vec::is_empty) {
            return Err(PlaylistUsage.into());
        }
        let explicit = root.is_some();
        let default = string_core::data::default_data_dir().ok_or("Missing profile location")?;
        let root = root.unwrap_or_else(|| default.clone());
        if smoke {
            let out = out.ok_or("Smoke mode requires --out")?;
            if !explicit {
                return Err("Smoke mode requires --data".into());
            }
            let real = resolved(default.parent().ok_or("Invalid profile location")?)?
                .join("String")
                .to_string_lossy()
                .to_lowercase();
            for p in [&root, &out] {
                if Path::new(&resolved(p)?.to_string_lossy().to_lowercase()).starts_with(&real) {
                    return Err(
                        "Smoke mode requires an isolated profile and output directory".into(),
                    );
                }
            }
            std::fs::create_dir_all(&out)?;
            return Ok(Self {
                root,
                explicit,
                smoke: Some(out),
                quit,
                add,
                playlist,
            });
        }
        if out.is_some() {
            return Err("--out requires --smoke-test".into());
        }
        Ok(Self {
            root,
            explicit,
            smoke: None,
            quit,
            add,
            playlist,
        })
    }
}
fn run() -> Result<i32, Error> {
    let mut args = Args::parse()?;
    let auto = args.smoke.is_some();
    let mut startup_adds = None;
    let start = if !args.quit
        && let Some(paths) = &args.add
    {
        use string_platform::instance::Names;
        let names = if auto {
            Names::smoke()
        } else {
            Names::string()
        };
        playlist_add::claim(
            &names,
            args.playlist.as_deref(),
            paths,
            std::time::Duration::from_secs(5),
            || Ok(startup_adds.insert(add_work::Adds::new()?).input.clone()),
        )?
    } else {
        platform::claim(&args)?
    };
    let mut instance = match start {
        platform::Start::Run(instance) => instance,
        platform::Start::Done(code) => return Ok(code),
    };
    let adds = if let Some(mut adds) = startup_adds {
        let paths = adds.finish_startup()?;
        if paths.is_empty() && adds.close_if_idle() {
            return Ok(0);
        }
        args.add = Some(paths);
        adds
    } else {
        let adds = add_work::Adds::new()?;
        let input = adds.input.clone();
        instance.route_adds(move |name, paths, bytes| input.submit(name, paths, bytes));
        adds
    };
    std::fs::create_dir_all(&args.root)?;
    let log = Arc::new(string_platform::errors_log::ErrorLog::new(&args.root));
    string_platform::errors_log::log_panics(log.clone());
    let mut platform = platform::Platform::new(instance, &args, log.clone());
    platform.take_added(); // the classified startup batch owns these paths
    let exit = Arc::new(AtomicI32::new(i32::from(auto)));
    let result = exit.clone();
    let opts = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        centered: false,
        persist_window: false,
        viewport: platform.configure(
            egui::ViewportBuilder::default()
                .with_inner_size([1400., 880.])
                .with_min_inner_size([820., 520.])
                .with_visible(false)
                .with_icon(
                    eframe::icon_data::from_png_bytes(include_bytes!("../assets/String.png"))
                        .unwrap_or_default(),
                ),
        ),
        wgpu_options: egui_wgpu::WgpuConfiguration {
            surface: egui_wgpu::SurfaceConfig {
                present_mode: egui_wgpu::wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: Some(2),
            },
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "String",
        opts,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, args, platform, adds, exit)?))),
    )
    .map_err(|e| {
        log.write("startup", &e);
        e.to_string()
    })?;
    Ok(result.load(Ordering::Acquire))
}
fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(if e.is::<PlaylistUsage>() { 2 } else { 1 })
        }
    }
}
