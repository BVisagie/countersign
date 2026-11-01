use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One value inside a vault item.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "field", rename_all = "snake_case")]
pub enum Field {
    Username,
    Password,
    Totp,
    Uri { index: usize },
    Notes,
    CardholderName,
    CardNumber,
    CardCode,
    CardExpiry,
    Identity { name: String },
    SshPrivateKey,
    SshPublicKey,
    SshFingerprint,
    Custom { index: usize },
    PasswordHistory { index: usize },
}

/// Identity values that are hidden until revealed.
const SECRET_IDENTITY: &[&str] = &["ssn", "passportNumber", "licenseNumber"];

impl Field {
    /// A short label for receipts and buttons.
    pub fn label(&self) -> String {
        match self {
            Field::Username => "Username".into(),
            Field::Password => "Password".into(),
            Field::Totp => "Verification code".into(),
            Field::Uri { .. } => "Website".into(),
            Field::Notes => "Notes".into(),
            Field::CardholderName => "Cardholder name".into(),
            Field::CardNumber => "Card number".into(),
            Field::CardCode => "Security code".into(),
            Field::CardExpiry => "Expiration".into(),
            Field::Identity { name } => identity_label(name).into(),
            Field::SshPrivateKey => "Private key".into(),
            Field::SshPublicKey => "Public key".into(),
            Field::SshFingerprint => "Fingerprint".into(),
            Field::Custom { .. } => "Custom field".into(),
            Field::PasswordHistory { .. } => "Old password".into(),
        }
    }

    /// Whether the value is hidden until the user reveals it, and kept off
    /// clipboard history when copied.
    pub fn is_secret_kind(&self, item: &Value) -> bool {
        match self {
            Field::Password
            | Field::Totp
            | Field::CardNumber
            | Field::CardCode
            | Field::SshPrivateKey
            | Field::PasswordHistory { .. } => true,
            Field::Identity { name } => SECRET_IDENTITY.contains(&name.as_str()),
            Field::Custom { index } => item["fields"][*index]["type"].as_i64() == Some(1),
            _ => false,
        }
    }

    /// Reads the value from a full item. The TOTP field returns the stored
    /// key, not a code.
    pub fn read(&self, item: &Value) -> Option<String> {
        let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        match self {
            Field::Username => text(&item["login"]["username"]),
            Field::Password => text(&item["login"]["password"]),
            Field::Totp => text(&item["login"]["totp"]),
            Field::Uri { index } => text(&item["login"]["uris"][*index]["uri"]),
            Field::Notes => text(&item["notes"]),
            Field::CardholderName => text(&item["card"]["cardholderName"]),
            Field::CardNumber => {
                text(&item["card"]["number"]).map(|n| n.chars().filter(|c| c.is_ascii_digit()).collect())
            }
            Field::CardCode => text(&item["card"]["code"]),
            Field::CardExpiry => {
                let month = text(&item["card"]["expMonth"]);
                let year = text(&item["card"]["expYear"]);
                match (month, year) {
                    (Some(m), Some(y)) => Some(format!("{:0>2}/{y}", m)),
                    (None, Some(y)) => Some(y),
                    (Some(m), None) => Some(format!("{:0>2}", m)),
                    (None, None) => None,
                }
            }
            Field::Identity { name } => text(&item["identity"][name.as_str()]),
            Field::SshPrivateKey => text(&item["sshKey"]["privateKey"]),
            Field::SshPublicKey => text(&item["sshKey"]["publicKey"]),
            Field::SshFingerprint => text(&item["sshKey"]["keyFingerprint"]),
            Field::Custom { index } => text(&item["fields"][*index]["value"]),
            Field::PasswordHistory { index } => text(&item["passwordHistory"][*index]["password"]),
        }
    }
}

fn identity_label(name: &str) -> &'static str {
    match name {
        "title" => "Title",
        "firstName" => "First name",
        "middleName" => "Middle name",
        "lastName" => "Last name",
        "username" => "Username",
        "company" => "Company",
        "ssn" => "Social security number",
        "passportNumber" => "Passport number",
        "licenseNumber" => "License number",
        "email" => "Email",
        "phone" => "Phone",
        "address1" | "address2" | "address3" => "Address",
        "city" => "City",
        "state" => "State or province",
        "postalCode" => "Postal code",
        "country" => "Country",
        _ => "Identity field",
    }
}

