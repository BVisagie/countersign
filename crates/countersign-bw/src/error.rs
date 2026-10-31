use std::path::PathBuf;

use crate::version::CliVersion;

pub type Result<T, E = BwError> = std::result::Result<T, E>;

/// Failures reported by the official CLI or by the adapter around it.
#[derive(Debug, thiserror::Error)]
pub enum BwError {
    #[error("the Bitwarden CLI was not found at {0}")]
    CliMissing(PathBuf),
    #[error("Bitwarden CLI {found} is not supported; Countersign needs {minimum} or newer")]
    Unsupported { found: String, minimum: CliVersion },
    #[error("the vault is locked")]
    Locked,
    #[error("you are not logged in")]
    NotLoggedIn,
    #[error("the item was not found")]
    NotFound,
    #[error("{0}")]
    Cli(String),
    #[error("the Bitwarden CLI did not respond in time")]
    Timeout,
    #[error("the Bitwarden CLI stopped unexpectedly")]
    Disconnected,
    #[error("unexpected response from the Bitwarden CLI: {0}")]
    Protocol(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl BwError {
    /// Maps a failure message from the CLI to a typed error where one exists.
    pub(crate) fn from_cli_message(message: &str) -> Self {
        match message.trim() {
            "Vault is locked." => BwError::Locked,
            "You are not logged in." => BwError::NotLoggedIn,
            "Not found." => BwError::NotFound,
            other => BwError::Cli(other.to_owned()),
        }
    }
}
