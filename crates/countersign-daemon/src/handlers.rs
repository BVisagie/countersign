//! Turns app requests into CLI calls.

use std::path::Path;
use std::sync::Arc;

use countersign_bw::{GenerateOptions as BwGenerate, SessionKey};
use countersign_ipc::{
    ErrorCode, Field, GenerateOptions, IpcError, PROTOCOL_VERSION, Request, Response, Secret, TerminalLaunch,
    TotpCode, VaultState, redact, redact_protected,
};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::core::{Daemon, LockReason, ipc_error};
use crate::settings;

/// Bitwarden's limit for one attachment.
const ATTACHMENT_LIMIT: u64 = 500 << 20;

fn secret(value: Secret) -> SecretString {
    SecretString::from(value.expose().to_owned())
}

fn invalid(message: impl Into<String>) -> IpcError {
    IpcError::new(ErrorCode::Invalid, message)
}

impl Daemon {
    pub async fn handle(self: &Arc<Self>, request: Request) -> Result<Response, IpcError> {
        use Request as R;
        match request {
            R::Hello { protocol } => {
                if protocol != PROTOCOL_VERSION {
                    return Err(invalid("Countersign's app and background service are different versions."));
                }
                Ok(Response::Hello { protocol: PROTOCOL_VERSION, status: self.status() })
            }
            R::Status => Ok(Response::Status { status: self.status() }),
            R::Refresh => {
                let _op = self.ops.lock().await;
                self.refresh().await;
                Ok(Response::Status { status: self.status() })
            }
            R::Activity => {
                if self.is_unlocked() {
                    self.touch();
                }
                Ok(Response::Ok)
            }
            R::InstallCli => crate::install::install(self).await.map(|()| Response::Ok),
            R::SetServer { url } => self.set_server(url).await,
            R::LoginApiKey { client_id, client_secret } => {
                let _op = self.ops.lock().await;
                self.require_logged_out()?;
                self.cli()?
                    .login_api_key(&secret(client_id), &secret(client_secret))
                    .await
                    .map_err(ipc_error)?;
                self.refresh().await;
                Ok(Response::Status { status: self.status() })
            }
            R::LoginTerminal => {
                self.require_logged_out()?;
                let cli = self.cli()?;
                let env = cli
                    .environment()
                    .into_iter()
                    .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
                    .collect();
                Ok(Response::Terminal {
                    launch: TerminalLaunch {
                        program: cli.path().to_owned(),
                        args: vec!["login".into(), "--raw".into()],
                        env,
                    },
                })
            }
            R::LoginFinished { session } => {
                {
                    let _op = self.ops.lock().await;
                    self.refresh().await;
                }
                if let Some(key) = session.and_then(|s| SessionKey::from_raw(s.expose().to_owned()))
                    && self.status().vault == VaultState::Locked
                {
                    match self.adopt_session(key).await {
                        Ok(()) => self.spawn_sync(),
                        Err(e) => tracing::info!("could not open the vault after sign-in: {e}"),
                    }
                }
                Ok(Response::Status { status: self.status() })
            }
            R::Logout => {
                self.lock(LockReason::SignedOut).await;
                let _op = self.ops.lock().await;
                self.cli()?.logout().await.map_err(ipc_error)?;
                self.refresh().await;
                Ok(Response::Status { status: self.status() })
            }
            R::PrepareUnlock => self.prepare_unlock().await.map(|()| Response::Ok),
            R::Unlock { password } => {
                self.unlock(secret(password)).await?;
                self.spawn_sync();
                Ok(Response::Ok)
            }
            R::Lock => {
                self.lock(LockReason::Requested).await;
                Ok(Response::Ok)
            }
            R::VerifyPassword { item_id, password } => {
                self.verify_password(&item_id, secret(password)).await.map(|()| Response::Ok)
            }
            R::Sync => self.sync().await.map(|()| Response::Ok),
            R::Vault => {
                if !self.is_unlocked() {
                    return Err(IpcError::new(ErrorCode::Locked, "The vault is locked."));
                }
                Ok(Response::Vault { vault: self.vault() })
            }
            // Showing an item is not activity: the app reloads items on its
            // own after a sync.
            R::Item { id } => {
                let item = self.client().await?.get_item(&id).await.map_err(ipc_error)?;
                let (mut item, mut secret_fields) = redact(&item);
                if item["reprompt"].as_i64() == Some(1) && !self.has_grant(&id) {
                    redact_protected(&mut item, &mut secret_fields);
                }
                Ok(Response::Item { item, secret_fields })
            }
            R::Reveal { id, field } => {
                self.touch();
                let client = self.client().await?;
                let item = client.get_item(&id).await.map_err(ipc_error)?;
                self.check_reprompt(&id, &item)?;
                let value = match field {
                    Field::Totp => {
                        Secret::new(client.get_totp(&id).await.map_err(ipc_error)?.expose_secret())
                    }
                    other => Secret::from(
                        other
                            .read(&item)
                            .ok_or_else(|| IpcError::new(ErrorCode::NotFound, "That field is empty."))?,
                    ),
                };
                Ok(Response::Secret { value })
            }
            R::Copy { id, field } => self.copy_field(&id, field).await,
            R::SkipFollowUp => {
                self.skip_follow_up();
                Ok(Response::Ok)
            }
            R::CopyValue { value, label } => {
                self.copy_value(Zeroizing::new(value.expose().to_owned()), label).await
            }
            // Live codes refresh on a timer, so this is not activity either.
            R::Totp { id } => {
                let client = self.client().await?;
                let item = client.get_item(&id).await.map_err(ipc_error)?;
                self.check_reprompt(&id, &item)?;
                let period = totp_period(item["login"]["totp"].as_str().unwrap_or_default());
                let code = client.get_totp(&id).await.map_err(ipc_error)?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or_default();
                let remaining = period - (now % u64::from(period)) as u32;
                Ok(Response::Totp {
                    totp: TotpCode { code: Secret::new(code.expose_secret()), period, remaining },
                })
            }
            R::Generate { options } => {
                self.touch();
                let value =
                    self.client().await?.generate(&generate_options(&options)).await.map_err(ipc_error)?;
                Ok(Response::Secret { value: Secret::new(value.expose_secret()) })
            }
            R::SaveAttachment { item_id, attachment_id, path } => {
                self.touch();
                let client = self.client().await?;
                let item = client.get_item(&item_id).await.map_err(ipc_error)?;
                self.check_reprompt(&item_id, &item)?;
                let data = client.get_attachment(&item_id, &attachment_id).await.map_err(ipc_error)?;
                write_user_file(&path, &data).await?;
                Ok(Response::Ok)
            }
            R::ItemForEdit { id } => {
                self.touch();
                let item = self.client().await?.get_item(&id).await.map_err(ipc_error)?;
                self.check_reprompt(&id, &item)?;
                Ok(Response::Json { value: item })
            }
            R::CreateItem { item } => {
                self.touch();
                validate_item(&item)?;
                let client = self.client().await?;
                let created = client.create_item(&item).await.map_err(ipc_error)?;
                self.changed(&client).await;
                let (item, secret_fields) = redact(&created);
                Ok(Response::Item { item, secret_fields })
            }
            R::EditItem { id, item } => {
                self.touch();
                validate_item(&item)?;
                let client = self.client().await?;
                let current = client.get_item(&id).await.map_err(ipc_error)?;
                self.check_reprompt(&id, &current)?;
                let edited = client.edit_item(&id, &item).await.map_err(ipc_error)?;
                self.changed(&client).await;
                let (item, secret_fields) = redact(&edited);
                Ok(Response::Item { item, secret_fields })
            }
            R::DeleteItem { id, permanent } => {
                self.touch();
                let client = self.client().await?;
                client.delete_item(&id, permanent).await.map_err(ipc_error)?;
                self.changed(&client).await;
                Ok(Response::Ok)
            }
            R::RestoreItem { id } => {
                self.touch();
                let client = self.client().await?;
                client.restore_item(&id).await.map_err(ipc_error)?;
                self.changed(&client).await;
                Ok(Response::Ok)
            }
            R::CreateFolder { name } => {
                let name = folder_name(&name)?;
                let client = self.client().await?;
                let folder = client.create_folder(name).await.map_err(ipc_error)?;
                self.changed(&client).await;
                Ok(Response::Json { value: folder })
            }
            R::RenameFolder { id, name } => {
                let name = folder_name(&name)?;
                let client = self.client().await?;
                let folder = client.edit_folder(&id, name).await.map_err(ipc_error)?;
                self.changed(&client).await;
                Ok(Response::Json { value: folder })
            }
            R::DeleteFolder { id } => {
                let client = self.client().await?;
                client.delete_folder(&id).await.map_err(ipc_error)?;
                self.changed(&client).await;
                Ok(Response::Ok)
            }
            R::MoveItem { id, organization_id, collection_ids } => {
                self.touch();
                if collection_ids.is_empty() {
                    return Err(invalid("Choose at least one collection."));
                }
                let client = self.client().await?;
                let moved =
                    client.move_item(&id, &organization_id, &collection_ids).await.map_err(ipc_error)?;
                self.changed(&client).await;
                let (item, secret_fields) = redact(&moved);
                Ok(Response::Item { item, secret_fields })
            }
            R::AddAttachment { item_id, path } => {
                self.touch();
                let data = read_user_file(&path).await?;
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("attachment").to_owned();
                let client = self.client().await?;
                let item = client.create_attachment(&item_id, &name, &data).await.map_err(ipc_error)?;
                self.changed(&client).await;
                let (item, secret_fields) = redact(&item);
                Ok(Response::Item { item, secret_fields })
            }
            R::DeleteAttachment { item_id, attachment_id } => {
                self.touch();
                let client = self.client().await?;
                client.delete_attachment(&item_id, &attachment_id).await.map_err(ipc_error)?;
                self.changed(&client).await;
                Ok(Response::Ok)
            }
            R::Sends => {
                let values = self.client().await?.list_sends().await.map_err(ipc_error)?;
                Ok(Response::List { values })
            }
            R::CreateSend { send, file } => {
                self.touch();
                let client = self.client().await?;
                let created = match file {
                    None => client.create_send(&send).await.map_err(ipc_error)?,
                    Some(_) => return Err(invalid("File Sends are not available yet.")),
                };
                Ok(Response::Json { value: created })
            }
            R::EditSend { id, send } => {
                self.touch();
                let edited = self.client().await?.edit_send(&id, &send).await.map_err(ipc_error)?;
                Ok(Response::Json { value: edited })
            }
            R::DeleteSend { id } => {
                self.touch();
                self.client().await?.delete_send(&id).await.map_err(ipc_error)?;
                Ok(Response::Ok)
            }
            R::RemoveSendPassword { id } => {
                self.touch();
                let send = self.client().await?.remove_send_password(&id).await.map_err(ipc_error)?;
                Ok(Response::Json { value: send })
            }
            R::Settings => Ok(Response::Settings { settings: self.settings() }),
            R::UpdateSettings { settings: new } => {
                let new = new.normalized();
                let cli_changed = new.cli_path != self.settings().cli_path;
                settings::save(&self.paths.settings_file, &new).map_err(|e| {
                    IpcError::new(ErrorCode::Internal, format!("Saving settings failed: {e}"))
                })?;
                self.set_settings(new.clone());
                if cli_changed {
                    self.lock(LockReason::Requested).await;
                    let _op = self.ops.lock().await;
                    self.refresh().await;
                }
                Ok(Response::Settings { settings: new })
            }
        }
    }

