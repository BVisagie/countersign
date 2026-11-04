//! A contract test of the daemon against the real official CLI and a
//! Bitwarden test account: after each change, the vault index the app shows
//! must already have it, however far behind the CLI's lists are.
//!
//! Ignored by default. Run with the variables the CLI contract test uses
//! (see `crates/countersign-bw/tests/contract.rs`):
//!
//! ```text
//! COUNTERSIGN_TEST_BW=… COUNTERSIGN_TEST_SERVER=… COUNTERSIGN_TEST_CLIENT_ID_FILE=… \
//! COUNTERSIGN_TEST_CLIENT_SECRET_FILE=… COUNTERSIGN_TEST_PASSWORD_FILE=… \
//! cargo test -p countersign-daemon contract -- --ignored --nocapture
//! ```
//!
//! Use a throwaway account only. The test creates and permanently deletes
//! items and folders whose names start with `countersign-contract`.

use std::sync::Arc;

use countersign_ipc::{Request, Response, Secret, Settings, VaultSnapshot};
use serde_json::{Value, json};

use crate::core::Daemon;
use crate::paths::Paths;

const PREFIX: &str = "countersign-contract";
const ROUNDS: usize = 4;

fn secret_from(var: &str) -> Secret {
    let path = std::env::var(var).unwrap_or_else(|_| panic!("{var} is not set"));
    let text = std::fs::read_to_string(path).expect("readable secret file");
    Secret::new(text.trim_end_matches(['\r', '\n']))
}

async fn ask(daemon: &Arc<Daemon>, request: Request) -> Response {
    daemon.handle(request).await.unwrap_or_else(|e| panic!("{e}"))
}

fn id_of(response: Response) -> String {
    match response {
        Response::Item { item, .. } | Response::Json { value: item } => {
            item["id"].as_str().unwrap().to_owned()
        }
        _ => panic!("no object in the answer"),
    }
}

/// Checks one thing the index must show, and counts it.
fn check(failures: &mut Vec<String>, what: &str, ok: bool) {
    println!("{what}: {}", if ok { "shown" } else { "MISSING" });
    if !ok {
        failures.push(what.to_owned());
    }
}

fn item<'a>(vault: &'a VaultSnapshot, id: &str) -> Option<&'a countersign_ipc::ItemSummary> {
    vault.items.iter().find(|i| i.id == id)
}

#[tokio::test]
#[ignore]
async fn the_index_shows_every_change_at_once() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths {
        settings_file: root.path().join("settings.toml"),
        profile_dir: root.path().join("profile"),
        managed_cli_root: root.path().join("cli"),
        runtime_dir: root.path().join("run"),
        socket: root.path().join("run/daemon.sock"),
        lock_file: root.path().join("run/daemon.lock"),
    };
    let bw = std::env::var("COUNTERSIGN_TEST_BW").expect("COUNTERSIGN_TEST_BW");
    let settings = Settings { cli_path: Some(bw.into()), ..Settings::default() };
    let daemon = Daemon::new(paths, settings);
    daemon.refresh().await;
    ask(&daemon, Request::SetServer { url: std::env::var("COUNTERSIGN_TEST_SERVER").ok() }).await;
    ask(
        &daemon,
        Request::LoginApiKey {
            client_id: secret_from("COUNTERSIGN_TEST_CLIENT_ID_FILE"),
            client_secret: secret_from("COUNTERSIGN_TEST_CLIENT_SECRET_FILE"),
        },
    )
    .await;
    // Unlocking also schedules the sync that follows it, which runs during
    // the changes below.
    ask(&daemon, Request::Unlock { password: secret_from("COUNTERSIGN_TEST_PASSWORD_FILE") }).await;

    let client = daemon.client().await.unwrap();
    let leftovers = daemon.vault();
    for old in leftovers.items.iter().chain(&leftovers.trash).filter(|i| i.name.starts_with(PREFIX)) {
        ask(&daemon, Request::DeleteItem { id: old.id.clone(), permanent: true }).await;
    }
    for old in leftovers.folders.iter().filter(|f| f.name.starts_with(PREFIX)) {
        ask(&daemon, Request::DeleteFolder { id: old.id.clone() }).await;
    }

    let mut failures = Vec::new();
    for round in 0..ROUNDS {
        let note = json!({
            "type": 2, "name": format!("{PREFIX} {round}"), "notes": null, "favorite": false,
            "folderId": null, "reprompt": 0, "secureNote": {"type": 0}
        });
        let id = id_of(ask(&daemon, Request::CreateItem { item: note }).await);
        check(&mut failures, "a new item", item(&daemon.vault(), &id).is_some());

        let renamed = format!("{PREFIX} {round} edited");
        let mut full = daemon.read_item(&client, &id).await.unwrap();
        full["name"] = Value::from(renamed.clone());
        ask(&daemon, Request::EditItem { id: id.clone(), item: full }).await;
        check(&mut failures, "an edited name", item(&daemon.vault(), &id).is_some_and(|i| i.name == renamed));

        ask(&daemon, Request::SetFavorite { id: id.clone(), favorite: true }).await;
        check(&mut failures, "a favorite", item(&daemon.vault(), &id).is_some_and(|i| i.favorite));

        let folder = id_of(ask(&daemon, Request::CreateFolder { name: format!("{PREFIX} {round}") }).await);
        check(&mut failures, "a new folder", daemon.vault().folders.iter().any(|f| f.id == folder));
        let folder_name = format!("{PREFIX} {round} renamed");
        ask(&daemon, Request::RenameFolder { id: folder.clone(), name: folder_name.clone() }).await;
        check(
            &mut failures,
            "a renamed folder",
            daemon.vault().folders.iter().any(|f| f.id == folder && f.name == folder_name),
        );

        let mut full = daemon.read_item(&client, &id).await.unwrap();
        full["folderId"] = Value::from(folder.clone());
        ask(&daemon, Request::EditItem { id: id.clone(), item: full }).await;
        check(
            &mut failures,
            "an item moved to a folder",
            item(&daemon.vault(), &id).is_some_and(|i| i.folder_id.as_deref() == Some(&folder)),
        );
        ask(&daemon, Request::DeleteFolder { id: folder.clone() }).await;
        let vault = daemon.vault();
        check(&mut failures, "a deleted folder", !vault.folders.iter().any(|f| f.id == folder));
        check(
            &mut failures,
            "its item in no folder",
            item(&vault, &id).is_some_and(|i| i.folder_id.is_none()),
        );

        ask(&daemon, Request::DeleteItem { id: id.clone(), permanent: false }).await;
        let vault = daemon.vault();
        check(
            &mut failures,
            "an item in the trash",
            item(&vault, &id).is_none() && vault.trash.iter().any(|i| i.id == id),
        );
        ask(&daemon, Request::RestoreItem { id: id.clone() }).await;
        let vault = daemon.vault();
        check(
            &mut failures,
            "a restored item",
            item(&vault, &id).is_some() && !vault.trash.iter().any(|i| i.id == id),
        );
        ask(&daemon, Request::DeleteItem { id: id.clone(), permanent: true }).await;
        let vault = daemon.vault();
        check(
            &mut failures,
            "an item deleted for good",
            item(&vault, &id).is_none() && !vault.trash.iter().any(|i| i.id == id),
        );
    }

    ask(&daemon, Request::Logout).await;
    assert!(failures.is_empty(), "the index missed: {failures:?}");
}
