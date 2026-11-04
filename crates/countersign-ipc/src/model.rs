use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::field::Field;

/// The kinds of vault item Bitwarden stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Login,
    SecureNote,
    Card,
    Identity,
    SshKey,
}

impl ItemKind {
    pub fn from_type(value: i64) -> Option<Self> {
        Some(match value {
            1 => ItemKind::Login,
            2 => ItemKind::SecureNote,
            3 => ItemKind::Card,
            4 => ItemKind::Identity,
            5 => ItemKind::SshKey,
            _ => return None,
        })
    }

    pub fn type_number(self) -> i64 {
        match self {
            ItemKind::Login => 1,
            ItemKind::SecureNote => 2,
            ItemKind::Card => 3,
            ItemKind::Identity => 4,
            ItemKind::SshKey => 5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ItemKind::Login => "Login",
            ItemKind::SecureNote => "Note",
            ItemKind::Card => "Card",
            ItemKind::Identity => "Identity",
            ItemKind::SshKey => "SSH key",
        }
    }
}

/// Searchable, non-secret metadata for one item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemSummary {
    pub id: String,
    pub kind: ItemKind,
    pub name: String,
    /// Username, card brand and last digits, identity name or key fingerprint.
    pub subtitle: Option<String>,
    pub username: Option<String>,
    /// Lower-case hosts of the login's websites, for search and icons.
    pub hosts: Vec<String>,
    pub first_uri: Option<String>,
    pub favorite: bool,
    pub folder_id: Option<String>,
    pub organization_id: Option<String>,
    pub collection_ids: Vec<String>,
    pub has_password: bool,
    pub has_totp: bool,
    pub has_passkey: bool,
    pub attachments: u32,
    pub reprompt: bool,
    pub deleted: bool,
    pub revision: Option<String>,
}

/// Builds the summary for a full item returned by the CLI. Card numbers
/// contribute only their last four digits.
pub fn summarize(item: &Value) -> Option<ItemSummary> {
    let text = |v: &Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let id = text(&item["id"])?;
    let kind = ItemKind::from_type(item["type"].as_i64()?)?;
    let name = text(&item["name"]).unwrap_or_else(|| "(no name)".into());
    let uris: Vec<String> = item["login"]["uris"]
        .as_array()
        .map(|list| list.iter().filter_map(|u| text(&u["uri"])).collect())
        .unwrap_or_default();
    let mut hosts: Vec<String> = uris.iter().filter_map(|u| host_of(u)).collect();
    hosts.dedup();
    let username = match kind {
        ItemKind::Login => text(&item["login"]["username"]),
        ItemKind::Identity => text(&item["identity"]["username"]),
        _ => None,
    };
    let subtitle = match kind {
        ItemKind::Login => username.clone(),
        ItemKind::Card => {
            let brand = text(&item["card"]["brand"]);
            let last4 =
                Field::CardNumber.read(item).filter(|n| n.len() >= 4).map(|n| n[n.len() - 4..].to_owned());
            match (brand, last4) {
                (Some(b), Some(l)) => Some(format!("{b}, *{l}")),
                (None, Some(l)) => Some(format!("*{l}")),
                (b, None) => b,
            }
        }
        ItemKind::Identity => {
            let parts: Vec<String> =
                ["firstName", "lastName"].iter().filter_map(|k| text(&item["identity"][*k])).collect();
            if parts.is_empty() { text(&item["identity"]["email"]) } else { Some(parts.join(" ")) }
        }
        ItemKind::SshKey => text(&item["sshKey"]["keyFingerprint"]),
        ItemKind::SecureNote => None,
    };
    let collection_ids =
        item["collectionIds"].as_array().map(|c| c.iter().filter_map(&text).collect()).unwrap_or_default();
    Some(ItemSummary {
        id,
        kind,
        name,
        subtitle,
        username,
        hosts,
        first_uri: uris.into_iter().next(),
        favorite: item["favorite"].as_bool().unwrap_or(false),
        folder_id: text(&item["folderId"]),
        organization_id: text(&item["organizationId"]),
        collection_ids,
        has_password: text(&item["login"]["password"]).is_some(),
        has_totp: text(&item["login"]["totp"]).is_some(),
        has_passkey: item["login"]["fido2Credentials"].as_array().is_some_and(|p| !p.is_empty()),
        attachments: item["attachments"].as_array().map_or(0, |a| a.len() as u32),
        reprompt: item["reprompt"].as_i64() == Some(1),
        deleted: !item["deletedDate"].is_null(),
        revision: text(&item["revisionDate"]),
    })
}

