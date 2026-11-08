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
    /// When auto-save was last switched on (ms): messages written before it are never read, so what was said while it was off stays unread.
    #[serde(default)]
    pub memory_auto_since: u64,
    /// Automatic skill proposals (off by default): after a long successful turn, a proposal waits for a person's approval.
    #[serde(default)]
    pub skills_auto: bool,
    #[serde(default)]
    pub skills_auto_since: u64,
    /// Scheduled tasks run only while this is on (off by default; the person switches it on in the Schedule tab).
    #[serde(default)]
    pub sched_on: bool,
    /// The container runtime (an absolute path to docker or podman) and the image a container chat runs in. Both are set by the person.
    #[serde(default)]
    pub container_runtime: Option<String>,
    #[serde(default)]
    pub container_image: Option<String>,
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
        let before = guard.memory_auto;
        let skills_before = guard.skills_auto;
        change(&mut guard);
        if !skills_before && guard.skills_auto {
            guard.skills_auto_since = crate::now_ms();
        }
        if before == Auto::Off && guard.memory_auto != Auto::Off {
            guard.memory_auto_since = crate::now_ms();
        }
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
        let fresh = SettingsStore::open(dir.path().join("other.json"));
        assert_eq!(fresh.get().memory_auto_since, 0);
        let on = fresh.update(|s| s.memory_auto = Auto::Project);
        assert!(on.memory_auto_since > 0, "switching on from off records the time");
        let again = fresh.update(|s| s.memory_auto = Auto::ProjectUser);
        assert_eq!(again.memory_auto_since, on.memory_auto_since, "changing between two on states does not move it");
        assert!(!fresh.get().skills_auto, "automatic skill proposals start off");
        let on_skills = fresh.update(|s| s.skills_auto = true);
        assert!(on_skills.skills_auto_since > 0, "switching skill proposals on records the time");
        let off = fresh.update(|s| s.memory_auto = Auto::Off);
        std::thread::sleep(std::time::Duration::from_millis(3));
        assert!(fresh.update(|s| s.memory_auto = Auto::Project).memory_auto_since >= off.memory_auto_since);
        assert_eq!(Auto::parse("all"), None);
    }
}
