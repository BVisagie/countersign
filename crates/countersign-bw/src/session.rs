use std::fmt;

use secrecy::{ExposeSecret, SecretString};

/// The session key the official CLI uses to protect its unlocked user key.
///
/// It is never logged, and its memory is cleared when dropped.
pub struct SessionKey(SecretString);

impl SessionKey {
    /// Accepts the base64 key printed by `bw unlock` and returned by
    /// `bw serve`. It encodes 64 random bytes: 88 characters ending in `==`.
    pub fn from_raw(raw: String) -> Option<Self> {
        let valid = raw.len() == 88
            && raw.ends_with("==")
            && raw[..86].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/');
        let secret = SecretString::from(raw);
        valid.then_some(SessionKey(secret))
    }

    pub(crate) fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

impl Clone for SessionKey {
    fn clone(&self) -> Self {
        SessionKey(SecretString::from(self.expose().to_owned()))
    }
}

impl fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionKey(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::SessionKey;

    #[test]
    fn accepts_only_cli_session_keys() {
        let good = format!("{}==", "A".repeat(86));
        assert!(SessionKey::from_raw(good).is_some());
        assert!(SessionKey::from_raw("short==".into()).is_none());
        let bad = format!("{}==", "A\"".repeat(43));
        assert!(SessionKey::from_raw(bad).is_none());
        assert_eq!(
            format!("{:?}", SessionKey::from_raw(format!("{}==", "b".repeat(86))).unwrap()),
            "SessionKey(..)"
        );
    }
}
