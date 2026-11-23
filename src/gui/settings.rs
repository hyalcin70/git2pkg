//! Persistente, kleine GUI-Einstellungen (`~/.config/git2pkg/settings.json`).

use crate::gui::i18n::ThemeMode;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuiSettings {
    pub output_dir: Option<PathBuf>,
    pub theme: Option<ThemeMode>,
    /// Zuletzt genutzter Ordner für Build-Logs (manuell oder automatisch).
    pub last_log_dir: Option<PathBuf>,
}

impl GuiSettings {
    pub fn config_path() -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        home.join(".config/git2pkg/settings.json")
    }

    pub fn load_from(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|content| serde_json::from_str::<Self>(&content).ok())
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> bool {
        if let Some(parent) = path.parent() {
            if fs::create_dir_all(parent).is_err() {
                return false;
            }
        }
        serde_json::to_string_pretty(self)
            .ok()
            .and_then(|json| fs::write(path, json).ok())
            .is_some()
    }

    pub fn load() -> Self {
        Self::load_from(&Self::config_path())
    }

    pub fn save(&self) {
        let _ = self.save_to(&Self::config_path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "git2pkg-settings-{}-{}.json",
            name,
            std::process::id()
        ))
    }

    #[test]
    fn settings_speichern_und_laden() {
        let path = test_path("roundtrip");
        let settings = GuiSettings {
            output_dir: Some(PathBuf::from("/tmp/example-output")),
            theme: Some(ThemeMode::Dark),
            last_log_dir: Some(PathBuf::from("/tmp/example-logs")),
        };
        assert!(settings.save_to(&path));
        assert_eq!(GuiSettings::load_from(&path), settings);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn settings_fallback_bei_fehlender_oder_defekter_datei() {
        let path = test_path("fallback");
        let _ = fs::remove_file(&path);
        assert_eq!(GuiSettings::load_from(&path), GuiSettings::default());

        fs::write(&path, "{ das ist kein json").unwrap();
        assert_eq!(GuiSettings::load_from(&path), GuiSettings::default());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn settings_leere_datei_nutzt_defaults() {
        let path = test_path("empty");
        fs::write(&path, "").unwrap();
        assert_eq!(GuiSettings::load_from(&path), GuiSettings::default());
        let _ = fs::remove_file(path);
    }
}
