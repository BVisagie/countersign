//! Copying fields to the clipboard, clearing them, and replacing a copied
//! password with the item's one-time code.

use std::sync::Arc;
use std::time::Duration;

use countersign_ipc::{ClipboardEvent, ClipboardState, ErrorCode, Event, Field, IpcError, Response};
use secrecy::ExposeSecret;
use zeroize::Zeroizing;

use crate::core::{Daemon, ipc_error};

impl Daemon {
    pub async fn copy_field(self: &Arc<Self>, id: &str, field: Field) -> Result<Response, IpcError> {
        self.touch();
        let client = self.client().await?;
        let item = client.get_item(id).await.map_err(ipc_error)?;
        self.check_reprompt(id, &item)?;
        let name = item["name"].as_str().unwrap_or_default().to_owned();
        let has_totp = Field::Totp.read(&item).is_some();
        let (value, sensitive) = match &field {
            Field::Totp => {
                let code = client.get_totp(id).await.map_err(ipc_error)?;
                (Zeroizing::new(code.expose_secret().to_owned()), true)
            }
            other => {
                let value = other.read(&item).ok_or_else(|| {
                    IpcError::new(
                        ErrorCode::NotFound,
                        format!("This item has no {}.", other.label().to_lowercase()),
                    )
                })?;
                (Zeroizing::new(value), other.is_secret_kind(&item))
            }
        };
        drop(item);
        let settings = self.settings();
        self.copy_timers.lock().expect("timers").cancel();
        let generation = self.put(value, sensitive).await?;
        self.note_recent(id);
        let follow_up = field == Field::Password && has_totp && settings.totp_follow_up;
        self.schedule_clear(generation, settings.clipboard_clear_secs);
        if follow_up {
            self.schedule_follow_up(generation, id.to_owned(), name.clone(), settings.totp_follow_up_secs);
        }
        self.emit_clipboard(ClipboardEvent {
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
        secret: bool,
    ) -> Result<Response, IpcError> {
        self.touch();
        let settings = self.settings();
        self.copy_timers.lock().expect("timers").cancel();
        let generation = self.put(value, secret).await?;
        self.schedule_clear(generation, settings.clipboard_clear_secs);
        self.emit_clipboard(ClipboardEvent {
            state: ClipboardState::Copied,
            label: format!("{label} copied"),
            item_name: None,
            clears_in: Some(settings.clipboard_clear_secs),
            follow_up_in: None,
        });
        Ok(Response::Ok)
    }

    async fn put(&self, value: Zeroizing<String>, sensitive: bool) -> Result<u64, IpcError> {
        let clipboard = self.clipboard.clone();
        tokio::task::spawn_blocking(move || clipboard.copy(value, sensitive))
            .await
            .map_err(|e| IpcError::new(ErrorCode::Internal, e.to_string()))?
            .map_err(|e| IpcError::new(ErrorCode::Internal, e.to_string()))
    }

    fn schedule_clear(self: &Arc<Self>, generation: u64, after_secs: u32) {
        let me = self.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(u64::from(after_secs))).await;
            let clipboard = me.clipboard.clone();
            let cleared = tokio::task::spawn_blocking(move || clipboard.clear_if_ours(Some(generation)))
                .await
                .unwrap_or(false);
            if cleared {
                me.emit_clipboard(ClipboardEvent {
                    state: ClipboardState::Cleared,
                    label: "Clipboard cleared".into(),
                    item_name: None,
                    clears_in: None,
                    follow_up_in: None,
                });
            }
        });
        if let Some(old) = self.copy_timers.lock().expect("timers").clear.replace(task) {
            old.abort();
        }
    }

    fn schedule_follow_up(self: &Arc<Self>, generation: u64, id: String, name: String, after_secs: u32) {
        let me = self.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(u64::from(after_secs))).await;
            // Leave the clipboard alone if the password was replaced.
            if !me.clipboard.is_current(generation) {
                return;
            }
            let code = async { me.client().await?.get_totp(&id).await.map_err(ipc_error) }.await;
            let code = match code {
                Ok(code) => Zeroizing::new(code.expose_secret().to_owned()),
                Err(e) => {
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
            if !me.clipboard.is_current(generation) {
                return;
            }
            let clear_secs = me.settings().clipboard_clear_secs;
            if let Some(clear) = me.copy_timers.lock().expect("timers").clear.take() {
                clear.abort();
            }
            match me.put(code, true).await {
                Ok(next) => {
                    me.schedule_clear(next, clear_secs);
                    me.emit_clipboard(ClipboardEvent {
                        state: ClipboardState::FollowUpCopied,
                        label: "Verification code copied".into(),
                        item_name: Some(name),
                        clears_in: Some(clear_secs),
                        follow_up_in: None,
                    });
                }
                Err(e) => tracing::warn!("copying the verification code failed: {e}"),
            }
        });
        if let Some(old) = self.copy_timers.lock().expect("timers").follow_up.replace(task) {
            old.abort();
        }
    }

    /// Cancels a pending one-time-code replacement, keeping the password.
    pub fn skip_follow_up(&self) {
        if let Some(task) = self.copy_timers.lock().expect("timers").follow_up.take() {
            task.abort();
        }
    }

    fn emit_clipboard(&self, clipboard: ClipboardEvent) {
        self.emit(Event::Clipboard { clipboard });
    }
}
