//! Puts values on the Wayland clipboard from inside the daemon.
//!
//! Each copy is served by its own thread until another application replaces
//! it or Countersign clears it. Secrets are offered with the
//! `x-kde-passwordManagerHint` type, which tells clipboard history tools,
//! including Omarchy's, not to keep them.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use wl_clipboard_rs::copy::{self, ClipboardType, MimeType, Options, Seat, Source};
use zeroize::Zeroizing;

#[derive(Default)]
pub struct Clipboard {
    generation: AtomicU64,
    current: Mutex<Option<Owned>>,
}

struct Owned {
    generation: u64,
    alive: Arc<AtomicBool>,
}

/// Returned when a copy's guard refuses it, for example after a lock.
#[derive(Debug, thiserror::Error)]
#[error("the copy was cancelled")]
pub struct Refused;

impl Clipboard {
    /// Offers `value` on the clipboard and returns its generation. Blocks
    /// until the compositor has accepted the offer, so call it from a
    /// blocking task.
    ///
    /// `allow` runs under the clipboard's lock with the generation that is
    /// currently ours, just before this copy takes over. Returning false
    /// withdraws the offer. Because it runs here rather than in the caller,
    /// cancelling the caller cannot leave an unguarded value behind. A
    /// backstop clears the value `expires` (plus a grace period) later even
    /// if the caller's own timer never runs.
    pub fn copy(
        self: &Arc<Self>,
        value: Zeroizing<String>,
        sensitive: bool,
        expires: Duration,
        allow: impl FnOnce(Option<u64>) -> bool + Send + 'static,
    ) -> Result<u64> {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (ready_tx, ready_rx) = sync_channel(1);
        let me = self.clone();
        std::thread::Builder::new().name("countersign-clipboard".into()).spawn(move || {
            let mut options = Options::new();
            options
                .foreground(true)
                .clipboard(ClipboardType::Regular)
                .seat(Seat::All)
                .trim_newline(false)
                .sensitive(sensitive);
            let bytes: Box<[u8]> = value.as_bytes().into();
            drop(value);
            let prepared = match options.prepare_copy(Source::Bytes(bytes), MimeType::Text) {
                Ok(prepared) => prepared,
                Err(e) => {
                    let _ = ready_tx.send(Err(anyhow!("the clipboard is unavailable: {e}")));
                    return;
                }
            };
            let alive = Arc::new(AtomicBool::new(true));
            let allowed = {
                let mut current = me.current.lock().expect("clipboard state");
                let live = current.as_ref().filter(|o| o.alive.load(Ordering::SeqCst)).map(|o| o.generation);
                let allowed = allow(live);
                if allowed {
                    *current = Some(Owned { generation, alive: alive.clone() });
                }
                allowed
            };
            if !allowed {
                // Dropping the prepared copy withdraws the offer.
                drop(prepared);
                let _ = ready_tx.send(Err(Refused.into()));
                return;
            }
            let _ = ready_tx.send(Ok(()));
            if let Err(e) = prepared.serve() {
                tracing::debug!("clipboard serving ended: {e}");
            }
            alive.store(false, Ordering::SeqCst);
        })?;
        ready_rx.recv().map_err(|_| anyhow!("the clipboard thread stopped"))??;
        let me = self.clone();
        std::thread::Builder::new().name("countersign-clipboard-expiry".into()).spawn(move || {
            std::thread::sleep(expires + Duration::from_secs(2));
            me.clear_if_ours(Some(generation));
        })?;
        Ok(generation)
    }

    /// Whether Countersign's value from `generation` is still on the clipboard.
    pub fn is_current(&self, generation: u64) -> bool {
        self.current
            .lock()
            .expect("clipboard state")
            .as_ref()
            .is_some_and(|o| o.generation == generation && o.alive.load(Ordering::SeqCst))
    }

    /// Clears the clipboard if it still holds Countersign's value from
    /// `generation`, or any value of ours when `generation` is `None`.
    /// Never clears another application's value. Blocking. Holds the lock
    /// throughout, so no copy can take over between the check and the clear.
    pub fn clear_if_ours(&self, generation: Option<u64>) -> bool {
        let mut current = self.current.lock().expect("clipboard state");
        let ours = current
            .as_ref()
            .is_some_and(|o| generation.is_none_or(|g| g == o.generation) && o.alive.load(Ordering::SeqCst));
        if !ours {
            return false;
        }
        if let Err(e) = copy::clear(ClipboardType::Regular, Seat::All) {
            tracing::warn!("clearing the clipboard failed: {e}");
            return false;
        }
        *current = None;
        true
    }
}
