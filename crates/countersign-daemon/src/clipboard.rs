//! Puts values on the Wayland clipboard from inside the daemon.
//!
//! Each copy is served by its own thread until another application replaces
//! it or Countersign clears it. Values are offered with the
//! `x-kde-passwordManagerHint` type, which tells clipboard history tools,
//! including Omarchy's, not to keep them.
//!
//! Every offer also carries a marker type holding its generation. The
//! compositor publishes an offer only once its thread flushes it, so a
//! copy is registered as ours only after the marker reads back, and a clear
//! happens only while the marker shows our generation. Both run under one
//! lock, which leaves no moment where a published secret has no owner.

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use wl_clipboard_rs::copy::{self, ClipboardType, MimeSource, MimeType, Options, Seat, Source};
use wl_clipboard_rs::paste;
use zeroize::Zeroizing;

const MARKER: &str = "application/x-countersign-copy";
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(2);

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

/// The generation in the marker of the value the compositor currently holds.
fn published() -> Option<u64> {
    let (pipe, _) = paste::get_contents(
        paste::ClipboardType::Regular,
        paste::Seat::Unspecified,
        paste::MimeType::Specific(MARKER),
    )
    .ok()?;
    let mut text = String::new();
    pipe.take(32).read_to_string(&mut text).ok()?;
    text.trim().parse().ok()
}

impl Clipboard {
    /// Offers `value` on the clipboard and returns its generation. Blocking;
    /// call it from a blocking task.
    ///
    /// `allow` runs under the clipboard's lock, after the compositor shows
    /// the offer and before it is registered, with the generation currently
    /// registered as ours. Returning false withdraws the value. A backstop,
    /// armed before anything is published, clears it `expires` (plus a grace
    /// period) later even if the caller's own timer never runs.
    pub fn copy(
        self: &Arc<Self>,
        value: Zeroizing<String>,
        sensitive: bool,
        expires: Duration,
        allow: impl FnOnce(Option<u64>) -> bool,
    ) -> Result<u64> {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let me = self.clone();
        std::thread::Builder::new().name("countersign-clipboard-expiry".into()).spawn(move || {
            std::thread::sleep(expires + Duration::from_secs(2));
            me.clear_if_ours(Some(generation));
        })?;

        let (ready_tx, ready_rx) = sync_channel(1);
        let alive = Arc::new(AtomicBool::new(true));
        let serving = alive.clone();
        std::thread::Builder::new().name("countersign-clipboard".into()).spawn(move || {
            let mut options = Options::new();
            options
                .foreground(true)
                .clipboard(ClipboardType::Regular)
                .seat(Seat::All)
                .trim_newline(false)
                .sensitive(sensitive);
            let sources = vec![
                MimeSource { source: Source::Bytes(value.as_bytes().into()), mime_type: MimeType::Text },
                MimeSource {
                    source: Source::Bytes(generation.to_string().into_bytes().into()),
                    mime_type: MimeType::Specific(MARKER.into()),
                },
            ];
            drop(value);
            match options.prepare_copy_multi(sources) {
                Ok(prepared) => {
                    let _ = ready_tx.send(Ok(()));
                    if let Err(e) = prepared.serve() {
                        tracing::debug!("clipboard serving ended: {e}");
                    }
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(anyhow!("the clipboard is unavailable: {e}")));
                }
            }
            serving.store(false, Ordering::SeqCst);
        })?;
        ready_rx.recv().map_err(|_| anyhow!("the clipboard thread stopped"))??;

        let start = Instant::now();
        while published() != Some(generation) {
            if start.elapsed() > PUBLISH_TIMEOUT || !alive.load(Ordering::SeqCst) {
                return Err(anyhow!("the clipboard did not take the copy"));
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        let mut current = self.current.lock().expect("clipboard state");
        let live = current.as_ref().filter(|o| o.alive.load(Ordering::SeqCst)).map(|o| o.generation);
        if !allow(live) {
            if published() == Some(generation) {
                let _ = copy::clear(ClipboardType::Regular, Seat::All);
            }
            return Err(Refused.into());
        }
        *current = Some(Owned { generation, alive });
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

    /// Clears the clipboard if the compositor holds Countersign's value from
    /// `generation`, or our registered value when `generation` is `None`.
    /// Never clears another application's value. Blocking.
    pub fn clear_if_ours(&self, generation: Option<u64>) -> bool {
        let mut current = self.current.lock().expect("clipboard state");
        let Some(target) = generation.or_else(|| current.as_ref().map(|o| o.generation)) else {
            return false;
        };
        let registered = current.as_ref().is_some_and(|o| o.generation == target);
        if published() != Some(target) {
            if registered {
                *current = None;
            }
            return false;
        }
        if let Err(e) = copy::clear(ClipboardType::Regular, Seat::All) {
            tracing::warn!("clearing the clipboard failed: {e}");
            return false;
        }
        if registered {
            *current = None;
        }
        true
    }
}