    fn require_logged_out(&self) -> Result<(), IpcError> {
        match self.status().vault {
            VaultState::LoggedOut => Ok(()),
            _ => Err(invalid("Sign out first.")),
        }
    }

    async fn set_server(&self, url: Option<String>) -> Result<Response, IpcError> {
        let _op = self.ops.lock().await;
        self.require_logged_out()?;
        let url = url.map(|u| u.trim().trim_end_matches('/').to_owned()).filter(|u| !u.is_empty());
        if let Some(url) = &url {
            let valid = (url.starts_with("https://") || url.starts_with("http://"))
                && url.len() < 2048
                && !url.contains(char::is_whitespace)
                && url.split("://").nth(1).is_some_and(|rest| !rest.is_empty());
            if !valid {
                return Err(invalid("Enter the server address, starting with https://."));
            }
        }
        self.cli()?.config_server(url.as_deref()).await.map_err(ipc_error)?;
        self.refresh().await;
        Ok(Response::Status { status: self.status() })
    }

    fn spawn_sync(self: &Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            if let Err(e) = me.sync().await {
                tracing::info!("sync after unlock failed: {e}");
            }
        });
    }
}

fn generate_options(options: &GenerateOptions) -> BwGenerate {
    BwGenerate {
        passphrase: options.passphrase,
        length: Some(options.length.clamp(5, 128)),
        uppercase: options.uppercase,
        lowercase: options.lowercase,
        number: options.number,
        special: options.special,
        min_number: Some(options.min_number.min(9)),
        min_special: Some(options.min_special.min(9)),
        avoid_ambiguous: options.avoid_ambiguous,
        words: Some(options.words.clamp(3, 20)),
        separator: Some(options.separator.chars().take(1).collect()),
        capitalize: options.capitalize,
        include_number: options.include_number,
    }
}

