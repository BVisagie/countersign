//! Messages and vault models shared by the Countersign daemon and app.
//!
//! The app talks to the daemon over a private Unix socket using one JSON
//! object per line. The daemon owns the Bitwarden session; the app receives
//! item metadata, and a secret value only when the user asks to see or edit
//! it. Copying never sends the secret to the app.

mod field;
mod model;
mod protocol;
mod secret;

pub use field::{Field, redact, redact_protected};
pub use model::{
    Account, CliState, ClipboardEvent, ClipboardState, Collection, Folder, GenerateOptions, ItemKind,
    ItemSummary, Organization, Settings, Status, TerminalLaunch, TotpCode, VaultSnapshot, VaultState,
    summarize,
};
pub use protocol::{
    ErrorCode, Event, IpcError, MAX_FRAME, PROTOCOL_VERSION, Request, RequestFrame, Response, ServerFrame,
};
pub use secret::Secret;

use std::path::PathBuf;

/// The per-user runtime folder holding the daemon socket and lock.
/// `COUNTERSIGN_RUNTIME_DIR` overrides it, which keeps development and test
/// instances apart from the one you use.
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("COUNTERSIGN_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/countersign-{}", current_uid())));
    base.join("countersign")
}

pub fn socket_path() -> PathBuf {
    runtime_dir().join("daemon.sock")
}

/// The current user's id, read from `/proc/self`. Only used when
/// `XDG_RUNTIME_DIR` is missing; the daemon still verifies the folder's
/// owner and mode before using it.
fn current_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(u32::MAX)
}
