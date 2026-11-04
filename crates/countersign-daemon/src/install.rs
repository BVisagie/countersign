//! Installs the pinned official CLI release for Countersign's own use.
//!
//! Runs only when you ask. The archive comes from Bitwarden's GitHub release
//! over HTTPS and must match the SHA-256 recorded with the pin before
//! anything is extracted. The extracted `bw` must report exactly the pinned
//! version in an empty profile before it replaces anything. It never touches
//! a system package or a CLI you configured yourself.
//!
//! When a Countersign update raises the pin, the same request installs the
//! new release over the old one. The download and checks run while the vault
//! stays open; only the switch locks it. Older releases Countersign installed
//! are then removed.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use countersign_bw::{Cli, CliVersion, PINNED_VERSION, PinnedArchive};
use countersign_ipc::{CliState, ErrorCode, IpcError};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};

use crate::core::{Daemon, LockReason};
use crate::paths::ensure_private_dir;

/// Larger than any official archive; a bigger download is refused.
const ARCHIVE_LIMIT: u64 = 200 << 20;
/// Larger than the official executable.
const BINARY_LIMIT: u64 = 400 << 20;

fn failed(message: impl Into<String>) -> IpcError {
    IpcError::new(ErrorCode::CliUnavailable, message)
}

pub async fn install(daemon: &Arc<Daemon>) -> Result<(), IpcError> {
    let _op = daemon.ops.lock().await;
    let current = daemon.status().cli;
    if let CliState::Ready { version, managed: true, .. } = &current
        && *version == PINNED_VERSION.to_string()
    {
        return Ok(());
    }
    // A working CLI keeps the vault usable while the new one downloads.
    let first = !matches!(current, CliState::Ready { .. });
    if first {
        daemon.update_status(|s| s.cli = CliState::Installing);
    }
    let root = &daemon.paths.managed_cli_root;
    let staged = match fetch(root).await {
        Ok(staged) => staged,
        Err(e) => {
            tracing::warn!("installing the Bitwarden CLI failed: {}", e.message);
            daemon.refresh().await;
            return Err(e);
        }
    };
    // Nothing may keep running the copy being replaced, nor start from it at
    // the next unlock, so lock as choosing another CLI does.
    daemon.lock_within_op(LockReason::Requested).await;
    let result = place(root, staged);
    match &result {
        // Installing means using the managed CLI, even over a custom path.
        Ok(()) if daemon.settings().cli_path.is_some() => {
            let settings = countersign_ipc::Settings { cli_path: None, ..daemon.settings() };
            if let Err(e) = crate::settings::save(&daemon.paths.settings_file, &settings) {
                daemon.refresh().await;
                return Err(failed(format!(
                    "The CLI is installed, but switching to it failed: {e}. Choose it in Settings."
                )));
            }
            daemon.set_settings(settings);
        }
        Ok(()) => {}
        Err(e) => tracing::warn!("installing the Bitwarden CLI failed: {}", e.message),
    }
    daemon.refresh().await;
    if result.is_ok() {
        remove_older(root);
    }
    result
}

/// A downloaded, checked release, not yet in place.
struct Staged {
    /// Removed with everything left in it when dropped.
    dir: tempfile::TempDir,
    binary: PathBuf,
}

/// Downloads the pinned release into a staging folder under `root` and
/// checks its checksum and version. Changes nothing outside that folder.
async fn fetch(root: &Path) -> Result<Staged, IpcError> {
    let archive = PinnedArchive::for_this_machine()
        .ok_or_else(|| failed(format!("Bitwarden publishes no CLI for {} Linux.", std::env::consts::ARCH)))?;
    ensure_private_dir(root).map_err(|e| failed(e.to_string()))?;
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(root)
        .map_err(|e| failed(format!("Cannot prepare the install: {e}")))?;
    fs::set_permissions(staging.path(), fs::Permissions::from_mode(0o700))
        .map_err(|e| failed(e.to_string()))?;

    let zip_path = staging.path().join(archive.file);
    let digest = download(&archive.url(), &zip_path).await?;
    if !digest.eq_ignore_ascii_case(archive.sha256) {
        return Err(failed(
            "The downloaded CLI did not match Bitwarden's published checksum, so it was not installed.",
        ));
    }

    let binary = staging.path().join("bw");
    {
        let (zip_path, binary) = (zip_path.clone(), binary.clone());
        tokio::task::spawn_blocking(move || extract_bw(&zip_path, &binary))
            .await
            .map_err(|e| failed(e.to_string()))??;
    }

    let profile = staging.path().join("profile");
    fs::create_dir(&profile).map_err(|e| failed(e.to_string()))?;
    let version = Cli::new(&binary, &profile).version().await.map_err(|e| failed(e.to_string()))?;
    if version != PINNED_VERSION {
        return Err(failed(format!("The downloaded CLI reports {version}, not {PINNED_VERSION}.")));
    }
    Ok(Staged { dir: staging, binary })
}

