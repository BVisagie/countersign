use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::field::Field;
use crate::model::{
    ClipboardEvent, GenerateOptions, Settings, Status, TerminalLaunch, TotpCode, VaultSnapshot,
};
use crate::secret::Secret;

/// Bumped whenever a message changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 1;

/// The largest line either side accepts.
pub const MAX_FRAME: usize = 64 << 20;

/// A request from the app. `id` pairs it with its response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestFrame {
    pub id: u64,
    pub request: Request,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Hello {
        protocol: u32,
    },
    Status,
    /// Rechecks the CLI and sign-in state.
    Refresh,
    /// Records user activity for the inactivity lock.
    Activity,
    InstallCli,
    SetServer {
        url: Option<String>,
    },
    LoginApiKey {
        client_id: Secret,
        client_secret: Secret,
    },
    /// Returns how to run the official `bw login` in a terminal.
    LoginTerminal,
    /// Reports that the sign-in terminal finished. A session key printed by
    /// `bw login --raw` opens the vault without asking for the password again.
    LoginFinished {
        session: Option<Secret>,
    },
    Logout,
    /// Starts the CLI ahead of an unlock so it is ready when the password is.
    PrepareUnlock,
    Unlock {
        password: Secret,
    },
    Lock,
    /// Checks the master password for an item that requires a re-prompt.
    VerifyPassword {
        item_id: String,
        password: Secret,
    },
    Sync,
    Vault,
    /// The item with secrets removed, and the secret fields it has.
    Item {
        id: String,
    },
    Reveal {
        id: String,
        field: Field,
    },
    /// Copies a field to the clipboard inside the daemon.
    Copy {
        id: String,
        field: Field,
    },
    /// Keeps the copied password instead of replacing it with the one-time code.
    SkipFollowUp,
    /// Copies a value the user already sees, such as a generated password.
    CopyValue {
        value: Secret,
        label: String,
        secret: bool,
    },
    Totp {
        id: String,
    },
    Generate {
        options: GenerateOptions,
    },
    SaveAttachment {
        item_id: String,
        attachment_id: String,
        path: PathBuf,
    },
    /// The complete item, secrets included, for the edit form.
    ItemForEdit {
        id: String,
    },
    CreateItem {
        item: Value,
    },
    EditItem {
        id: String,
        item: Value,
    },
    DeleteItem {
        id: String,
        permanent: bool,
    },
    RestoreItem {
        id: String,
    },
    CreateFolder {
        name: String,
    },
    RenameFolder {
        id: String,
        name: String,
    },
    DeleteFolder {
        id: String,
    },
    MoveItem {
        id: String,
        organization_id: String,
        collection_ids: Vec<String>,
    },
    AddAttachment {
        item_id: String,
        path: PathBuf,
    },
    DeleteAttachment {
        item_id: String,
        attachment_id: String,
    },
    Sends,
    /// Creates a Send. A file Send reads `file`; text Sends leave it empty.
    CreateSend {
        send: Value,
        file: Option<PathBuf>,
    },
    EditSend {
        id: String,
        send: Value,
    },
    DeleteSend {
        id: String,
    },
    RemoveSendPassword {
        id: String,
    },
    Settings,
    UpdateSettings {
        settings: Settings,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Hello { protocol: u32, status: Status },
    Status { status: Status },
    Vault { vault: VaultSnapshot },
    Item { item: Value, secret_fields: Vec<Field> },
    Secret { value: Secret },
    Totp { totp: TotpCode },
    Terminal { launch: TerminalLaunch },
    Json { value: Value },
    List { values: Vec<Value> },
    Settings { settings: Settings },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Locked,
    LoggedOut,
    NotFound,
    WrongPassword,
    /// The item requires the master password first.
    RepromptRequired,
    /// Bitwarden requires Premium for this.
    Premium,
    CliUnavailable,
    Busy,
    Invalid,
    Cli,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl IpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        IpcError { code, message: message.into() }
    }
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for IpcError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Status {
        status: Status,
    },
    /// Items, folders or Sends changed; refresh what is shown.
    VaultChanged,
    Clipboard {
        clipboard: ClipboardEvent,
    },
}

/// Everything the daemon writes: responses and unsolicited events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub enum ServerFrame {
    Response { id: u64, result: Result<Response, IpcError> },
    Event { event: Event },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let frame = RequestFrame { id: 7, request: Request::Unlock { password: Secret::new("pw") } };
        let line = serde_json::to_string(&frame).unwrap();
        assert_eq!(line, r#"{"id":7,"request":{"op":"unlock","password":"pw"}}"#);
        let back: RequestFrame = serde_json::from_str(&line).unwrap();
        assert!(matches!(back.request, Request::Unlock { .. }));

        let reply = ServerFrame::Response { id: 7, result: Err(IpcError::new(ErrorCode::Locked, "locked")) };
        let line = serde_json::to_string(&reply).unwrap();
        let back: ServerFrame = serde_json::from_str(&line).unwrap();
        assert!(matches!(
            back,
            ServerFrame::Response { id: 7, result: Err(IpcError { code: ErrorCode::Locked, .. }) }
        ));
    }

    #[test]
    fn debug_output_hides_secrets() {
        let frame =
            Request::LoginApiKey { client_id: Secret::new("user.id"), client_secret: Secret::new("s3cret") };
        let text = format!("{frame:?}");
        assert!(!text.contains("s3cret") && !text.contains("user.id"));
    }
}
