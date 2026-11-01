//! Copying fields to the clipboard, clearing them, and replacing a copied
//! password with the item's one-time code.
//!
//! Every copy is marked sensitive, so clipboard history skips it. Each copy
//! records the lock epoch it started in; the clipboard refuses the value if
//! a lock happened in between, even when the task that asked has since been
//! cancelled.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use countersign_ipc::{ClipboardEvent, ClipboardState, ErrorCode, Event, Field, IpcError, Response};
use secrecy::ExposeSecret;
use zeroize::Zeroizing;

use crate::clipboard::{Clear, Refused};
use crate::core::{Daemon, ipc_error};

impl Daemon {
    pub async fn copy_field(self: &Arc<Self>, id: &str, field: Field) -> Result<Response, IpcError> {
        self.touch();
        let epoch = self.epoch();
        let client = self.client().await?;
        let item = client.get_item(id).await.map_err(ipc_error)?;
        self.check_reprompt(id, &item)?;
        let name = item["name"].as_str().unwrap_or_default().to_owned();
        let has_totp = Field::Totp.read(&item).is_some();
        let value = match &field {
            Field::Totp => {
                Zeroizing::new(client.get_totp(id).await.map_err(ipc_error)?.expose_secret().to_owned())
            }
            other => Zeroizing::new(other.read(&item).ok_or_else(|| {
                IpcError::new(
                    ErrorCode::NotFound,
                    format!("This item has no {}.", other.label().to_lowercase()),
                )
            })?),
        };
        drop(item);
        let settings = self.settings();
        self.copy_timers.lock().expect("timers").cancel();
        let generation = self.put(value, epoch, None, settings.clipboard_clear_secs).await?;
        self.copy_timers.lock().expect("timers").item_name = Some(name.clone());
        self.note_recent(id);
        self.emit(Event::Recent { ids: self.vault().recent });
        let follow_up = field == Field::Password && has_totp && settings.totp_follow_up;
        self.schedule_clear(generation, settings.clipboard_clear_secs);
        if follow_up {
            self.schedule_follow_up(
                generation,
                epoch,
                id.to_owned(),
                name.clone(),
                settings.totp_follow_up_secs,
            );
        }
        self.announce(ClipboardEvent {
            state: ClipboardState::Copied,
            label: format!("{} copied", field.label()),
            item_name: Some(name),
            clears_in: Some(settings.clipboard_clear_secs),
            follow_up_in: follow_up.then_some(settings.totp_follow_up_secs),
        });
        Ok(Response::Ok)
    }

    /// Copies a value the user already sees, such as a generated password.
    pub async fn copy_value(
        self: &Arc<Self>,
        value: Zeroizing<String>,
        label: String,
    ) -> Result<Response, IpcError> {
        self.touch();
        let epoch = self.epoch();
        let settings = self.settings();
        self.copy_timers.lock().expect("timers").cancel();
        let generation = self.put(value, epoch, None, settings.clipboard_clear_secs).await?;
        self.schedule_clear(generation, settings.clipboard_clear_secs);
        self.announce(ClipboardEvent {
            state: ClipboardState::Copied,
            label: format!("{label} copied"),
            item_name: None,
            clears_in: Some(settings.clipboard_clear_secs),
            follow_up_in: None,
        });
        Ok(Response::Ok)
    }

    /// Puts a value on the clipboard unless a lock happened since `epoch`.
    /// With `replacing`, only while that generation is still ours.
    async fn put(
        &self,
        value: Zeroizing<String>,
        epoch: u64,
        replacing: Option<u64>,
        expires_secs: u32,
    ) -> Result<u64, IpcError> {
        let clipboard = self.clipboard.clone();
        let counter = self.epoch_counter();
        let allow = move || counter.load(Ordering::SeqCst) == epoch;
        let expires = Duration::from_secs(u64::from(expires_secs));
        let result =
            tokio::task::spawn_blocking(move || clipboard.copy(value, true, expires, allow, replacing))
                .await
                .map_err(|e| IpcError::new(ErrorCode::Internal, e.to_string()))?;
        result.map_err(|e| {
            if e.is::<Refused>() {
                IpcError::new(ErrorCode::Locked, "The vault was locked before the copy finished.")
            } else {
                IpcError::new(ErrorCode::Internal, e.to_string())
            }
        })
    }

