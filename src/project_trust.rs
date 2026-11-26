//! What a person chose to trust in a project's own configuration (see `project_check`) for new chats of the same project, not only for the
//! chat the question was asked in. Only a decision made with "remember for this project" is recorded, only the hashes of the exact risky parts
//! that were shown, and a chat in the project never starts on a recorded one unless the files as they are now still give the same hashes (the
//! stored entry only lets the question be skipped; it is never read as what the files say). A declined question is never recorded.
//!
//! Filed per account (a configuration that sends the account's credentials somewhere is a risk to that account), in `<data>/project_trust.json`
//! (`0600`, no secrets: hashes only), at most 200 projects.
use crate::project_check::Scan;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

pub const MAX_PROJECTS: usize = 200;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// `Scan::hash` of what was trusted.
    #[serde(default)]
    pub hash: String,
    /// `Scan::strong` of the same; an entry without it never lets a question be skipped.
    #[serde(default)]
    pub strong: Option<String>,
    /// When it was last written (ms), to drop the project used longest ago.
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

/// The text a project is filed under for an account (None: the default account): the two are never mixed.
pub fn key(account: Option<&str>, project: &str) -> String {
    format!("{}\n{}", account.unwrap_or(""), project)
}

/// Whether a decision about this scan may be remembered for the project. Not when a configuration file is a link (what the hash covers is not
/// what the program reads), too big to be read (its "hash" is a size and a time, easy to forge) or not valid (the person was shown no
/// content). Danger is allowed: a person who chose to remember it was asked in stronger words.
pub fn remember_allowed(scan: &Scan) -> bool {
    scan.needs_decision() && !scan.findings.iter().any(|f| matches!(f.key, "link" | "too_big" | "malformed"))
}

pub struct TrustStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl TrustStore {
    /// A file that cannot be read is kept as `.broken` and the store starts empty.
    pub fn open(path: PathBuf) -> Self {
        let store = Self { path, lock: Mutex::new(()) };
        let _ = store.read_or_set_aside();
        store
    }

    /// The file as it is: Some(empty) when there is none or it is not this file (that one is first moved to `.broken`, so what is written
    /// next never destroys it); None when it exists but cannot be read now (nothing is written over it then).
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

    /// Whether a new chat in the project `key` may skip the question: something was recorded for it, and the files as they are now (`scan`,
    /// made at the start of this chat) give the same hashes, and a decision about them may be remembered at all.
    pub fn trusts(&self, key: &str, scan: &Scan) -> bool {
        if !remember_allowed(scan) {
            return false;
        }
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        self.load().projects.get(key).is_some_and(|e| !e.hash.is_empty() && e.hash == scan.hash && e.strong.as_deref() == Some(scan.strong.as_str()))
    }

    /// Records what was trusted for the project (nothing for a scan that may not be remembered).
    pub fn record(&self, key: &str, scan: &Scan) -> bool {
        self.record_at(key, scan, crate::now_ms())
    }