/// Returns a copy of `item` with every secret value replaced by `null`,
/// and the list of secret fields that had a value. Passkey private keys
/// are removed too; they are never shown.
pub fn redact(item: &Value) -> (Value, Vec<Field>) {
    let mut out = item.clone();
    let mut present = Vec::new();
    let mut clear = |field: Field, slot: Option<&mut Value>| {
        if let Some(slot) = slot
            && slot.as_str().is_some_and(|s| !s.is_empty())
        {
            *slot = Value::Null;
            present.push(field);
        }
    };
    clear(Field::Password, out.pointer_mut("/login/password"));
    clear(Field::Totp, out.pointer_mut("/login/totp"));
    clear(Field::CardNumber, out.pointer_mut("/card/number"));
    clear(Field::CardCode, out.pointer_mut("/card/code"));
    clear(Field::SshPrivateKey, out.pointer_mut("/sshKey/privateKey"));
    for name in SECRET_IDENTITY {
        clear(Field::Identity { name: (*name).to_owned() }, out.pointer_mut(&format!("/identity/{name}")));
    }
    let custom = item["fields"].as_array().map_or(0, Vec::len);
    for index in 0..custom {
        if item["fields"][index]["type"].as_i64() == Some(1) {
            clear(Field::Custom { index }, out.pointer_mut(&format!("/fields/{index}/value")));
        }
    }
    let history = item["passwordHistory"].as_array().map_or(0, Vec::len);
    for index in 0..history {
        clear(
            Field::PasswordHistory { index },
            out.pointer_mut(&format!("/passwordHistory/{index}/password")),
        );
    }
    if let Some(passkeys) = out.pointer_mut("/login/fido2Credentials").and_then(Value::as_array_mut) {
        for passkey in passkeys {
            if let Some(key) = passkey.get_mut("keyValue") {
                *key = Value::Null;
            }
        }
    }
    (out, present)
}

/// Hides the rest of an item that requires the master password: its notes
/// and custom text fields, which otherwise show in full. Call after
/// [`redact`] until the password has been entered for the item.
pub fn redact_protected(item: &mut Value, present: &mut Vec<Field>) {
    if let Some(notes) = item.get_mut("notes")
        && notes.as_str().is_some_and(|n| !n.is_empty())
    {
        *notes = Value::Null;
        present.push(Field::Notes);
    }
    if let Some(fields) = item.get_mut("fields").and_then(Value::as_array_mut) {
        for (index, field) in fields.iter_mut().enumerate() {
            if field["type"].as_i64() == Some(0)
                && let Some(value) = field.get_mut("value")
                && value.as_str().is_some_and(|v| !v.is_empty())
            {
                *value = Value::Null;
                present.push(Field::Custom { index });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn login() -> Value {
        json!({
            "id": "0b6a1c4e-1f2d-4c3b-9a8e-7d6c5b4a3f21",
            "type": 1,
            "name": "GitHub",
            "notes": "note",
            "login": {
                "username": "me",
                "password": "pw",
                "totp": "JBSWY3DPEHPK3PXP",
                "uris": [{"uri": "https://github.com/login"}],
                "fido2Credentials": [{"credentialId": "c", "keyValue": "private"}]
            },
            "fields": [
                {"name": "plain", "value": "visible", "type": 0},
                {"name": "pin", "value": "1234", "type": 1}
            ],
            "passwordHistory": [{"password": "old", "lastUsedDate": "2026-01-01T00:00:00Z"}]
        })
    }

    #[test]
    fn redaction_removes_every_secret() {
        let (out, present) = redact(&login());
        assert!(out["login"]["password"].is_null());
        assert!(out["login"]["totp"].is_null());
        assert!(out["fields"][1]["value"].is_null());
        assert_eq!(out["fields"][0]["value"], "visible");
        assert!(out["passwordHistory"][0]["password"].is_null());
        assert!(out["login"]["fido2Credentials"][0]["keyValue"].is_null());
        assert_eq!(out["login"]["username"], "me");
        assert_eq!(
            present,
            vec![
                Field::Password,
                Field::Totp,
                Field::Custom { index: 1 },
                Field::PasswordHistory { index: 0 }
            ]
        );
        let text = serde_json::to_string(&out).unwrap();
        for secret in ["\"pw\"", "JBSWY3", "1234", "\"old\"", "private"] {
            assert!(!text.contains(secret), "{secret} leaked");
        }
    }

    #[test]
    fn protected_items_hide_notes_and_text_fields() {
        let mut item = login();
        item["reprompt"] = json!(1);
        let (mut out, mut present) = redact(&item);
        redact_protected(&mut out, &mut present);
        assert!(out["notes"].is_null());
        assert!(out["fields"][0]["value"].is_null());
        assert!(present.contains(&Field::Notes));
        assert!(present.contains(&Field::Custom { index: 0 }));
        assert!(!serde_json::to_string(&out).unwrap().contains("visible"));
    }

    #[test]
    fn reads_fields() {
        let item = login();
        assert_eq!(Field::Password.read(&item).as_deref(), Some("pw"));
        assert_eq!(Field::Uri { index: 0 }.read(&item).as_deref(), Some("https://github.com/login"));
        assert!(Field::Custom { index: 1 }.is_secret_kind(&item));
        assert!(!Field::Custom { index: 0 }.is_secret_kind(&item));
        let card = json!({"card": {"number": "4111 1111-1111 1111", "expMonth": "3", "expYear": "2031"}});
        assert_eq!(Field::CardNumber.read(&card).as_deref(), Some("4111111111111111"));
        assert_eq!(Field::CardExpiry.read(&card).as_deref(), Some("03/2031"));
    }

    #[test]
    fn fields_serialize_with_tags() {
        let f = Field::Custom { index: 2 };
        assert_eq!(serde_json::to_string(&f).unwrap(), r#"{"field":"custom","index":2}"#);
        let back: Field = serde_json::from_str(r#"{"field":"password"}"#).unwrap();
        assert_eq!(back, Field::Password);
    }
}
