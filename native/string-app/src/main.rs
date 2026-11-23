#![windows_subsystem = "windows"]
pub mod app;
#[cfg(test)]
mod checks;
pub mod integration;
pub mod model;
pub mod platform;
pub mod playback;
pub mod player_bar;
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
}
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
        let mut args = std::env::args_os().skip(1);
        let (mut root, mut out, mut smoke) = (None, None, false);
        let mut quit = false;
        while let Some(k) = args.next() {
            match k.to_str() {
                Some("--smoke-test") => smoke = true,
                Some("--quit") => quit = true,
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
                        "Usage: String [--quit] [--data <dir>] [--smoke-test --out <dir>]".into(),
                    );
                }
            }
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
        })
    }
}
fn run() -> Result<i32, Error> {
    let args = Args::parse()?;
    let auto = args.smoke.is_some();
    let Some(instance) = platform::claim(auto, args.quit)? else {
        return Ok(0);
    };
    std::fs::create_dir_all(&args.root)?;
    let log = Arc::new(string_platform::errors_log::ErrorLog::new(&args.root));
    string_platform::errors_log::log_panics(log.clone());
    let platform = platform::Platform::new(instance, &args, log.clone());
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
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, args, platform, exit)?))),
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
            std::process::exit(1)
        }
    }
}