    fn schedule_clear(self: &Arc<Self>, generation: u64, after_secs: u32) {
        let me = self.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(u64::from(after_secs))).await;
            let clipboard = me.clipboard.clone();
            let outcome = tokio::task::spawn_blocking(move || clipboard.clear_if_ours(Some(generation)))
                .await
                .unwrap_or(Clear::Failed);
            // A copy started since then speaks for itself; an aborted timer
            // that was already running must not end its notification. The
            // lock orders this check and report against copies announcing
            // themselves.
            let _order = me.clipboard_events.lock().expect("clipboard events");
            if me.clipboard.newest() != generation {
                return;
            }
            let (state, label) = match outcome {
                // Cleared, or another app replaced it: nothing of ours is left.
                Clear::Cleared | Clear::NotOurs => (ClipboardState::Cleared, "Clipboard cleared"),
                Clear::Failed => (ClipboardState::Failed, "Countersign could not clear the clipboard"),
            };
            me.emit_clipboard(ClipboardEvent {
                state,
                label: label.into(),
                item_name: None,
                clears_in: None,
                follow_up_in: None,
            });
        });
        let mut timers = self.copy_timers.lock().expect("timers");
        timers.clear_at = Some(std::time::Instant::now() + Duration::from_secs(u64::from(after_secs)));
        if let Some(old) = timers.clear.replace(task) {
            old.abort();
        }
    }

    fn schedule_follow_up(
        self: &Arc<Self>,
        generation: u64,
        epoch: u64,
        id: String,
        name: String,
        after_secs: u32,
    ) {
        let me = self.clone();
        let state = Arc::new(FollowUpState::default());
        let decided = state.clone();
        let task = tokio::spawn(async move {
            // However the task ends, a later skip must not claim to keep a
            // password the follow-up stopped tracking.
            let _done = Finish(decided.clone());
            tokio::time::sleep(Duration::from_secs(u64::from(after_secs))).await;
            // Leave the clipboard alone if the password was replaced.
            if !me.clipboard.is_current(generation) || me.epoch() != epoch {
                return;
            }
            let code = async { me.client().await?.get_totp(&id).await.map_err(ipc_error) }.await;
            let code = match code {
                Ok(code) => Zeroizing::new(code.expose_secret().to_owned()),
                Err(e) => {
                    // Report the failure only if a skip has not already
                    // announced the kept password.
                    if !decided.try_end() {
                        return;
                    }
                    me.emit_clipboard(ClipboardEvent {
                        state: ClipboardState::Failed,
                        label: format!("Verification code unavailable: {}", e.message),
                        item_name: Some(name),
                        clears_in: None,
                        follow_up_in: None,
                    });
                    return;
                }
            };
            let clear_secs = me.settings().clipboard_clear_secs;
            // The clipboard replaces the password only if it is still ours
            // and the vault is still open when the code takes over.
            // Claim the replacement before the code is published: publishing
            // it ends the password's offer, so a skip must win before that
            // point to keep the password, or stay silent after it.
            if !decided.try_replace() {
                return;
            }
            match me.put(code, epoch, Some(generation), clear_secs).await {
                Ok(next) => {
                    me.schedule_clear(next, clear_secs);
                    me.announce(ClipboardEvent {
                        state: ClipboardState::FollowUpCopied,
                        label: "Verification code copied".into(),
                        item_name: Some(name),
                        clears_in: Some(clear_secs),
                        follow_up_in: None,
                    });
                }
                Err(e) if e.code == ErrorCode::Locked => {}
                Err(e) => tracing::warn!("copying the verification code failed: {e}"),
            }
        });
        let mut timers = self.copy_timers.lock().expect("timers");
        timers.follow_up_state = Some(state);
        if let Some(old) = timers.follow_up.replace(task) {
            old.abort();
        }
    }

    /// Cancels a pending one-time-code replacement, keeping the password.
    /// Re-announces the copy without the follow-up, so every receipt and
    /// notification stops counting down to it. Only when the skip wins
    /// against a replacement already under way: then the code is withdrawn.
    pub fn skip_follow_up(&self) {
        let (remaining, item_name) = {
            let mut timers = self.copy_timers.lock().expect("timers");
            let won = timers.follow_up_state.as_ref().is_some_and(|s| s.try_cancel());
            if !won {
                // The code already replaced the password, or the follow-up
                // ended; its own announcement stands.
                return;
            }
            if let Some(task) = timers.follow_up.take() {
                task.abort();
            }
            let remaining = timers.clear_at.map(|at| at.saturating_duration_since(std::time::Instant::now()));
            (remaining, timers.item_name.clone())
        };
        self.announce(ClipboardEvent {
            state: ClipboardState::Copied,
            label: "Password kept".into(),
            item_name,
            clears_in: remaining.map(|d| d.as_secs_f64().ceil() as u32),
            follow_up_in: None,
        });
    }

    /// Announces a copy, ordered against expiry reports.
    fn announce(&self, clipboard: ClipboardEvent) {
        let _order = self.clipboard_events.lock().expect("clipboard events");
        self.emit(Event::Clipboard { clipboard });
    }

    fn emit_clipboard(&self, clipboard: ClipboardEvent) {
        self.emit(Event::Clipboard { clipboard });
    }
}

