use std::path::Path;

use anyhow::Result;
use countersign_ipc::Settings;

use crate::paths::write_private;

/// Reads settings, falling back to defaults for a missing or unreadable file.
pub fn load(path: &Path) -> Settings {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str::<Settings>(&text) {
            Ok(settings) => settings.normalized(),
            Err(e) => {
                tracing::warn!("ignoring unreadable settings at {}: {e}", path.display());
                Settings::default()
            }
        },
        Err(_) => Settings::default(),
    }
}

pub fn save(path: &Path, settings: &Settings) -> Result<()> {
    let text = toml::to_string_pretty(settings)?;
    write_private(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        assert_eq!(load(&path), Settings::default());
        let custom = Settings { clipboard_clear_secs: 45, lock_after_idle_mins: 0, ..Settings::default() };
        save(&path, &custom).unwrap();
        assert_eq!(load(&path), custom);
        std::fs::write(&path, "clipboard_clear_secs = \"x\"").unwrap();
        assert_eq!(load(&path), Settings::default());
    }
}
