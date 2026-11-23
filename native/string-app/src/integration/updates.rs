use crate::services::{UpdateInfo, UpdateProgress};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};
use string_extras::updates::{InstallPlan, Progress, Updater, Version};

pub struct Updates {
    root: PathBuf,
    smoke: bool,
    ready: Mutex<Option<InstallPlan>>,
    pub info: UpdateInfo,
}

fn updater() -> io::Result<Updater> {
    let exe = env::current_exe()?;
    let install = exe
        .parent()
        .ok_or_else(|| io::Error::other("Missing installation folder"))?;
    let source = Updater::new(install, env!("CARGO_PKG_VERSION"))?;
    if !source.installed() {
        return Ok(source);
    }
    let version = fs::read_to_string(install.join("setup/version.txt"))?;
    Updater::new(install, version.trim_start_matches('\u{feff}').trim())
}

impl Updates {
    pub fn new(root: &Path, smoke: bool) -> Self {
        let mut info = UpdateInfo {
            current: env!("CARGO_PKG_VERSION").into(),
            published: true,
            ..Default::default()
        };
        if !smoke
            && let Ok(exe) = env::current_exe()
            && let Some(dir) = exe.parent()
            && Updater::new(dir, env!("CARGO_PKG_VERSION")).is_ok_and(|up| up.installed())
        {
            info.installed = true;
            if let Ok(version) = fs::read_to_string(dir.join("setup/version.txt")) {
                info.current = version.trim_start_matches('\u{feff}').trim().into();
            }
        }
        Self {
            root: root.into(),
            smoke,
            ready: Mutex::default(),
            info,
        }
    }

    /// Called by the Settings worker.
    pub fn check(&self) -> io::Result<UpdateInfo> {
        if self.smoke {
            return Ok(self.info.clone());
        }
        let c = updater()?.check()?;
        let mut info = UpdateInfo {
            current: c.current.to_string(),
            installed: c.installed,
            newer: c.newer,
            published: true,
            ..Default::default()
        };
        if let Some(release) = c.latest {
            info.latest = release.version.to_string();
            info.notes = release.notes;
        }
        Ok(info)
    }

    pub fn prepare(
        &self,
        want: &str,
        progress: &mut dyn FnMut(UpdateProgress),
    ) -> io::Result<Version> {
        if self.smoke {
            return Err(io::Error::other("Updates are unavailable in smoke mode"));
        }
        let mut ready = self
            .ready
            .try_lock()
            .map_err(|_| io::Error::other("An update is in progress"))?;
        *ready = None;
        let plan = updater()?.prepare(&self.root, want, |p| {
            progress(match p {
                Progress::Receiving { got, total } => UpdateProgress::Receiving { got, total },
                Progress::Verifying => UpdateProgress::Verifying,
            })
        })?;
        let version = plan.version;
        *ready = Some(plan);
        Ok(version)
    }

    pub fn launch(&self, version: Version) -> io::Result<()> {
        let mut ready = self
            .ready
            .try_lock()
            .map_err(|_| io::Error::other("An update is in progress"))?;
        let plan = ready
            .as_ref()
            .filter(|p| p.version == version)
            .ok_or_else(|| io::Error::other("Prepare the update first"))?;
        if self.smoke || !updater()?.installed() {
            return Err(io::Error::other("Only an installed String updates itself"));
        }
        plan.spawn()?;
        *ready = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn smoke_cannot_prepare_or_start_an_installer() {
        let u = Updates::new(Path::new("."), true);
        assert!(!u.check().unwrap().installed);
        assert!(u.prepare("2.0.0", &mut |_| {}).is_err());
        assert!(u.launch("2.0.0".parse().unwrap()).is_err());
    }
}
