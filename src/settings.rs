//! The few switches the person sets in the app, kept in `<data>/settings.json` (0600).
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// What the app may save by itself from the person's own messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Auto {
    /// Nothing is saved unless the person (or a tool call they approve) saves it.
    #[default]
    Off,
    /// Notes about this project only.
    Project,
    /// Notes about this project and about the person.
    ProjectUser,
}

impl Auto {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "off" => Some(Self::Off),
            "project" => Some(Self::Project),
            "project_user" => Some(Self::ProjectUser),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub memory_auto: Auto,
    /// The person has seen the one-time card that explains auto-save.
    #[serde(default)]
    pub memory_card_seen: bool,
}

pub struct SettingsStore {
    path: PathBuf,
    inner: Mutex<Settings>,
}

impl SettingsStore {
    pub fn open(path: PathBuf) -> Self {
        let inner = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        Self { path, inner: Mutex::new(inner) }
    }

    pub fn get(&self) -> Settings {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn update(&self, change: impl FnOnce(&mut Settings)) -> Settings {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        change(&mut guard);
        if let Ok(text) = serde_json::to_string_pretty(&*guard) {
            let _ = crate::private::write(&self.path, &text);
        }
        guard.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_off_and_a_change_survives_a_restart_privately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::open(path.clone());
        assert_eq!(store.get().memory_auto, Auto::Off);
        store.update(|s| s.memory_auto = Auto::Project);
        assert_eq!(SettingsStore::open(path.clone()).get().memory_auto, Auto::Project);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(SettingsStore::open(path).get().memory_auto, Auto::Off, "an unreadable file means the default, which is off");
        assert_eq!(Auto::parse("project_user"), Some(Auto::ProjectUser));
        assert_eq!(Auto::parse("all"), None);
    }
}
