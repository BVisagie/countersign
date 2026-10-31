//! Detects Hyprland's session lock through its IPC socket.
//!
//! Hyprland reports no lock state directly. An active `ext-session-lock`
//! appears as `LOCK` in a monitor's `solitaryBlockedBy` list, the same signal
//! Omarchy's `omarchy-hyprland-session-locked` reads.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const TIMEOUT: Duration = Duration::from_secs(1);

fn socket() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let instance = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    Some(PathBuf::from(runtime).join("hypr").join(instance).join(".socket.sock"))
}

/// `Some(true)` when Hyprland holds a session lock, `Some(false)` when it
/// clearly does not, and `None` outside Hyprland or when undetermined.
pub async fn session_locked() -> Option<bool> {
    let path = socket()?;
    let work = async {
        let mut stream = UnixStream::connect(path).await.ok()?;
        stream.write_all(b"j/monitors").await.ok()?;
        let mut buf = Vec::new();
        stream.take(4 << 20).read_to_end(&mut buf).await.ok()?;
        Some(buf)
    };
    let buf = tokio::time::timeout(TIMEOUT, work).await.ok()??;
    parse_locked(&buf)
}

fn parse_locked(json: &[u8]) -> Option<bool> {
    let monitors: Vec<Value> = serde_json::from_slice(json).ok()?;
    let has = |m: &Value, reason: &str| {
        m["solitaryBlockedBy"].as_array().is_some_and(|b| b.iter().any(|r| r == reason))
    };
    if monitors.iter().any(|m| has(m, "LOCK")) {
        return Some(true);
    }
    // A monitor without a workspace stops at that reason before the lock
    // check, so only monitors with a workspace prove the session is open.
    if monitors.iter().any(|m| !has(m, "WORKSPACE")) {
        return Some(false);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse_locked;

    #[test]
    fn reads_lock_state() {
        assert_eq!(parse_locked(br#"[{"solitaryBlockedBy":["LOCK","WORKSPACE"]}]"#), Some(true));
        assert_eq!(parse_locked(br#"[{"solitaryBlockedBy":["CANDIDATE"]}]"#), Some(false));
        assert_eq!(parse_locked(br#"[{"name":"DP-1"}]"#), Some(false));
        assert_eq!(parse_locked(br#"[{"solitaryBlockedBy":["WORKSPACE"]}]"#), None);
        assert_eq!(parse_locked(b"not json"), None);
    }
}
