//! Background work: inactivity and screen-lock locking, periodic sync and
//! the logind watchers.

use std::sync::Arc;
use std::time::Duration;

use countersign_ipc::VaultState;

use crate::core::{Daemon, LockReason};
use crate::{hyprland, logind};

pub fn spawn(daemon: Arc<Daemon>) {
    tokio::spawn(watch_idle_and_screen(daemon.clone()));
    tokio::spawn(sync_periodically(daemon.clone()));
    let d = daemon.clone();
    tokio::spawn(async move {
        if let Err(e) = logind::watch_sleep(d).await {
            tracing::warn!("cannot watch for sleep: {e}");
        }
    });
    tokio::spawn(async move {
        if let Err(e) = logind::watch_session_lock(daemon).await {
            tracing::warn!("cannot watch logind session locks: {e}");
        }
    });
}

/// Polls Hyprland's lock state and the inactivity timer, only while the
/// vault is unlocked. Each poll is one request on Hyprland's socket.
async fn watch_idle_and_screen(daemon: Arc<Daemon>) {
    let mut status = daemon.subscribe_status();
    loop {
        if status.wait_for(|s| s.vault == VaultState::Unlocked).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        if !daemon.is_unlocked() {
            continue;
        }
        let settings = daemon.settings();
        let idle_limit = Duration::from_secs(u64::from(settings.lock_after_idle_mins) * 60);
        if settings.lock_after_idle_mins > 0 && daemon.idle_for() >= idle_limit {
            daemon.lock(LockReason::Idle).await;
        } else if settings.lock_on_screen_lock && hyprland::session_locked().await == Some(true) {
            daemon.lock(LockReason::ScreenLocked).await;
        }
    }
}

async fn sync_periodically(daemon: Arc<Daemon>) {
    let mut status = daemon.subscribe_status();
    let mut settings = daemon.subscribe_settings();
    loop {
        if status.wait_for(|s| s.vault == VaultState::Unlocked).await.is_err() {
            return;
        }
        let minutes = settings.borrow_and_update().sync_interval_mins;
        let wait = async {
            if minutes == 0 {
                std::future::pending::<()>().await;
            }
            tokio::time::sleep(Duration::from_secs(u64::from(minutes) * 60)).await;
        };
        tokio::select! {
            () = wait => {
                if daemon.is_unlocked()
                    && let Err(e) = daemon.sync().await
                {
                    tracing::info!("periodic sync failed: {e}");
                }
            }
            changed = settings.changed() => if changed.is_err() { return },
            closed = async { status.wait_for(|s| s.vault != VaultState::Unlocked).await.is_err() } => if closed { return },
        }
    }
}
