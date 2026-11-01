use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Where Countersign keeps its files.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `~/.config/countersign/settings.toml`
    pub settings_file: PathBuf,
    /// The official CLI's private profile, separate from any `bw` profile
    /// you use yourself.
    pub profile_dir: PathBuf,
    /// Checksum-verified CLI releases installed by Countersign.
    pub managed_cli_root: PathBuf,
    pub runtime_dir: PathBuf,
    pub socket: PathBuf,
    pub lock_file: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        let config = dirs::config_dir().context("no config folder")?.join("countersign");
        let data = dirs::data_dir().context("no data folder")?.join("countersign");
        let runtime_dir = countersign_ipc::runtime_dir();
        Ok(Paths {
            settings_file: config.join("settings.toml"),
            profile_dir: data.join("profile"),
            managed_cli_root: data.join("bitwarden-cli"),
            socket: runtime_dir.join("daemon.sock"),
            lock_file: runtime_dir.join("daemon.lock"),
            runtime_dir,
        })
    }

    /// The managed executable for one CLI release.
    pub fn managed_cli(&self, version: &str) -> PathBuf {
        self.managed_cli_root.join(version).join("bw")
    }
}

/// Creates `path` with mode 0700 if needed, then checks that the current
/// user owns it and that nobody else can read it.
pub fn ensure_private_dir(path: &Path) -> Result<()> {
    match fs::DirBuilder::new().recursive(true).mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
    }
    let meta = fs::symlink_metadata(path).with_context(|| format!("checking {}", path.display()))?;
    if !meta.is_dir() {
        bail!("{} is not a folder", path.display());
    }
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if meta.uid() != uid {
        bail!("{} belongs to another user", path.display());
    }
    if meta.mode() & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting {}", path.display()))?;
    }
    Ok(())
}

/// Writes a file atomically with mode 0600.
pub fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path.parent().context("file without a folder")?;
    fs::create_dir_all(parent)?;
    let tmp =
        parent.join(format!(".{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("countersign")));
    let mut file = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_dirs_are_created_and_tightened() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("a/b");
        ensure_private_dir(&dir).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&dir).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
    }

    #[test]
    fn private_files_are_written_atomically() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("x/settings.toml");
        write_private(&file, b"a = 1").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"a = 1");
        assert_eq!(fs::metadata(&file).unwrap().mode() & 0o777, 0o600);
    }
}
