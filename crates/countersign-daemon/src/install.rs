//! Installs the pinned official CLI release for Countersign's own use.
//!
//! Runs only when you ask. The archive comes from Bitwarden's GitHub release
//! over HTTPS and must match the SHA-256 recorded with the pin before
//! anything is extracted. The extracted `bw` must report exactly the pinned
//! version in an empty profile before it replaces anything. It never touches
//! a system package or a CLI you configured yourself.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use countersign_bw::{Cli, PINNED_VERSION, PinnedArchive};
use countersign_ipc::{CliState, ErrorCode, IpcError};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};

use crate::core::Daemon;
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
    if let CliState::Ready { version, managed: true } = &daemon.status().cli
        && *version == PINNED_VERSION.to_string()
    {
        return Ok(());
    }
    daemon.update_status(|s| s.cli = CliState::Installing);
    let result = run(&daemon.paths.managed_cli_root).await;
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
    result
}

async fn run(root: &Path) -> Result<(), IpcError> {
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

    let target_dir = root.join(PINNED_VERSION.to_string());
    let next = staging.path().join("release");
    fs::DirBuilder::new().mode(0o700).create(&next).map_err(|e| failed(e.to_string()))?;
    fs::rename(&binary, next.join("bw")).map_err(|e| failed(e.to_string()))?;
    let old = staging.path().join("previous");
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