/// Settles, exactly once, whether a code follow-up replaces the password,
/// is cancelled by "Keep password", or ends without either.
#[derive(Debug, Default)]
pub(crate) struct FollowUpState(AtomicU8);

impl FollowUpState {
    const PENDING: u8 = 0;
    const REPLACED: u8 = 1;
    const CANCELLED: u8 = 2;
    const ENDED: u8 = 3;

    fn settle(&self, to: u8) -> bool {
        self.0.compare_exchange(Self::PENDING, to, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }

    /// The follow-up takes the clipboard; false if a skip came first.
    pub fn try_replace(&self) -> bool {
        self.settle(Self::REPLACED)
    }

    /// The follow-up gives up, for example when the code is unavailable;
    /// false if a skip already settled it.
    pub fn try_end(&self) -> bool {
        self.settle(Self::ENDED)
    }

    /// "Keep password"; false if the code already took over or the
    /// follow-up ended.
    pub fn try_cancel(&self) -> bool {
        self.settle(Self::CANCELLED)
    }
}

/// Marks a follow-up ended when its task stops, unless it was settled.
struct Finish(Arc<FollowUpState>);

impl Drop for Finish {
    fn drop(&mut self) {
        self.0.settle(FollowUpState::ENDED);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_up_is_settled_once() {
        // A skip during the replacement loses once the code took over...
        let state = FollowUpState::default();
        assert!(state.try_replace());
        assert!(!state.try_cancel());
        // ...and wins before it, refusing the replacement.
        let state = FollowUpState::default();
        assert!(state.try_cancel());
        assert!(!state.try_replace());
        // A failure after a skip stays silent; a skip after a failure is refused.
        let state = FollowUpState::default();
        assert!(state.try_cancel());
        assert!(!state.try_end());
        let state = FollowUpState::default();
        assert!(state.try_end());
        assert!(!state.try_cancel());
        // A follow-up that ended cannot be "kept".
        let state = Arc::new(FollowUpState::default());
        drop(Finish(state.clone()));
        assert!(!state.try_cancel());
        assert!(!state.try_replace());
    }
}
