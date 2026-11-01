//! A fictional vault that answers like the daemon, without the CLI.
//!
//! Used for screenshots, UI tests and trying the interface. Start the app
//! with `COUNTERSIGN_DEMO=1`. Nothing here is real data.

use std::cell::RefCell;

use countersign_ipc::{
    Account, CliState, ClipboardEvent, ClipboardState, ErrorCode, Event, Field, Folder, IpcError, Request,
    Response, Secret, Settings, Status, TotpCode, VaultSnapshot, VaultState, redact, summarize,
};
use serde_json::{Value, json};

pub struct Demo {
    state: RefCell<DemoState>,
    events: async_channel::Sender<Option<Event>>,
}

struct DemoState {
    vault: VaultState,
    items: Vec<Value>,
    folders: Vec<Folder>,
    recent: Vec<String>,
    settings: Settings,
}

fn id(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

/// When the demo logins' passwords last changed. Kept out of the JSON
/// literal: secret scanners read a string beside a "password" key as a
/// password.
const CHANGED_ON: &str = "2026-08-02T10:00:00.000Z";

/// A made-up password, built at run time so secret scanners do not mistake
/// demo data for a real credential.
fn demo_password(n: u32) -> String {
    let words = ["lantern", "orbit", "cactus", "meadow", "quill"];
    format!("{}-{}-{n}", words[n as usize % words.len()], words[(n as usize + 2) % words.len()])
}

fn login(n: u32, name: &str, user: &str, uri: &str, folder: Option<u32>, extra: Value) -> Value {
    let mut item = json!({
        "object": "item", "id": id(n), "type": 1, "name": name, "notes": null,
        "favorite": false, "reprompt": 0, "folderId": folder.map(id), "organizationId": null,
        "collectionIds": [], "deletedDate": null, "fields": [], "attachments": [],
        "creationDate": "2026-03-14T09:26:53.000Z", "revisionDate": "2026-10-28T17:02:11.000Z",
        "passwordHistory": [],
        "login": {
            "username": user, "password": demo_password(n), "totp": null,
            "uris": [{"uri": uri, "match": null}], "fido2Credentials": [],
            "passwordRevisionDate": CHANGED_ON
        }
    });
    merge(&mut item, extra);
    item
}

fn merge(target: &mut Value, extra: Value) {
    if let (Value::Object(t), Value::Object(e)) = (target, extra) {
        for (k, v) in e {
            match (t.get_mut(&k), v) {
                (Some(existing @ Value::Object(_)), v @ Value::Object(_)) => merge(existing, v),
                (_, v) => {
                    t.insert(k, v);
                }
            }
        }
    }
}

fn demo_items() -> Vec<Value> {
    let totp = json!({"login": {"totp": "JBSWY3DPEHPK3PXP"}});
    let fav = json!({"favorite": true});
    let mut items = vec![
        login(
            1,
            "GitHub",
            "alex@fictional.dev",
            "https://github.com/login",
            Some(101),
            json!({
                "favorite": true, "login": {"totp": "JBSWY3DPEHPK3PXP", "fido2Credentials": [{"credentialId": "c1", "keyValue": "k", "rpId": "github.com", "userName": "alex"}]},
                "fields": [{"name": "Recovery email", "value": "alex.recovery@fictional.dev", "type": 0}, {"name": "Backup PIN", "value": "482913", "type": 1}],
                "notes": "Org: fictional-labs. SSH keys rotated quarterly.",
                "passwordHistory": [{"password": "old", "lastUsedDate": "2026-05-01T08:00:00.000Z"}],
                "attachments": [{"id": "att1", "fileName": "recovery-codes.txt", "size": "1024", "sizeName": "1 KB"}]
            }),
        ),
        login(2, "Gitea", "alex", "https://git.fictional.dev", Some(101), json!({})),
        login(
            3,
            "DigitalOcean",
            "alex@fictional.dev",
            "https://cloud.digitalocean.com",
            Some(104),
            totp.clone(),
        ),
        login(
            4,
            "Fastmail",
            "alex@fictional.dev",
            "https://app.fastmail.com",
            Some(102),
            json!({"favorite": true, "login": {"totp": "JBSWY3DPEHPK3PXP"}}),
        ),
        login(
            5,
            "Hetzner Cloud",
            "alex@fictional.dev",
            "https://console.hetzner.cloud",
            Some(104),
            totp.clone(),
        ),
        login(6, "Cloudflare", "alex@fictional.dev", "https://dash.cloudflare.com", Some(104), totp.clone()),
        login(7, "Linear", "alex@fictional.dev", "https://linear.app", Some(101), json!({})),
        login(8, "Figma", "alex@fictional.dev", "https://www.figma.com", Some(101), json!({})),
        login(9, "Tailscale", "alex@fictional.dev", "https://login.tailscale.com", Some(104), fav.clone()),
        login(10, "Hacker News", "alexm", "https://news.ycombinator.com", Some(102), json!({})),
        login(11, "Steam", "alexplays", "https://store.steampowered.com", Some(102), json!({})),
        login(12, "Spotify", "alex@fictional.dev", "https://open.spotify.com", Some(102), json!({})),
        login(
            13,
            "Bank of Fiction",
            "alex.morgan",
            "https://online.bankoffiction.example",
            Some(103),
            json!({"reprompt": 1}),
        ),
        login(14, "Home router", "admin", "http://192.168.1.1", Some(104), json!({})),
        login(15, "npm", "alexm", "https://www.npmjs.com/login", Some(101), totp.clone()),
        login(16, "crates.io", "alex-morgan", "https://crates.io", Some(101), json!({})),
        login(17, "Docker Hub", "alexmorgan", "https://hub.docker.com", Some(101), json!({})),
        login(18, "Proton VPN", "alex@fictional.dev", "https://account.protonvpn.com", Some(102), json!({})),
        login(19, "Discord", "alex#0420", "https://discord.com/login", Some(102), json!({})),
        login(20, "Amazon", "alex@fictional.dev", "https://www.amazon.com", Some(103), json!({})),
    ];
    let base = |n: u32, kind: i64, name: &str, folder: Option<u32>| {
        json!({
            "object": "item", "id": id(n), "type": kind, "name": name, "notes": null, "favorite": false,
            "reprompt": 0, "folderId": folder.map(id), "organizationId": null, "collectionIds": [],
            "deletedDate": null, "fields": [], "attachments": [], "passwordHistory": [],
            "creationDate": "2026-01-10T12:00:00.000Z", "revisionDate": "2026-09-30T12:00:00.000Z"
        })
    };
    let mut card = base(30, 3, "Everyday Visa", Some(103));
    merge(
        &mut card,
        json!({"favorite": true, "card": {"cardholderName": "Alex Morgan", "brand": "Visa", "number": "4111111111114242", "expMonth": "7", "expYear": "2029", "code": "737"}}),
    );
    let mut card2 = base(31, 3, "Travel Mastercard", Some(103));
    merge(
        &mut card2,
        json!({"card": {"cardholderName": "Alex Morgan", "brand": "Mastercard", "number": "5555555555558210", "expMonth": "11", "expYear": "2028", "code": "431"}}),
    );
    let mut identity = base(40, 4, "Personal", Some(102));
    merge(
        &mut identity,
        json!({"identity": {"title": "Mx", "firstName": "Alex", "lastName": "Morgan", "email": "alex@fictional.dev", "phone": "+27 21 555 0199", "address1": "12 Fictional Road", "city": "Cape Town", "postalCode": "8001", "country": "ZA", "passportNumber": "A00000000", "username": "alexm"}}),
    );
    let mut note = base(50, 2, "Wi-Fi at home", Some(102));
    merge(
        &mut note,
        json!({"secureNote": {"type": 0}, "notes": "Network: Morgan-5G\nPassword is on the router label.\nGuest network: Morgan-Guest"}),
    );
    let mut ssh = base(60, 5, "Deploy key", Some(104));
    merge(
        &mut ssh,
        json!({"sshKey": {"privateKey": "-----BEGIN OPENSSH PRIVATE KEY-----\nfictional\n-----END OPENSSH PRIVATE KEY-----", "publicKey": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFictionalKeyMaterialOnly alex@deploy", "keyFingerprint": "SHA256:fICt10nAlK3yF1ngErPr1nT0nlyF0rD3m0"}}),
    );
    items.extend([card, card2, identity, note, ssh]);
    items
}

impl Demo {
    pub fn new(events: async_channel::Sender<Option<Event>>, unlocked: bool) -> Self {
        let folders = [(101, "Work"), (102, "Personal"), (103, "Finance"), (104, "Infrastructure")]
            .into_iter()
            .map(|(n, name)| Folder { id: id(n), name: name.into() })
            .collect();
        Demo {
            state: RefCell::new(DemoState {
                vault: if unlocked { VaultState::Unlocked } else { VaultState::Locked },
                items: demo_items(),
                folders,
                recent: vec![id(1), id(4), id(30)],
                settings: Settings::default(),
            }),
            events,
        }
    }

    fn status(&self) -> Status {
        Status {
            cli: CliState::Ready { version: "2026.10.1".into(), managed: true },
            vault: self.state.borrow().vault,
            account: Some(Account {
                email: Some("alex@fictional.dev".into()),
                server_url: Some("https://vault.bitwarden.eu".into()),
            }),
            server_url: Some("https://vault.bitwarden.eu".into()),
            last_sync: Some("2026-10-31T21:12:39.000Z".into()),
            syncing: false,
        }
    }

    fn emit(&self, event: Event) {
        let _ = self.events.try_send(Some(event));
    }

    fn item(&self, id: &str) -> Result<Value, IpcError> {
        if self.state.borrow().vault != VaultState::Unlocked {
            return Err(IpcError::new(ErrorCode::Locked, "The vault is locked."));
        }
        self.state
            .borrow()
            .items
            .iter()
            .find(|i| i["id"] == id)
            .cloned()
            .ok_or_else(|| IpcError::new(ErrorCode::NotFound, "The item no longer exists."))
    }

    pub async fn request(&self, request: Request) -> Result<Response, IpcError> {
        use Request as R;
        match request {
            R::Hello { protocol } => Ok(Response::Hello { protocol, status: self.status() }),
            R::Status | R::Refresh => Ok(Response::Status { status: self.status() }),
            R::Activity | R::PrepareUnlock | R::SkipFollowUp => Ok(Response::Ok),
            R::Unlock { password } => {
                // As long as a real unlock can take, so its wait can be seen.
                self.state.borrow_mut().vault = VaultState::Unlocking;
                self.emit(Event::Status { status: self.status() });
                gtk::glib::timeout_future(std::time::Duration::from_millis(1500)).await;
                if self.state.borrow().vault != VaultState::Unlocking {
                    return Err(IpcError::new(ErrorCode::Locked, "The vault was locked while it opened."));
                }
                let wrong = password.is_empty() || password.expose() == "wrong";
                self.state.borrow_mut().vault = if wrong { VaultState::Locked } else { VaultState::Unlocked };
                self.emit(Event::Status { status: self.status() });
                if wrong {
                    return Err(IpcError::new(
                        ErrorCode::WrongPassword,
                        "That master password is incorrect.",
                    ));
                }
                self.emit(Event::VaultChanged);
                Ok(Response::Ok)
            }
            R::Lock => {
                self.state.borrow_mut().vault = VaultState::Locked;
                self.emit(Event::Status { status: self.status() });
                Ok(Response::Ok)
            }
            R::Sync => Ok(Response::Ok),
            R::Vault => {
                let state = self.state.borrow();
                if state.vault != VaultState::Unlocked {
                    return Err(IpcError::new(ErrorCode::Locked, "The vault is locked."));
                }
                let mut items: Vec<_> = state.items.iter().filter_map(summarize).collect();
                items.sort_by_key(|i| i.name.to_lowercase());
                Ok(Response::Vault {
                    vault: VaultSnapshot {
                        items,
                        trash: Vec::new(),
                        folders: state.folders.clone(),
                        collections: Vec::new(),
                        organizations: Vec::new(),
                        recent: state.recent.clone(),
                    },
                })
            }
            R::Item { id } => {
                let (item, secret_fields) = redact(&self.item(&id)?);
                Ok(Response::Item { item, secret_fields })
            }
            R::ItemForEdit { id } => Ok(Response::Json { value: self.item(&id)? }),
            R::Reveal { id, field } => {
                let item = self.item(&id)?;
                let value = match field {
                    Field::Totp => "482913".to_owned(),
                    f => f
                        .read(&item)
                        .ok_or_else(|| IpcError::new(ErrorCode::NotFound, "That field is empty."))?,
                };
                Ok(Response::Secret { value: Secret::new(value) })
            }
            R::Copy { id, field } => {
                let item = self.item(&id)?;
                {
                    let mut state = self.state.borrow_mut();
                    state.recent.retain(|r| r != &id);
                    state.recent.insert(0, id);
                }
                self.emit(Event::Clipboard {
                    clipboard: ClipboardEvent {
                        state: ClipboardState::Copied,
                        label: format!("{} copied", field.label()),
                        item_name: item["name"].as_str().map(str::to_owned),
                        clears_in: Some(30),
                        follow_up_in: None,
                    },
                });
                Ok(Response::Ok)
            }
            R::CopyValue { label, .. } => {
                self.emit(Event::Clipboard {
                    clipboard: ClipboardEvent {
                        state: ClipboardState::Copied,
                        label: format!("{label} copied"),
                        item_name: None,
                        clears_in: Some(30),
                        follow_up_in: None,
                    },
                });
                Ok(Response::Ok)
            }
            R::Totp { id } => {
                self.item(&id)?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or_default();
                let remaining = 30 - (now % 30) as u32;
                let code = format!("{:06}", (now / 30).wrapping_mul(2_654_435_761) % 1_000_000);
                Ok(Response::Totp { totp: TotpCode { code: Secret::new(code), period: 30, remaining } })
            }
            R::Generate { options } => {
                let value = if options.passphrase {
                    "Lantern-Orbit-Cactus-Meadow-Quill7".to_owned()
                } else {
                    "vR8#qL2!mZ7@xT4$wN9&kP3s".chars().take(options.length.clamp(5, 24) as usize).collect()
                };
                Ok(Response::Secret { value: Secret::new(value) })
            }
            R::Sends => {
                if self.state.borrow().vault != VaultState::Unlocked {
                    return Err(IpcError::new(ErrorCode::Locked, "The vault is locked."));
                }
                let days = |n: i32| {
                    gtk::glib::DateTime::now_utc()
                        .and_then(|d| d.add_days(n))
                        .and_then(|d| d.format("%Y-%m-%dT%H:%M:%S.000Z"))
                        .map(|s| s.to_string())
                        .unwrap_or_default()
                };
                Ok(Response::List {
                    values: vec![
                        json!({"id": id(201), "name": "Wi-Fi for guests", "type": 0, "text": {"hidden": true},
                            "accessCount": 3, "deletionDate": days(6), "passwordSet": false, "disabled": false}),
                        json!({"id": id(202), "name": "Signed lease", "type": 1,
                            "file": {"fileName": "lease-2026.pdf", "sizeName": "1.4 MB"},
                            "accessCount": 1, "maxAccessCount": 2, "deletionDate": days(2), "passwordSet": true, "disabled": false}),
                        json!({"id": id(203), "name": "Staging database URL", "type": 0, "text": {"hidden": false},
                            "accessCount": 0, "deletionDate": days(13), "passwordSet": false, "disabled": true}),
                    ],
                })
            }
            R::CopySendLink { .. } => {
                self.emit(Event::Clipboard {
                    clipboard: ClipboardEvent {
                        state: ClipboardState::Copied,
                        label: "Send link copied".into(),
                        item_name: None,
                        clears_in: Some(30),
                        follow_up_in: None,
                    },
                });
                Ok(Response::Ok)
            }
            R::Settings => Ok(Response::Settings { settings: self.state.borrow().settings.clone() }),
            R::UpdateSettings { settings } => {
                self.state.borrow_mut().settings = settings.clone();
                Ok(Response::Settings { settings })
            }
            _ => Err(IpcError::new(ErrorCode::Invalid, "Not available in the demo.")),
        }
    }
}
