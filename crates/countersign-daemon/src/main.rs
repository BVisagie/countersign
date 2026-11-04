//! `countersignd`: Countersign's background service.
//!
//! It owns the official Bitwarden CLI and the unlocked session, puts values
//! on the clipboard, and locks the vault on inactivity, screen lock and
//! sleep. The app starts it on demand; it exits once nothing needs it.

mod clipboard;
#[cfg(test)]
mod contract;
mod copy;
mod core;
mod handlers;
mod hyprland;
mod install;
mod logind;
mod paths;
mod server;
mod settings;
mod triggers;
mod unlisted;

use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::net::UnixListener;
use tokio::signal::unix::{SignalKind, signal};

use crate::core::{Daemon, LockReason};
use crate::paths::{Paths, ensure_private_dir};

/// How long the daemon stays after the last app disconnects.
const IDLE_EXIT: Duration = Duration::from_secs(60);

fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("countersignd {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("COUNTERSIGN_LOG")
                .unwrap_or_else(|_| "warn,countersignd=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    if let Err(e) = harden() {
        tracing::error!("refusing to start: {e:#}");
        return ExitCode::FAILURE;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!("cannot start: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Keeps secrets out of core dumps and away from same-user debuggers that
/// lack CAP_SYS_PTRACE.
fn harden() -> Result<()> {
    // SAFETY: prctl and setrlimit with valid arguments have no memory effects.
    unsafe {
        if libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) != 0 {
            bail!("cannot disable core dumps: {}", std::io::Error::last_os_error());
        }
        let none = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        if libc::setrlimit(libc::RLIMIT_CORE, &none) != 0 {
            bail!("cannot limit core dumps: {}", std::io::Error::last_os_error());
        }
        if libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0) != 0 {
            bail!("core dumps are still enabled");
        }
    }
    Ok(())
}

async fn run() -> Result<()> {
    let paths = Paths::discover()?;
    ensure_private_dir(&paths.runtime_dir)?;

    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&paths.lock_file)
        .context("opening the daemon lock")?;
    // SAFETY: flock on a valid descriptor that outlives the call.
    if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&lock), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        tracing::info!("another countersignd is already running");
        return Ok(());
    }

    let _ = std::fs::remove_file(&paths.socket);
    let listener = UnixListener::bind(&paths.socket).context("binding the daemon socket")?;
    std::fs::set_permissions(&paths.socket, std::fs::Permissions::from_mode(0o600))?;

    let settings = settings::load(&paths.settings_file);
    let socket = paths.socket.clone();
    let daemon = Daemon::new(paths, settings);
    {
        let daemon = daemon.clone();
        tokio::spawn(async move {
            let _op = daemon.ops.lock().await;
            daemon.refresh().await;
        });
    }
    triggers::spawn(daemon.clone());

    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    tokio::select! {
        () = server::serve(daemon.clone(), listener) => {}
        () = exit_when_idle(&daemon) => tracing::info!("nothing needs countersignd; exiting"),
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
    daemon.lock(LockReason::Shutdown).await;
    let _ = std::fs::remove_file(socket);
    drop(lock);
    Ok(())
}

async fn exit_when_idle(daemon: &Daemon) {
    loop {
        tokio::time::sleep(Duration::from_secs(15)).await;
        if daemon.can_exit(IDLE_EXIT) {
            return;
        }
    }
}
