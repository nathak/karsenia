//! What the new-chat form remembers for each project: the model, the permission mode, the account and whether the chat works in its own
//! worktree. Filed under the project's folder (see `Meta::project_key`), in `<data>/project_defaults.json` (`0600`, no secrets: an account is
//! its id only). Written when a chat is made from the form, read when the form learns which folder it is for.
//!
//! What is never remembered: skipping every check (`bypassPermissions`: a person always chooses it, and confirms it, again), and a chat on a
//! provider or in a container (a chat is not put on a third party by what was chosen last time).
use crate::claude::perm::PermMode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// How many projects are kept; past this the one used longest ago goes.
pub const MAX_PROJECTS: usize = 200;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// A model name `clean_model` accepts; None is the default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// An account id; None is the default account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// A permission mode by its name (a text, so one name this build does not know cannot spoil the whole file); never the one that skips checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub worktree: bool,
    /// When this was last written (ms), to drop the project used longest ago.
    #[serde(default)]
    pub used: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    v: u32,
    #[serde(default)]
    projects: BTreeMap<String, Entry>,
}

/// The text a folder is filed under: canonical, without the `\\?\` Windows puts before a drive path. None for a folder that is not there.
pub fn key_of(path: &Path) -> Option<String> {
    let real = path.canonicalize().ok()?;
    Some(crate::platform::strip_verbatim(real.to_str()?).to_owned())
}

/// A permission mode that may be remembered and filled in: one this build knows, and not the one that skips every check.
fn safe_mode(name: &str) -> Option<&'static str> {
    PermMode::parse(name).filter(|m| *m != PermMode::Bypass).map(PermMode::name)
}

impl Entry {
    fn cleaned(mut self) -> Entry {
        self.permission = self.permission.as_deref().and_then(safe_mode).map(str::to_owned);
        self.model = self.model.as_deref().and_then(crate::registry::clean_model);
        self
    }
}

/// What the new-chat form is told about a folder: the remembered entry cleaned for use now. The JSON has `found`; `account` is left out when it
/// is not known to be there any more (the form keeps its own), `null` when it is the default account; `permission` is left out when none
/// may be filled in (the form keeps the default, and never the mode that skips checks).
pub fn form_json(cwd: &str, entry: Option<Entry>, account_usable: impl Fn(&str) -> bool, trusted: bool) -> serde_json::Value {
    let Some(entry) = entry else { return with_trust(serde_json::json!({"cwd": cwd, "found": false}), trusted) };
    let mut out = serde_json::json!({"cwd": cwd, "found": true, "model": entry.model, "worktree": entry.worktree});
    match entry.account.as_deref() {
        None => out["account"] = serde_json::Value::Null,
        Some(id) if account_usable(id) => out["account"] = serde_json::json!(id),
        Some(_) => {}
    }
    if let Some(mode) = entry.permission {
        out["permission"] = serde_json::json!(mode);
    }
    with_trust(out, trusted)
}

/// `trusted: true` when what the person trusted in the project's own configuration is remembered (the form offers to forget it); left out otherwise.
fn with_trust(mut out: serde_json::Value, trusted: bool) -> serde_json::Value {
    if trusted {
        out["trusted"] = serde_json::json!(true);
    }
    out
}

pub struct DefaultsStore {
    path: PathBuf,
    /// One writer at a time in this program; the file is read again under it, so what another program using the same data folder wrote
    /// earlier is kept (two writes at the very same moment can still lose one of them: only a project's latest settings).
    lock: Mutex<()>,
}

impl DefaultsStore {
    /// A file that cannot be read is kept as `.broken` (the next write must not be what destroys it) and the store starts empty.
    pub fn open(path: PathBuf) -> Self {
        let store = Self { path, lock: Mutex::new(()) };
        let _ = store.read_or_set_aside();
        store
    }