/// Moves a checked release into `root`, replacing any copy of the same
/// release, and puts the old copy back if that fails.
fn place(root: &Path, staged: Staged) -> Result<(), IpcError> {
    let target_dir = root.join(PINNED_VERSION.to_string());
    let next = staged.dir.path().join("release");
    fs::DirBuilder::new().mode(0o700).create(&next).map_err(|e| failed(e.to_string()))?;
    fs::rename(&staged.binary, next.join("bw")).map_err(|e| failed(e.to_string()))?;
    let old = staged.dir.path().join("previous");
    if target_dir.exists() {
        fs::rename(&target_dir, &old).map_err(|e| failed(e.to_string()))?;
    }
    if let Err(e) = fs::rename(&next, &target_dir) {
        // Put the previous copy back rather than lose it with the staging folder.
        if old.exists() {
            let _ = fs::rename(&old, &target_dir);
        }
        return Err(failed(format!("Cannot install the CLI: {e}")));
    }
    tracing::info!("installed the official Bitwarden CLI {PINNED_VERSION}");
    Ok(())
}

/// Removes the other releases Countersign installed before, once the pinned
/// one is in use. A copy that cannot be removed is only logged.
fn remove_older(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(version) = name.to_str().and_then(CliVersion::parse) else { continue };
        if version == PINNED_VERSION || !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        match fs::remove_dir_all(entry.path()) {
            Ok(()) => tracing::info!("removed the Bitwarden CLI {version} Countersign installed before"),
            Err(e) => tracing::warn!("removing the Bitwarden CLI {version}: {e}"),
        }
    }
}

/// Streams the archive to `path`, returning its SHA-256 in hex.
async fn download(url: &str, path: &Path) -> Result<String, IpcError> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(600))
        .user_agent(concat!("countersign/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| failed(e.to_string()))?;
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| download_failed(&e))?;
    if response.content_length().is_some_and(|n| n > ARCHIVE_LIMIT) {
        return Err(failed("The CLI download is larger than expected."));
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| failed(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| download_failed(&e))?;
        total += chunk.len() as u64;
        if total > ARCHIVE_LIMIT {
            return Err(failed("The CLI download is larger than expected."));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).map_err(|e| failed(e.to_string()))?;
    }
    file.sync_all().map_err(|e| failed(e.to_string()))?;
    Ok(hex::encode(hasher.finalize()))
}

/// Why a download failed, in words. reqwest's own message is mostly the
/// URL, and the app shows this one, so the full error goes to the log.
fn download_failed(e: &reqwest::Error) -> IpcError {
    let mut cause = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(next) = source {
        cause.push_str(": ");
        cause.push_str(&next.to_string());
        source = next.source();
    }
    tracing::warn!("downloading the Bitwarden CLI: {cause}");
    failed(match e.status() {
        Some(status) => {
            format!("The Bitwarden CLI download failed: GitHub answered {status}. Try again later.")
        }
        None if e.is_timeout() => {
            "The Bitwarden CLI download timed out. Check your internet connection and try again.".to_owned()
        }
        None => {
            "Countersign couldn't download the Bitwarden CLI. Check your internet connection and try again."
                .to_owned()
        }
    })
}

/// Extracts only the `bw` executable, with a size limit.
fn extract_bw(zip_path: &Path, binary: &PathBuf) -> Result<(), IpcError> {
    let file = fs::File::open(zip_path).map_err(|e| failed(e.to_string()))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| failed(format!("The CLI archive is unreadable: {e}")))?;
    let entry = zip.by_name("bw").map_err(|_| failed("The CLI archive has no bw executable."))?;
    if entry.size() > BINARY_LIMIT {
        return Err(failed("The CLI executable is larger than expected."));
    }
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(binary)
        .map_err(|e| failed(e.to_string()))?;
    let copied = io::copy(&mut io::Read::take(entry, BINARY_LIMIT + 1), &mut out)
        .map_err(|e| failed(e.to_string()))?;
    if copied > BINARY_LIMIT {
        return Err(failed("The CLI executable is larger than expected."));
    }
    out.sync_all().map_err(|e| failed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(root: &Path, version: &str, contents: &str) {
        fs::create_dir_all(root.join(version)).unwrap();
        fs::write(root.join(version).join("bw"), contents).unwrap();
    }

    /// The checked release replaces any copy of the pin, and afterwards
    /// only the pin is left; files that are not releases stay.
    #[test]
    fn switching_keeps_only_the_pinned_release() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        release(root, "2026.10.0", "old");
        release(root, &PINNED_VERSION.to_string(), "stale");
        fs::create_dir(root.join("notes")).unwrap();

        let dir = tempfile::Builder::new().prefix(".install-").tempdir_in(root).unwrap();
        let binary = dir.path().join("bw");
        fs::write(&binary, "new").unwrap();
        place(root, Staged { dir, binary }).unwrap();
        remove_older(root);

        let mut left: Vec<String> =
            fs::read_dir(root).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, [PINNED_VERSION.to_string(), "notes".to_owned()]);
        assert_eq!(fs::read_to_string(root.join(PINNED_VERSION.to_string()).join("bw")).unwrap(), "new");
    }
}