/// The lower-case host of a web address, without user info or port.
/// Addresses without a scheme are treated as hosts, as Bitwarden does.
pub(crate) fn host_of(uri: &str) -> Option<String> {
    let rest = match uri.split_once("://") {
        Some((scheme, rest)) if scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) => {
            rest
        }
        Some(_) => return None,
        None => uri,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    let host = if host_port.starts_with('[') {
        host_port.split(']').next().map(|h| format!("{h}]"))?
    } else {
        host_port.split(':').next()?.to_owned()
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty() && !host.contains(char::is_whitespace)).then_some(host)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub organization_id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Organization {
    pub id: String,
    pub name: String,
}

/// Everything the app needs to browse and search the vault, without secrets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultSnapshot {
    pub items: Vec<ItemSummary>,
    pub trash: Vec<ItemSummary>,
    pub folders: Vec<Folder>,
    pub collections: Vec<Collection>,
    pub organizations: Vec<Organization>,
    /// Item ids most recently copied from, newest first.
    pub recent: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CliState {
    Checking,
    Missing,
    Unsupported { found: String, minimum: String },
    Installing,
    Ready { version: String, managed: bool },
    Failed { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VaultState {
    Unknown,
    LoggedOut,
    Locked,
    Unlocking,
    Unlocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub email: Option<String>,
    pub server_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub cli: CliState,
    pub vault: VaultState,
    pub account: Option<Account>,
    /// The server the next sign-in will use. `None` means the US cloud.
    pub server_url: Option<String>,
    pub last_sync: Option<String>,
    pub syncing: bool,
}

/// The current one-time code and how long it stays valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TotpCode {
    pub code: crate::Secret,
    pub period: u32,
    pub remaining: u32,
}

/// How the app should start `bw login` in its embedded terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalLaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardState {
    /// A value is on the clipboard and will be cleared.
    Copied,
    /// The one-time code replaced the password.
    FollowUpCopied,
    /// Countersign cleared its value, or another app replaced it.
    Cleared,
    Failed,
}

/// Clipboard changes the app shows as receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardEvent {
    pub state: ClipboardState,
    pub label: String,
    pub item_name: Option<String>,
    /// Seconds until Countersign clears the value.
    pub clears_in: Option<u32>,
    /// Seconds until the one-time code replaces the password.
    pub follow_up_in: Option<u32>,
}

/// Options for the official password and passphrase generator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GenerateOptions {
    pub passphrase: bool,
    pub length: u32,
    pub uppercase: bool,
    pub lowercase: bool,
    pub number: bool,
    pub special: bool,
    pub min_number: u32,
    pub min_special: u32,
    pub avoid_ambiguous: bool,
    pub words: u32,
    pub separator: String,
    pub capitalize: bool,
    pub include_number: bool,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        GenerateOptions {
            passphrase: false,
            length: 20,
            uppercase: true,
            lowercase: true,
            number: true,
            special: true,
            min_number: 1,
            min_special: 1,
            avoid_ambiguous: false,
            words: 5,
            separator: "-".into(),
            capitalize: true,
            include_number: true,
        }
    }
}

/// Preferences, stored by the daemon in `~/.config/countersign/settings.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub clipboard_clear_secs: u32,
    pub totp_follow_up: bool,
    pub totp_follow_up_secs: u32,
    pub lock_after_idle_mins: u32,
    pub lock_on_screen_lock: bool,
    pub lock_on_suspend: bool,
    pub sync_interval_mins: u32,
    pub close_to_background: bool,
    pub cli_path: Option<PathBuf>,
    /// The generator's last options, shared by the generator page and the
    /// editor's generate button.
    pub generator: GenerateOptions,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            clipboard_clear_secs: 30,
            totp_follow_up: true,
            totp_follow_up_secs: 5,
            lock_after_idle_mins: 15,
            lock_on_screen_lock: true,
            lock_on_suspend: true,
            sync_interval_mins: 10,
            close_to_background: true,
            cli_path: None,
            generator: GenerateOptions::default(),
        }
    }
}

