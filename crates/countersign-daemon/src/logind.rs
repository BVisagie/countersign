//! Locks the vault when logind reports a screen lock or an imminent sleep.

use std::sync::Arc;

use anyhow::Result;
use futures_util::StreamExt;
use zbus::zvariant::{OwnedFd, OwnedObjectPath};
use zbus::{Connection, proxy};

use crate::core::{Daemon, LockReason};

#[proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Manager {
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::Result<OwnedFd>;
    fn get_user(&self, uid: u32) -> zbus::Result<OwnedObjectPath>;
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

#[proxy(interface = "org.freedesktop.login1.User", default_service = "org.freedesktop.login1")]
trait User {
    #[zbus(property)]
    fn display(&self) -> zbus::Result<(String, OwnedObjectPath)>;
}

#[proxy(interface = "org.freedesktop.login1.Session", default_service = "org.freedesktop.login1")]
trait Session {
    #[zbus(signal)]
    fn lock(&self) -> zbus::Result<()>;
}

/// Holds a delay inhibitor while the vault is unlocked, and locks the
/// vault before acknowledging sleep by releasing it.
pub async fn watch_sleep(daemon: Arc<Daemon>) -> Result<()> {
    let conn = Connection::system().await?;
    let manager = ManagerProxy::new(&conn).await?;
    let mut sleeps = manager.receive_prepare_for_sleep().await?;
    let mut status = daemon.subscribe_status();
    let mut settings = daemon.subscribe_settings();
    let mut inhibitor: Option<OwnedFd> = None;
    loop {
        let wanted = daemon.is_unlocked() && settings.borrow().lock_on_suspend;
        if wanted && inhibitor.is_none() {
            match manager
                .inhibit("sleep", "Countersign", "Lock the vault before the computer sleeps", "delay")
                .await
            {
                Ok(fd) => inhibitor = Some(fd),
                Err(e) => tracing::warn!("cannot delay sleep to lock the vault: {e}"),
            }
        } else if !wanted {
            inhibitor = None;
        }
        tokio::select! {
            signal = sleeps.next() => {
                let Some(signal) = signal else { break };
                if signal.args().is_ok_and(|a| a.start) {
                    if settings.borrow().lock_on_suspend {
                        daemon.lock(LockReason::Sleep).await;
                    }
                    // Releasing the inhibitor lets the computer sleep.
                    inhibitor = None;
                }
            }
            changed = status.changed() => if changed.is_err() { break },
            changed = settings.changed() => if changed.is_err() { break },
        }
    }
    Ok(())
}

/// Locks the vault when logind asks the user's display session to lock,
/// as `loginctl lock-session` and idle daemons do.
pub async fn watch_session_lock(daemon: Arc<Daemon>) -> Result<()> {
    let conn = Connection::system().await?;
    let manager = ManagerProxy::new(&conn).await?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let user = UserProxy::builder(&conn).path(manager.get_user(uid).await?)?.build().await?;
    let (_, session_path) = user.display().await?;
    let session = SessionProxy::builder(&conn).path(session_path)?.build().await?;
    let mut locks = session.receive_lock().await?;
    while locks.next().await.is_some() {
        if daemon.settings().lock_on_screen_lock {
            daemon.lock(LockReason::ScreenLocked).await;
        }
    }
    Ok(())
}
