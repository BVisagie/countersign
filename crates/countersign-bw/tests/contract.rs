//! Contract tests against the real official CLI and a Bitwarden test account.
//!
//! Ignored by default. Run with:
//!
//! ```text
//! COUNTERSIGN_TEST_BW=/path/to/bw \
//! COUNTERSIGN_TEST_SERVER=https://vault.bitwarden.eu \
//! COUNTERSIGN_TEST_CLIENT_ID_FILE=... \
//! COUNTERSIGN_TEST_CLIENT_SECRET_FILE=... \
//! COUNTERSIGN_TEST_PASSWORD_FILE=... \
//! cargo test -p countersign-bw --test contract -- --ignored --nocapture
//! ```
//!
//! Use a throwaway account only. The test creates and permanently deletes
//! items, folders and Sends whose names start with `countersign-contract`.

use std::time::Instant;

use countersign_bw::{AuthStatus, BwError, Cli, GenerateOptions, ListOptions, Serve};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};

const PREFIX: &str = "countersign-contract";

fn secret_from(var: &str) -> SecretString {
    let path = std::env::var(var).unwrap_or_else(|_| panic!("{var} is not set"));
    let text = std::fs::read_to_string(path).expect("readable secret file");
    SecretString::from(text.trim_end_matches(['\r', '\n']).to_owned())
}

fn ms(start: Instant) -> u128 {
    start.elapsed().as_millis()
}