impl Settings {
    /// Clamps values into supported ranges.
    pub fn normalized(mut self) -> Self {
        self.clipboard_clear_secs = self.clipboard_clear_secs.clamp(5, 600);
        self.totp_follow_up_secs =
            self.totp_follow_up_secs.clamp(1, self.clipboard_clear_secs.saturating_sub(1).max(1));
        if self.lock_after_idle_mins != 0 {
            self.lock_after_idle_mins = self.lock_after_idle_mins.clamp(1, 1440);
        }
        if self.sync_interval_mins != 0 {
            self.sync_interval_mins = self.sync_interval_mins.clamp(1, 1440);
        }
        self.generator = self.generator.normalized();
        self
    }
}

impl GenerateOptions {
    /// Clamps values into the ranges the generator offers.
    pub fn normalized(mut self) -> Self {
        self.length = self.length.clamp(5, 128);
        self.min_number = self.min_number.min(9);
        self.min_special = self.min_special.min(9);
        self.words = self.words.clamp(3, 20);
        self.separator = self.separator.chars().take(1).collect();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summarizes_logins_without_secrets() {
        let item = json!({
            "id": "0b6a1c4e-1f2d-4c3b-9a8e-7d6c5b4a3f21",
            "type": 1, "name": "GitHub", "favorite": true, "reprompt": 0,
            "folderId": null, "organizationId": null, "deletedDate": null,
            "login": {
                "username": "me", "password": "pw", "totp": "KEY",
                "uris": [{"uri": "https://User@GitHub.com:443/login"}, {"uri": "github.com"}]
            },
            "attachments": [{"id": "a"}]
        });
        let s = summarize(&item).unwrap();
        assert_eq!(s.kind, ItemKind::Login);
        assert_eq!(s.subtitle.as_deref(), Some("me"));
        assert_eq!(s.hosts, vec!["github.com"]);
        assert!(s.has_password && s.has_totp && s.favorite);
        assert_eq!(s.attachments, 1);
        let text = serde_json::to_string(&s).unwrap();
        assert!(!text.contains("\"pw\"") && !text.contains("KEY"));
    }

    #[test]
    fn card_summary_shows_only_last_four() {
        let item = json!({"id": "x", "type": 3, "name": "Visa", "card": {"brand": "Visa", "number": "4111 1111 1111 1234"}});
        let s = summarize(&item).unwrap();
        assert_eq!(s.subtitle.as_deref(), Some("Visa, *1234"));
        assert!(!serde_json::to_string(&s).unwrap().contains("4111"));
    }

    #[test]
    fn hosts_are_parsed_conservatively() {
        assert_eq!(host_of("https://a.example.com/x").as_deref(), Some("a.example.com"));
        assert_eq!(host_of("example.com:8080/x").as_deref(), Some("example.com"));
        assert_eq!(host_of("androidapp://com.example").as_deref(), Some("com.example"));
        assert_eq!(host_of("https://[::1]:8080/").as_deref(), Some("[::1]"));
        assert_eq!(host_of("not a url"), None);
        assert_eq!(host_of("a b://x"), None);
    }

    #[test]
    fn settings_are_clamped() {
        let s =
            Settings { clipboard_clear_secs: 1, totp_follow_up_secs: 99, ..Settings::default() }.normalized();
        assert_eq!(s.clipboard_clear_secs, 5);
        assert_eq!(s.totp_follow_up_secs, 4);
        let parsed: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, Settings::default());
        let generator =
            GenerateOptions { length: 500, words: 1, separator: "-_".into(), ..Default::default() };
        let s = Settings { generator, ..Settings::default() }.normalized();
        assert_eq!((s.generator.length, s.generator.words, s.generator.separator.as_str()), (128, 3, "-"));
        // Settings from before the generator was remembered still read.
        let parsed: Settings = serde_json::from_str(r#"{"clipboard_clear_secs": 45}"#).unwrap();
        assert_eq!(parsed.generator, GenerateOptions::default());
    }
}
