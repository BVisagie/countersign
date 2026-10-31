//! Installs the pinned official CLI release.

use std::sync::Arc;

use countersign_ipc::{ErrorCode, IpcError};

use crate::core::Daemon;

pub async fn install(_daemon: &Arc<Daemon>) -> Result<(), IpcError> {
    Err(IpcError::new(ErrorCode::Invalid, "Installing the CLI from Countersign is not available yet."))
}