    /// The file as it is: Some(empty) when there is none or it cannot be read as this file (a file that is there but is not this file's text, or
    /// not text at all, is first moved to `.broken`, so what is written next never destroys it). None when it exists but cannot be read now
    /// (a permission or disk error): nothing is written over it then.
    fn read_or_set_aside(&self) -> Option<File> {
        match std::fs::read(&self.path) {
            Ok(bytes) => match serde_json::from_slice::<File>(&bytes) {
                Ok(file) => Some(file),
                Err(_) => {
                    let _ = std::fs::rename(&self.path, self.path.with_extension("json.broken"));
                    Some(File::default())
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(File::default()),
            Err(_) => None,
        }
    }

    fn load(&self) -> File {
        std::fs::read(&self.path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    /// What the project `key` was last made with, as it may be used now (see `safe_mode`, `clean_model`). None for a project never seen.
    pub fn get(&self, key: &str) -> Option<Entry> {
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        self.load().projects.get(key).cloned().map(Entry::cleaned)
    }

    pub fn record(&self, key: &str, entry: Entry) {
        self.record_at(key, entry, crate::now_ms());
    }

    fn record_at(&self, key: &str, entry: Entry, used: u64) {
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let Some(mut file) = self.read_or_set_aside() else { return };
        file.v = 1;
        file.projects.insert(key.to_owned(), Entry { used, ..entry.cleaned() });
        while file.projects.len() > MAX_PROJECTS {
            let Some(oldest) = file.projects.iter().min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone()) else { break };
            file.projects.remove(&oldest);
        }
        if let Ok(text) = serde_json::to_string_pretty(&file) {
            let _ = crate::private::write_atomic(&self.path, &text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, DefaultsStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = DefaultsStore::open(dir.path().join("project_defaults.json"));
        (dir, store)
    }

    fn entry(model: &str, permission: &str) -> Entry {
        Entry { model: Some(model.into()), permission: Some(permission.into()), ..Default::default() }
    }

    #[test]
    fn the_form_is_told_the_account_only_while_it_is_usable_and_a_mode_only_when_one_may_be_filled_in() {
        use serde_json::json;
        let e = Entry { account: Some("a1".into()), ..entry("haiku", "plan") }.cleaned();
        assert_eq!(form_json("/p", Some(e.clone()), |id| id == "a1", false), json!({"cwd": "/p", "found": true, "model": "haiku", "worktree": false, "account": "a1", "permission": "plan"}));
        let gone = form_json("/p", Some(e), |_| false, false);
        assert!(gone.get("account").is_none(), "an account that is gone is not named: the form keeps its own: {gone}");
        let plain = form_json("/p", Some(Entry::default()), |_| false, false);
        assert_eq!(plain, json!({"cwd": "/p", "found": true, "model": null, "worktree": false, "account": null}), "the default account is null; no mode is left out");
        assert_eq!(form_json("/p", None, |_| true, false), json!({"cwd": "/p", "found": false}));
        assert_eq!(form_json("/p", None, |_| true, true), json!({"cwd": "/p", "found": false, "trusted": true}), "a project can be trusted without ever having been made from the form");
        assert_eq!(form_json("/p", Some(Entry::default()), |_| false, true)["trusted"], true);
    }

    #[test]
    fn what_a_project_was_made_with_is_kept_across_a_restart_in_a_private_file() {
        let (dir, store) = store();
        store.record("/p", Entry { account: Some("a1".into()), worktree: true, ..entry("haiku", "acceptEdits") });
        let again = DefaultsStore::open(dir.path().join("project_defaults.json"));
        let got = again.get("/p").unwrap();
        assert_eq!((got.model.as_deref(), got.permission.as_deref(), got.account.as_deref(), got.worktree), (Some("haiku"), Some("acceptEdits"), Some("a1"), true));
        assert!(got.used > 0);
        assert!(again.get("/other").is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("project_defaults.json")).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the file is the person's alone");
        }
    }

    #[test]
    fn a_folder_reached_by_a_link_or_with_a_trailing_slash_is_the_same_project() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let plain = key_of(&real).unwrap();
        assert_eq!(key_of(&dir.path().join("real").join(".")).unwrap(), plain);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real, dir.path().join("link")).unwrap();
            assert_eq!(key_of(&dir.path().join("link")).unwrap(), plain, "a link is the folder it points to");
        }
        assert!(key_of(&dir.path().join("missing")).is_none());
        assert!(!plain.starts_with(r"\\?\"));
    }

    #[test]
    fn skipping_every_check_is_never_remembered_and_wipes_what_was_there() {
        let (_dir, store) = store();
        store.record("/p", entry("opus", "acceptEdits"));
        store.record("/p", entry("opus", "bypassPermissions"));
        let got = store.get("/p").unwrap();
        assert_eq!(got.permission, None, "the earlier acceptEdits does not come back either");
        assert_eq!(got.model.as_deref(), Some("opus"), "the rest is kept");
    }

    #[test]
    fn a_hand_written_bypass_or_unknown_mode_or_model_is_not_given_back_and_the_other_projects_survive() {
        let (dir, _store) = store();
        let path = dir.path().join("project_defaults.json");
        std::fs::write(&path, r#"{"v":1,"projects":{"/a":{"permission":"bypassPermissions","model":"haiku"},"/b":{"permission":"yolo"},"/c":{"permission":"plan","model":"bad model; rm"}}}"#).unwrap();
        let store = DefaultsStore::open(path);
        let a = store.get("/a").unwrap();
        assert_eq!((a.permission, a.model.as_deref()), (None, Some("haiku")));
        assert_eq!(store.get("/b").unwrap().permission, None);
        let c = store.get("/c").unwrap();
        assert_eq!((c.permission.as_deref(), c.model), (Some("plan"), None));
        let _ = store;
    }

    #[test]
    fn the_project_used_longest_ago_is_dropped_past_the_cap() {
        let (_dir, store) = store();
        for i in 0..=MAX_PROJECTS as u64 {
            store.record_at(&format!("/p{i}"), entry("haiku", "plan"), 1000 + i);
        }
        assert!(store.get("/p0").is_none(), "the oldest went");
        assert!(store.get("/p1").is_some() && store.get(&format!("/p{MAX_PROJECTS}")).is_some());
        assert_eq!(store.load().projects.len(), MAX_PROJECTS);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_kept_aside_and_the_store_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project_defaults.json");
        std::fs::write(&path, "{ not json").unwrap();
        let store = DefaultsStore::open(path.clone());
        assert!(store.get("/p").is_none());
        assert_eq!(std::fs::read_to_string(dir.path().join("project_defaults.json.broken")).unwrap(), "{ not json");
        store.record("/p", entry("haiku", "plan"));
        assert!(DefaultsStore::open(path).get("/p").is_some());
    }

    #[test]
    fn a_file_that_goes_bad_while_running_is_set_aside_before_the_next_write_and_never_written_over() {
        let (dir, store) = store();
        let path = dir.path().join("project_defaults.json");
        store.record("/a", entry("haiku", "plan"));
        // Not text at all, and a text that is not what this file holds: both are kept as they were.
        std::fs::write(&path, [0xff, 0xfe, 0x00, 0x41]).unwrap();
        store.record("/b", entry("opus", "plan"));
        assert_eq!(std::fs::read(dir.path().join("project_defaults.json.broken")).unwrap(), vec![0xff, 0xfe, 0x00, 0x41], "the bad file is what was kept");
        assert!(store.get("/b").is_some(), "and the write went on into a fresh file");
        std::fs::write(&path, "{ \"projects\": [1, 2] }").unwrap();
        store.record("/c", entry("opus", "plan"));
        assert_eq!(std::fs::read_to_string(dir.path().join("project_defaults.json.broken")).unwrap(), "{ \"projects\": [1, 2] }");
    }

    #[test]
    fn an_old_file_with_fields_missing_still_reads() {
        let (dir, _store) = store();
        let path = dir.path().join("project_defaults.json");
        std::fs::write(&path, r#"{"projects":{"/p":{"model":"haiku"}}}"#).unwrap();
        let got = DefaultsStore::open(path).get("/p").unwrap();
        assert_eq!((got.model.as_deref(), got.permission, got.account, got.worktree), (Some("haiku"), None, None, false));
    }

    #[test]
    fn a_second_program_writing_the_same_file_is_not_written_over() {
        let (dir, one) = store();
        let two = DefaultsStore::open(dir.path().join("project_defaults.json"));
        one.record("/a", entry("haiku", "plan"));
        two.record("/b", entry("opus", "plan"));
        assert!(one.get("/a").is_some() && one.get("/b").is_some(), "each sees what the other wrote");
    }
}