/// The TOTP period from an `otpauth://` URI; 30 seconds otherwise.
fn totp_period(totp: &str) -> u32 {
    totp.strip_prefix("otpauth://")
        .and_then(|rest| rest.split_once('?'))
        .and_then(|(_, query)| {
            query.split('&').find_map(|pair| pair.strip_prefix("period=").and_then(|p| p.parse::<u32>().ok()))
        })
        .filter(|p| (1..=600).contains(p))
        .unwrap_or(30)
}

fn validate_item(item: &Value) -> Result<(), IpcError> {
    let kind = item["type"].as_i64().and_then(countersign_ipc::ItemKind::from_type);
    let name = item["name"].as_str().map(str::trim).unwrap_or_default();
    match (kind, name.is_empty()) {
        (None, _) => Err(invalid("Unknown item type.")),
        (_, true) => Err(invalid("Give the item a name.")),
        _ => Ok(()),
    }
}

fn folder_name(name: &str) -> Result<&str, IpcError> {
    let name = name.trim();
    if name.is_empty() { Err(invalid("Give the folder a name.")) } else { Ok(name) }
}

async fn read_user_file(path: &Path) -> Result<Vec<u8>, IpcError> {
    let meta = tokio::fs::metadata(path).await.map_err(|e| invalid(format!("Cannot read the file: {e}")))?;
    if !meta.is_file() {
        return Err(invalid("Choose a file."));
    }
    if meta.len() > ATTACHMENT_LIMIT {
        return Err(invalid("Attachments can be at most 500 MB."));
    }
    tokio::fs::read(path).await.map_err(|e| invalid(format!("Cannot read the file: {e}")))
}

async fn write_user_file(path: &Path, data: &[u8]) -> Result<(), IpcError> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .await
        .map_err(|e| invalid(format!("Cannot save the file: {e}")))?;
    file.write_all(data).await.map_err(|e| invalid(format!("Cannot save the file: {e}")))?;
    file.sync_all().await.map_err(|e| invalid(format!("Cannot save the file: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_periods() {
        assert_eq!(totp_period("JBSWY3DPEHPK3PXP"), 30);
        assert_eq!(totp_period("otpauth://totp/x?secret=A&period=60&digits=8"), 60);
        assert_eq!(totp_period("otpauth://totp/x?period=0"), 30);
        assert_eq!(totp_period("steam://ABC"), 30);
    }

    #[test]
    fn items_need_a_type_and_name() {
        assert!(validate_item(&serde_json::json!({"type": 1, "name": "x"})).is_ok());
        assert!(validate_item(&serde_json::json!({"type": 9, "name": "x"})).is_err());
        assert!(validate_item(&serde_json::json!({"type": 2, "name": "  "})).is_err());
    }
}
