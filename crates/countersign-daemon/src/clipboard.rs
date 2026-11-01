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

/// What clearing found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clear {
    /// Our value was on the clipboard and is gone now.
    Cleared,
    /// The clipboard held something else: another app replaced our value.
    NotOurs,
    /// Our value is still there; clearing it failed.
    Failed,
}

/// Returned when a copy's guard refuses it, for example after a lock.
#[derive(Debug, thiserror::Error)]
#[error("the copy was cancelled")]
pub struct Refused;

/// The clipboard could not be read, so whose value it holds is unknown.
#[derive(Debug)]
struct Unreadable;

/// The generation in the marker of the value the compositor holds:
/// `Ok(None)` when it holds nothing, or something without our marker, and
/// `Err` when it cannot be read, which must not be mistaken for either.
fn published() -> Result<Option<u64>, Unreadable> {
    let pipe = match paste::get_contents(
        paste::ClipboardType::Regular,
        paste::Seat::Unspecified,
        paste::MimeType::Specific(MARKER),
    ) {
        Ok((pipe, _)) => pipe,
        Err(paste::Error::ClipboardEmpty | paste::Error::NoMimeType) => return Ok(None),
        Err(e) => {
            tracing::debug!("reading the clipboard failed: {e}");
            return Err(Unreadable);
        }
    };
    let mut text = String::new();
    pipe.take(32).read_to_string(&mut text).map_err(|_| Unreadable)?;
    Ok(text.trim().parse().ok())
}

/// Whether a refused copy must clear the clipboard, from one reading of it:
/// yes if it shows our value, and yes if it cannot be read, since our
/// value may be there.
fn should_withdraw(read: Result<Option<u64>, Unreadable>, generation: u64) -> bool {
    match read {
        Ok(Some(g)) => g == generation,
        Ok(None) => false,
        Err(Unreadable) => true,
    }
}

impl Clipboard {
    /// Offers `value` on the clipboard and returns its generation. Blocking;
    /// call it from a blocking task.
    ///
    /// The clipboard's lock is held for the whole copy, so copies and clears
    /// never interleave. With `replacing`, the copy proceeds only while that
    /// generation is still ours; publishing it then cancels the old offer.
    /// `allow` runs after the compositor shows the new offer and before it is
    /// registered; returning false withdraws it. A backstop, armed before
    /// anything is published, clears the value `expires` (plus a grace
    /// period) later even if the caller's own timer never runs.
    pub fn copy(
        self: &Arc<Self>,
        value: Zeroizing<String>,
        sensitive: bool,
        expires: Duration,
        allow: impl FnOnce() -> bool,
        replacing: Option<u64>,
    ) -> Result<u64> {
        let mut current = self.current.lock().expect("clipboard state");
        if let Some(previous) = replacing
            && !current.as_ref().is_some_and(|o| o.generation == previous && o.alive.load(Ordering::SeqCst))
        {
            return Err(Refused.into());
        }
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
        while !matches!(published(), Ok(Some(g)) if g == generation) {
            if start.elapsed() > PUBLISH_TIMEOUT || !alive.load(Ordering::SeqCst) {
                return Err(anyhow!("the clipboard did not take the copy"));
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        if !allow() {
            // Withdraw our value; if the clipboard cannot be read, clear it
            // rather than risk leaving the value behind.
            if should_withdraw(published(), generation) {
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
    pub fn clear_if_ours(&self, generation: Option<u64>) -> Clear {
        let mut current = self.current.lock().expect("clipboard state");
        let Some(target) = generation.or_else(|| current.as_ref().map(|o| o.generation)) else {
            return Clear::NotOurs;
        };
        let registered = current.as_ref().is_some_and(|o| o.generation == target);
        match published() {
            Ok(Some(g)) if g == target => {}
            Ok(_) => {
                if registered {
                    *current = None;
                }
                return Clear::NotOurs;
            }
            // Unknown: keep ownership so the backstop tries again.
            Err(Unreadable) => return Clear::Failed,
        }
        if let Err(e) = copy::clear(ClipboardType::Regular, Seat::All) {
            tracing::warn!("clearing the clipboard failed: {e}");
            return Clear::Failed;
        }
        if registered {
            *current = None;
        }
        Clear::Cleared
    }

    /// The generation of the newest copy started, whether or not it finished.
    pub fn newest(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refused_copies_clear_when_unsure() {
        assert!(should_withdraw(Ok(Some(7)), 7));
        assert!(!should_withdraw(Ok(Some(8)), 7));
        assert!(!should_withdraw(Ok(None), 7));
        assert!(should_withdraw(Err(Unreadable), 7));
    }

    fn value(text: &str) -> Zeroizing<String> {
        Zeroizing::new(text.to_owned())
    }

    /// Uses the real clipboard: run in a Wayland session with
    /// `cargo test -p countersign-daemon -- --ignored clipboard`.
    #[test]
    #[ignore = "needs a Wayland session; replaces the clipboard"]
    fn clipboard_replacement_and_guards() {
        let clipboard = Arc::new(Clipboard::default());
        let ttl = Duration::from_secs(10);
        let first = clipboard.copy(value("first"), true, ttl, || true, None).unwrap();
        assert_eq!(published().unwrap(), Some(first));

        // A replacement while the first value is ours succeeds, even though
        // publishing it cancels the first offer.
        let second = clipboard.copy(value("second"), true, ttl, || true, Some(first)).expect("replacement");
        assert_eq!(published().unwrap(), Some(second));
        assert!(clipboard.is_current(second));

        // Replacing a value that is no longer ours, or a refused guard,
        // leaves the current value alone or withdraws the new one.
        assert!(clipboard.copy(value("third"), true, ttl, || true, Some(first)).is_err());
        assert_eq!(published().unwrap(), Some(second));
        assert!(clipboard.copy(value("fourth"), true, ttl, || false, None).is_err());
        assert_eq!(published().unwrap(), None);

        let last = clipboard.copy(value("fifth"), true, ttl, || true, None).unwrap();
        assert_eq!(clipboard.clear_if_ours(None), Clear::Cleared);
        assert_eq!(published().unwrap(), None);
        assert!(!clipboard.is_current(last));
    }
}