    fn record_at(&self, key: &str, scan: &Scan, used: u64) -> bool {
        if !remember_allowed(scan) {
            return false;
        }
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let Some(mut file) = self.read_or_set_aside() else { return false };
        file.v = 1;
        file.projects.insert(key.to_owned(), Entry { hash: scan.hash.clone(), strong: Some(scan.strong.clone()), used });
        while file.projects.len() > MAX_PROJECTS {
            let Some(oldest) = file.projects.iter().min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone()) else { break };
            file.projects.remove(&oldest);
        }
        serde_json::to_string_pretty(&file).is_ok_and(|text| crate::private::write_atomic(&self.path, &text).is_ok())
    }

    /// Forgets what was trusted for the project: new chats ask again (chats already made keep what they were told).
    pub fn forget(&self, key: &str) -> bool {
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let Some(mut file) = self.read_or_set_aside() else { return false };
        let had = file.projects.remove(key).is_some();
        if had {
            let _ = serde_json::to_string_pretty(&file).map(|text| crate::private::write_atomic(&self.path, &text));
        }
        had
    }

    /// Whether the project has a recorded trust (for the form to say so; not a decision to skip anything).
    pub fn has(&self, key: &str) -> bool {
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        self.load().projects.contains_key(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_check::{Finding, Severity};

    fn scan(key: &'static str, severity: Severity, material: &str) -> Scan {
        Scan { findings: vec![Finding { severity, file: ".claude/settings.json", key, text: String::new() }], hash: crate::worktree::fnv(material.as_bytes()), strong: crate::sha256::hex(material.as_bytes()) }
    }

    fn store() -> (tempfile::TempDir, TrustStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = TrustStore::open(dir.path().join("project_trust.json"));
        (dir, store)
    }

    #[test]
    fn what_was_trusted_holds_across_a_restart_in_a_private_file_and_only_for_the_same_hashes() {
        let (dir, store) = store();
        let shown = scan("hooks", Severity::Danger, "one");
        assert!(store.record("\n/p", &shown));
        let again = TrustStore::open(dir.path().join("project_trust.json"));
        assert!(again.trusts("\n/p", &shown));
        assert!(!again.trusts("\n/p", &scan("hooks", Severity::Danger, "two")), "any change of the risky parts asks again");
        assert!(!again.trusts("\n/other", &shown), "another project does not inherit it");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.path().join("project_trust.json")).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn a_link_a_file_too_big_or_not_valid_is_never_recorded_or_trusted() {
        let (_dir, store) = store();
        for key in ["link", "too_big", "malformed"] {
            let s = scan(key, Severity::Warn, key);
            assert!(!remember_allowed(&s), "{key}");
            assert!(!store.record("\n/p", &s), "{key}");
            assert!(!store.trusts("\n/p", &s), "{key}");
        }
        // Nothing risky at all: nothing to remember either.
        let calm = Scan { findings: vec![], hash: "h".into(), strong: "s".into() };
        assert!(!store.record("\n/p", &calm));
    }

    #[test]
    fn a_hand_written_entry_only_skips_the_question_when_the_files_as_they_are_match_it() {
        let (dir, _store) = store();
        let path = dir.path().join("project_trust.json");
        let current = scan("hooks", Severity::Danger, "now");
        let write = |entry: serde_json::Value| std::fs::write(&path, serde_json::json!({"v": 1, "projects": {"\n/p": entry}}).to_string()).unwrap();
        write(serde_json::json!({"hash": "0000", "strong": "ffff"}));
        assert!(!TrustStore::open(path.clone()).trusts("\n/p", &current), "wrong hashes");
        write(serde_json::json!({"hash": current.hash}));
        assert!(!TrustStore::open(path.clone()).trusts("\n/p", &current), "a weak hash alone is not enough");
        write(serde_json::json!({"hash": current.hash, "strong": "ffff"}));
        assert!(!TrustStore::open(path.clone()).trusts("\n/p", &current), "the strong one must agree too");
        write(serde_json::json!({"hash": "", "strong": ""}));
        assert!(!TrustStore::open(path.clone()).trusts("\n/p", &current));
        write(serde_json::json!({"hash": current.hash, "strong": current.strong}));
        assert!(TrustStore::open(path).trusts("\n/p", &current), "the same files: the question is skipped");
    }

    #[test]
    fn forgetting_removes_only_that_project_and_says_whether_there_was_one() {
        let (_dir, store) = store();
        let s = scan("hooks", Severity::Danger, "x");
        store.record("\n/a", &s);
        store.record("\n/b", &s);
        assert!(store.has("\n/a"));
        assert!(store.forget("\n/a"));
        assert!(!store.has("\n/a") && store.has("\n/b"));
        assert!(!store.forget("\n/a"), "nothing left to forget");
        assert!(!store.trusts("\n/a", &s));
    }

    #[test]
    fn the_account_is_part_of_the_key() {
        assert_ne!(key(Some("a1"), "/p"), key(None, "/p"));
        assert_ne!(key(Some("a1"), "/p"), key(Some("a2"), "/p"));
        assert_eq!(key(None, "/p"), "\n/p");
    }

    #[test]
    fn the_project_used_longest_ago_is_dropped_past_the_cap() {
        let (_dir, store) = store();
        let s = scan("hooks", Severity::Danger, "x");
        for i in 0..=MAX_PROJECTS as u64 {
            store.record_at(&format!("\n/p{i}"), &s, 1000 + i);
        }
        assert!(!store.has("\n/p0") && store.has("\n/p1") && store.has(&format!("\n/p{MAX_PROJECTS}")));
    }

    #[test]
    fn a_file_that_is_bad_at_open_or_goes_bad_later_is_kept_aside_and_never_written_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project_trust.json");
        std::fs::write(&path, "{ not json").unwrap();
        let store = TrustStore::open(path.clone());
        assert_eq!(std::fs::read_to_string(dir.path().join("project_trust.json.broken")).unwrap(), "{ not json");
        let s = scan("hooks", Severity::Danger, "x");
        assert!(store.record("\n/a", &s));
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        assert!(store.record("\n/b", &s), "the write goes on into a fresh file");
        assert_eq!(std::fs::read(dir.path().join("project_trust.json.broken")).unwrap(), vec![0xff, 0xfe, 0x00], "and the bad one is what was kept");
    }
}
