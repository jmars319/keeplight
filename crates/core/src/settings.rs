//! Keeplight's own preferences, apart from the setup of kept devices:
//! `settings.json` beside the setup file. A missing or unreadable file means
//! the defaults; it holds nothing that can't be chosen again in a click.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Show the keeper's icon in the notification area. Off by default (owner
    /// decision 2026-11-13); failures are still told by a notification.
    pub tray_icon: bool,
    /// "Let AI tools make changes": while off (the default), AI tools can
    /// read and plan through the MCP server, but every change is refused
    /// with `ai.changes_off`. Turned on only from the window's Settings.
    #[serde(rename = "aiChanges")]
    pub ai_changes: bool,
    /// Don't ask the release feed whether a newer Keeplight is out. Off, so
    /// the window does check, by default; it only ever tells you.
    #[serde(rename = "noUpdateCheck")]
    pub no_update_check: bool,
}

impl Settings {
    pub fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        text.push('\n');
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_bad_files_are_the_defaults_and_saves_round_trip() {
        let dir = std::env::temp_dir().join(format!("keeplight-test-{}-settings", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default());
        Settings { tray_icon: true, ai_changes: true, no_update_check: false }.save(&path).unwrap();
        assert!(Settings::load(&path).tray_icon);
        assert!(std::fs::read_to_string(&path).unwrap().contains("\"aiChanges\": true"), "the fleet's name");
        std::fs::write(&path, "{ nope").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        std::fs::write(&path, "{}").unwrap();
        assert!(!Settings::load(&path).tray_icon, "fields added later default sensibly");
        assert!(!Settings::load(&path).ai_changes, "AI changes are off by default");
        assert!(!Settings::load(&path).no_update_check, "the update check is on by default");
    }
}