#[tokio::test]
#[ignore = "needs the official CLI and a throwaway Bitwarden account"]
async fn official_cli_contract() {
    let bw = std::env::var("COUNTERSIGN_TEST_BW").expect("COUNTERSIGN_TEST_BW");
    let server = std::env::var("COUNTERSIGN_TEST_SERVER").ok();
    let profile = tempfile::tempdir().unwrap();
    let cli = Cli::new(&bw, profile.path());

    let version = cli.version().await.expect("supported CLI");
    println!("CLI {version}");
    assert_eq!(
        cli.status().await.unwrap().status,
        AuthStatus::Unauthenticated
    );
    cli.config_server(server.as_deref()).await.unwrap();
    cli.login_api_key(
        &secret_from("COUNTERSIGN_TEST_CLIENT_ID_FILE"),
        &secret_from("COUNTERSIGN_TEST_CLIENT_SECRET_FILE"),
    )
    .await
    .expect("API key login");
    let password = secret_from("COUNTERSIGN_TEST_PASSWORD_FILE");

    let t = Instant::now();
    let serve = Serve::start(&cli, None).await.expect("serve starts");
    println!("serve start: {} ms", ms(t));
    let api = serve.client().clone();
    assert_eq!(api.status().await.unwrap().status, AuthStatus::Locked);
    assert!(matches!(
        api.list_items(&ListOptions::default()).await,
        Err(BwError::Locked)
    ));

    let wrong = SecretString::from("not the password".to_owned());
    assert!(matches!(api.unlock(&wrong).await, Err(BwError::Cli(_))));
    let t = Instant::now();
    let first_session = api.unlock(&password).await.expect("unlock");
    println!("unlock: {} ms", ms(t));
    api.sync().await.unwrap();

    // Remove anything a failed earlier run left behind.
    for item in api
        .list_items(&ListOptions {
            search: Some(PREFIX.into()),
            ..Default::default()
        })
        .await
        .unwrap()
    {
        api.delete_item(item["id"].as_str().unwrap(), true)
            .await
            .unwrap();
    }
    for item in api
        .list_items(&ListOptions {
            trash: true,
            ..Default::default()
        })
        .await
        .unwrap()
    {
        if item["name"]
            .as_str()
            .unwrap_or_default()
            .starts_with(PREFIX)
        {
            api.delete_item(item["id"].as_str().unwrap(), true)
                .await
                .unwrap();
        }
    }

    let t = Instant::now();
    let items = api.list_items(&ListOptions::default()).await.unwrap();
    println!("list {} items: {} ms", items.len(), ms(t));
    let t = Instant::now();
    let found = api
        .list_items(&ListOptions {
            search: Some("git".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    println!("search: {} results in {} ms", found.len(), ms(t));

    // Create, read and edit an item; fields Countersign does not touch must
    // survive the round trip.
    let created = api
        .create_item(&json!({
            "type": 1,
            "name": format!("{PREFIX} login"),
            "notes": "kept",
            "favorite": true,
            "folderId": null,
            "reprompt": 0,
            "fields": [{"name": "pin", "value": "4321", "type": 1}],
            "login": {
                "username": "contract@example.test",
                "password": "first-password",
                "totp": null,
                "uris": [{"uri": "https://contract.example.test", "match": null}]
            }
        }))
        .await
        .expect("create item");
    let id = created["id"].as_str().unwrap().to_owned();
    // `bw serve` can briefly answer "Not found." for an item it just
    // created; callers should use the returned item instead of re-reading.
    let t = Instant::now();
    let mut attempts = 0;
    let mut item = loop {
        attempts += 1;
        match api.get_item(&id).await {
            Ok(item) => break item,
            Err(e) if attempts < 50 => {
                if attempts == 1 {
                    println!("get after create: {e}");
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(e) => panic!("get after create: {e}"),
        }
    };
    println!(
        "get after create: ok after {attempts} attempts, {} ms",
        ms(t)
    );
    assert_eq!(item["login"]["password"], "first-password");
    item["name"] = Value::from(format!("{PREFIX} login edited"));
    item["login"]["password"] = Value::from("second-password");
    let edited = api.edit_item(&id, &item).await.expect("edit item");
    assert_eq!(edited["name"], format!("{PREFIX} login edited"));
    let reread = api.get_item(&id).await.unwrap();
    assert_eq!(reread["notes"], "kept");
    assert_eq!(reread["fields"][0]["value"], "4321");
    assert_eq!(
        reread["login"]["uris"][0]["uri"],
        "https://contract.example.test"
    );
    assert_eq!(reread["favorite"], true);
    println!("password history after edit: {}", reread["passwordHistory"]);

    // Re-unlocking checks the password. A wrong password leaves the vault
    // usable; a right one rotates the session key.
    assert!(api.unlock(&wrong).await.is_err());
    api.get_item(&id)
        .await
        .expect("still unlocked after a failed re-prompt");
    let second_session = api.unlock(&password).await.expect("re-prompt");
    api.get_item(&id)
        .await
        .expect("still unlocked after re-prompt");

    // Restart without locking: the current session key restores the vault,
    // and the rotated-out key does not.
    serve.shutdown().await;
    let t = Instant::now();
    let serve = Serve::start(&cli, Some(&second_session))
        .await
        .expect("restart");
    println!("restart with session: {} ms", ms(t));
    let api = serve.client().clone();
    assert_eq!(api.status().await.unwrap().status, AuthStatus::Unlocked);
    api.get_item(&id).await.expect("unlocked after restart");
    serve.shutdown().await;
    let stale = Serve::start(&cli, Some(&first_session))
        .await
        .expect("start with stale key");
    let stale_status = stale.client().status().await.unwrap().status;
    println!("status with rotated-out key: {stale_status:?}");
    assert_ne!(stale_status, AuthStatus::Unlocked);
    stale.shutdown().await;
    let serve = Serve::start(&cli, Some(&second_session)).await.unwrap();
    let api = serve.client().clone();

    let totp_item = api
        .list_items(&ListOptions::default())
        .await
        .unwrap()
        .into_iter()
        .find(|i| i["login"]["totp"].is_string())
        .map(|i| i["id"].as_str().unwrap().to_owned());
    match totp_item {
        None => println!("totp: no item with a TOTP key"),
        Some(totp_id) => match api.get_totp(&totp_id).await {
            Ok(code) => println!("totp available ({} digits)", code.expose_secret().len()),
            Err(BwError::Cli(message)) => println!("totp: {message}"),
            Err(e) => panic!("totp: {e}"),
        },
    }
    match api.get_totp(&id).await {
        Err(BwError::NotFound) => {}
        other => panic!("totp for an item without a key: {:?}", other.map(|_| ())),
    }

    match api
        .create_attachment(&id, "contract.txt", b"countersign attachment")
        .await
    {
        Ok(with_attachment) => {
            let attachment = with_attachment["attachments"][0]["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let data = api.get_attachment(&id, &attachment).await.unwrap();
            assert_eq!(&data[..], b"countersign attachment");
            api.delete_attachment(&id, &attachment).await.unwrap();
            println!("attachments: ok");
        }
        Err(BwError::Cli(message)) => println!("attachments: {message}"),
        Err(e) => panic!("attachment: {e}"),
    }

    // Trash, restore, then delete for good.
    api.delete_item(&id, false).await.unwrap();
    let trash = api
        .list_items(&ListOptions {
            trash: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(trash.iter().any(|i| i["id"] == id.as_str()));
    api.restore_item(&id).await.unwrap();
    api.get_item(&id).await.unwrap();
    api.delete_item(&id, true).await.unwrap();
    assert!(matches!(api.get_item(&id).await, Err(BwError::NotFound)));

    let folder = api
        .create_folder(&format!("{PREFIX} folder"))
        .await
        .unwrap();
    let folder_id = folder["id"].as_str().unwrap().to_owned();
    let renamed = api
        .edit_folder(&folder_id, &format!("{PREFIX} folder 2"))
        .await
        .unwrap();
    assert_eq!(renamed["name"], format!("{PREFIX} folder 2"));
    assert!(
        api.list_folders()
            .await
            .unwrap()
            .iter()
            .any(|f| f["id"] == folder_id.as_str())
    );
    api.delete_folder(&folder_id).await.unwrap();

    let deletion = time_in_days(3);
    let send = api
        .create_send(&json!({
            "name": format!("{PREFIX} send"),
            "notes": null,
            "type": 0,
            "text": {"text": "shared text", "hidden": false},
            "file": null,
            "maxAccessCount": null,
            "deletionDate": deletion,
            "expirationDate": null,
            "password": null,
            "disabled": false,
            "hideEmail": false
        }))
        .await
        .expect("create send");
    let send_id = send["id"].as_str().unwrap().to_owned();
    println!("send access url present: {}", send["accessUrl"].is_string());
    let mut send = api.get_send(&send_id).await.unwrap();
    send["name"] = Value::from(format!("{PREFIX} send edited"));
    let send = api.edit_send(&send_id, &send).await.expect("edit send");
    assert_eq!(send["name"], format!("{PREFIX} send edited"));
    assert!(
        api.list_sends()
            .await
            .unwrap()
            .iter()
            .any(|s| s["id"] == send_id.as_str())
    );
    api.delete_send(&send_id).await.unwrap();

    let password_options = GenerateOptions {
        length: Some(24),
        uppercase: true,
        lowercase: true,
        number: true,
        special: true,
        ..Default::default()
    };
    assert_eq!(
        api.generate(&password_options)
            .await
            .unwrap()
            .expose_secret()
            .chars()
            .count(),
        24
    );
    let phrase = GenerateOptions {
        passphrase: true,
        words: Some(5),
        separator: Some("-".into()),
        ..Default::default()
    };
    assert_eq!(
        api.generate(&phrase)
            .await
            .unwrap()
            .expose_secret()
            .split('-')
            .count(),
        5
    );

    let orgs = api.list_organizations().await.unwrap();
    let collections = api.list_collections().await.unwrap();
    println!(
        "organizations: {}, collections: {}",
        orgs.len(),
        collections.len()
    );

    api.lock().await.unwrap();
    assert!(matches!(
        api.list_items(&ListOptions::default()).await,
        Err(BwError::Locked)
    ));
    serve.shutdown().await;
    cli.logout().await.unwrap();
    assert_eq!(
        cli.status().await.unwrap().status,
        AuthStatus::Unauthenticated
    );
}

fn time_in_days(days: u64) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + days * 86_400;
    let (y, m, d, hh, mm, ss) = civil(secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.000Z")
}

/// Converts Unix seconds to a UTC civil date (Howard Hinnant's algorithm).
fn civil(secs: u64) -> (i64, u32, u32, u64, u64, u64) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, rem / 3_600, rem % 3_600 / 60, rem % 60)
}
