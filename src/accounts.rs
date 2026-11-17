//! Claude logins the person keeps apart (for example a personal one and a work one).
//!
//! The default account is the login the `claude` program already has (its own `~/.claude`): it has no entry here and a chat of it carries
//! no account. Every other account is an entry of `<data>/accounts.json` (no secret in it) and owns two folders made from its id:
//! `<data>/accounts/<id>` is the `CLAUDE_CONFIG_DIR` its chats run with (the login is made there by the person, with `claude auth login`;
//! this app never reads or stores a token), and `<data>/stores/<id>` holds its notes, skills and history index (see `realm`).
//! The two are apart on purpose: the program cleans its own folder, and a logout must not take the app's data with it.
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::claude::env::EnvPlan;

pub const MAX_ACCOUNTS: usize = 8;
const NAME_MAX: usize = 40;
/// `claude auth status` reads a file; a run that takes longer than this is something else.
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
const STATUS_MAX_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub created: u64,
    pub updated: u64,
}

/// What a chat keeps of its account: the id never changes, the name is what it was called when the chat was made.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountRef {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize)]
struct File {
    v: u32,
    accounts: Vec<Account>,
}

/// Sixteen lower-case hex digits: the form `crate::random_hex(8)` makes. Anything else is never turned into a path.
pub fn valid_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn validate_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("the account needs a name".into());
    }
    if name.chars().count() > NAME_MAX {
        return Err(format!("the name is longer than {NAME_MAX} characters"));
    }
    if name.chars().any(char::is_control) {
        return Err("the name has a control character".into());
    }
    Ok(name.to_owned())
}

/// The `CLAUDE_CONFIG_DIR` of an account's chats.
pub fn config_dir(data: &Path, id: &str) -> PathBuf {
    data.join("accounts").join(id)
}

/// Where an account's notes, skills and history index live.
pub fn stores_dir(data: &Path, id: &str) -> PathBuf {
    data.join("stores").join(id)
}

/// The folder the default account's `claude` uses: what `CLAUDE_CONFIG_DIR` says, else `~/.claude`.
pub fn default_config_dir(env: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    env.map(Path::to_path_buf).or_else(|| home.map(|h| h.join(".claude")))
}

/// Looks at a folder this app made, every time it is about to be used: a real folder (not a link) of this user, private, whose
/// parent is `parent`, and which is neither the default account's folder nor inside it. Nothing is repaired: a wrong mode or owner
/// is refused, because someone else may have touched it. Returns the canonical path.
///
/// Windows: there are no owner or mode bits to look at. The folder is as private as the data folder, which the app requires to be
/// inside the user's profile (see `platform::data_dir_in_profile`); the link, parent and default-folder checks stay.
pub fn check_dir(path: &Path, parent: &Path, default_dir: Option<&Path>) -> Result<PathBuf, String> {
    const UNSAFE: &str = "this account's folder is missing or not safe to use";
    let meta = std::fs::symlink_metadata(path).map_err(|_| UNSAFE.to_owned())?;
    if !meta.file_type().is_dir() {
        return Err(UNSAFE.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: geteuid only reads the process's own id.
        let uid = unsafe { libc::geteuid() };
        if meta.uid() != uid || meta.permissions().mode() & 0o777 != 0o700 {
            return Err(UNSAFE.into());
        }
    }
    let real = path.canonicalize().map_err(|_| UNSAFE.to_owned())?;
    let real_parent = parent.canonicalize().map_err(|_| UNSAFE.to_owned())?;
    if real.parent() != Some(real_parent.as_path()) {
        return Err(UNSAFE.into());
    }
    if let Some(default) = default_dir.and_then(|d| d.canonicalize().ok()) {
        if real.starts_with(&default) {
            return Err("this account's folder is the default account's folder".into());
        }
    }
    Ok(real)
}

/// Makes the two folders of an account (private), under parents that are private too.
pub fn make_dirs(data: &Path, id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err("not an account id".into());
    }
    for (parent, leaf) in [(data.join("accounts"), config_dir(data, id)), (data.join("stores"), stores_dir(data, id))] {
        crate::private::create_dir(&parent).and_then(|()| crate::private::create_dir(&leaf)).map_err(|e| format!("could not make the account's folders: {e}"))?;
    }
    Ok(())
}

pub struct AccountStore {
    path: PathBuf,
    data: PathBuf,
    inner: Mutex<Vec<Account>>,
}

impl AccountStore {
    /// An unreadable file opens as an empty list and is left where it is (never overwritten by opening); the notice says so. An entry
    /// whose id is not an id this app makes is dropped, so a hand-written file cannot name a path.
    pub fn open(data: &Path) -> (Self, Option<String>) {
        let path = data.join("accounts.json");
        let (mut list, notice) = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<File>(&text) {
                Ok(file) => (file.accounts, None),
                Err(_) => (Vec::new(), Some("accounts.json could not be read; no extra account is available until it is fixed or removed".to_owned())),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None),
            Err(_) => (Vec::new(), Some("accounts.json could not be read".to_owned())),
        };
        let mut seen = std::collections::HashSet::new();
        list.retain(|a| valid_id(&a.id) && seen.insert(a.id.clone()) && validate_name(&a.name).is_ok());
        list.truncate(MAX_ACCOUNTS);
        (Self { path, data: data.to_path_buf(), inner: Mutex::new(list) }, notice)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Account>> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn list(&self) -> Vec<Account> {
        self.lock().clone()
    }

    pub fn get(&self, id: &str) -> Option<Account> {
        self.lock().iter().find(|a| a.id == id).cloned()
    }

    fn write(&self, list: &[Account]) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&File { v: 1, accounts: list.to_vec() }).map_err(|e| e.to_string())?;
        crate::private::write(&self.path, &text).map_err(|e| format!("could not save: {e}"))
    }

    /// Adds an account and makes its folders. Nothing is kept when the file cannot be written.
    pub fn add(&self, name: &str, now: u64) -> Result<Account, String> {
        let name = validate_name(name)?;
        let mut guard = self.lock();
        if guard.len() >= MAX_ACCOUNTS {
            return Err("there are too many accounts".into());
        }
        if guard.iter().any(|a| a.name == name) {
            return Err("another account has this name".into());
        }
        let account = Account { id: crate::random_hex(8), name, created: now, updated: now };
        make_dirs(&self.data, &account.id)?;
        let mut list = guard.clone();
        list.push(account.clone());
        if let Err(error) = self.write(&list) {
            // The folders are new and empty: nothing of the person's is in them.
            let _ = std::fs::remove_dir(config_dir(&self.data, &account.id));
            let _ = std::fs::remove_dir(stores_dir(&self.data, &account.id));
            return Err(error);
        }
        *guard = list;
        Ok(account)
    }

    /// Only the name changes: the id, the folders and the chats bound to it stay.
    pub fn rename(&self, id: &str, name: &str, now: u64) -> Result<(), String> {
        let name = validate_name(name)?;
        let mut guard = self.lock();
        let mut list = guard.clone();
        if list.iter().any(|a| a.name == name && a.id != id) {
            return Err("another account has this name".into());
        }
        let account = list.iter_mut().find(|a| a.id == id).ok_or("that account is gone")?;
        (account.name, account.updated) = (name, now);
        self.write(&list)?;
        *guard = list;
        Ok(())
    }

    /// Takes the entry out of the file (its folders are the caller's to remove). Ok(false) when there was none.
    pub fn remove_entry(&self, id: &str) -> Result<bool, String> {
        let mut guard = self.lock();
        let mut list = guard.clone();
        list.retain(|a| a.id != id);
        if list.len() == guard.len() {
            return Ok(false);
        }
        self.write(&list)?;
        *guard = list;
        Ok(true)
    }
}

/// What `claude auth status` said. The rest of its output is dropped: it is not kept or logged.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Status {
    pub logged_in: bool,
    pub email: Option<String>,
    pub org: Option<String>,
    pub subscription: Option<String>,
    /// The folder the program says it used: it must be the one asked for, else something redirected it.
    pub config_dir: Option<String>,
}

/// Reads the JSON `claude auth status` prints. The text of a field that is not a string is ignored.
pub fn parse_status(bytes: &[u8]) -> Result<Status, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| "the status was not JSON".to_owned())?;
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(|s| s.chars().filter(|c| !c.is_control()).take(200).collect::<String>());
    Ok(Status {
        logged_in: value.get("loggedIn").and_then(|v| v.as_bool()).ok_or("the status has no loggedIn")?,
        email: text("email"),
        org: text("orgName"),
        subscription: text("subscriptionType"),
        config_dir: text("configDirectory"),
    })
}

/// Why `status` gave no answer. A redirect is a reason to refuse; a failure to ask is not (the CLI itself still has no login then).
#[derive(Debug, PartialEq)]
pub enum StatusError {
    /// The program reported another settings folder than the one asked for.
    Redirected,
    Failed(String),
}

impl StatusError {
    pub fn message(&self) -> String {
        match self {
            Self::Redirected => "the program used another settings folder than this account's".to_owned(),
            Self::Failed(text) => text.clone(),
        }
    }
}

/// Runs `claude auth <args>` for one config folder and returns what it printed (at most 64 KB). It starts in that folder (a project's
/// own settings are not read), with the credential variables of the account plan applied, and is ended after a time limit.
async fn run_auth(bin: &str, dir: &Path, env: &EnvPlan, args: [&str; 2]) -> Result<(Vec<u8>, std::process::ExitStatus), String> {
    let mut command = Command::new(bin);
    command.args(args).current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
        crate::proc_group::configure(&mut command);
    for name in &env.remove {
        command.env_remove(name);
    }
    for (name, value) in &env.set {
        command.env(name, value);
    }
    let mut child = command.spawn().map_err(|e| format!("could not start {bin}: {e}"))?;
    let group = match child.id() {
        Some(pid) => match crate::proc_group::adopt(pid) {
            Some(group) => Some(group),
            None => {
                let _ = child.start_kill();
                return Err("could not put the program in a group it can be ended with".into());
            }
        },
        None => None,
    };
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let work = async move {
        let mut bytes = Vec::new();
        (&mut stdout).take(STATUS_MAX_BYTES).read_to_end(&mut bytes).await.map_err(|e| e.to_string())?;
        let status = child.wait().await.map_err(|e| e.to_string())?;
        Ok::<_, String>((bytes, status))
    };
    match tokio::time::timeout(STATUS_TIMEOUT, work).await {
        Ok(bytes) => bytes,
        Err(_) => {
            if let Some(group) = &group {
                group.kill();
            }
            Err("the program did not answer in time".into())
        }
    }
}

/// Asks `claude auth status` about one config folder. `expect` is the canonical folder it must report when one is given; a different
/// one means a variable redirected it, and the answer is refused.
pub async fn status(bin: &str, dir: &Path, env: &EnvPlan, expect: Option<&Path>) -> Result<Status, StatusError> {
    // The exit status is not looked at here: a logged-out login may end with a code other than 0 and still say so in its answer.
    let (bytes, _) = run_auth(bin, dir, env, ["auth", "status"]).await.map_err(StatusError::Failed)?;
    let status = parse_status(&bytes).map_err(StatusError::Failed)?;
    if let Some(expect) = expect {
        let said = status.config_dir.as_deref().map(PathBuf::from).and_then(|p| p.canonicalize().ok());
        if said.as_deref() != Some(expect) {
            return Err(StatusError::Redirected);
        }
    }
    Ok(status)
}

/// Best effort: asks the program to drop the login kept in this folder before the folder is removed.
pub async fn logout(bin: &str, dir: &Path, env: &EnvPlan) -> Result<(), String> {
    // A program that ends with a failure did not drop the login: the caller reports it (the login may stay outside the folder).
    let (_, status) = run_auth(bin, dir, env, ["auth", "logout"]).await?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the program ended with {status}"))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn data() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        crate::private::create_dir(dir.path()).unwrap();
        dir
    }

    #[test]
    fn an_id_is_sixteen_lower_case_hex_digits_and_nothing_else() {
        assert!(valid_id("00ff00ff00ff00ff"));
        for bad in ["", "00ff", "00FF00FF00FF00FF", "../0000000000000", "00ff00ff00ff00f/", "00ff00ff00ff00fg", "00ff00ff00ff00ff0"] {
            assert!(!valid_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn names_are_trimmed_short_and_free_of_control_characters() {
        assert_eq!(validate_name("  Work ").unwrap(), "Work");
        assert!(validate_name("  ").is_err());
        assert!(validate_name(&"x".repeat(41)).is_err());
        assert!(validate_name("a\nb").is_err());
        assert!(validate_name(&"가".repeat(40)).is_ok(), "the limit counts characters");
    }

    #[test]
    fn adding_makes_private_folders_apart_from_each_other_and_a_file_without_secrets() {
        let dir = data();
        let (store, notice) = AccountStore::open(dir.path());
        assert!(notice.is_none() && store.list().is_empty());
        let account = store.add("Work", 5).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        for p in [config_dir(dir.path(), &account.id), stores_dir(dir.path(), &account.id), dir.path().join("accounts"), dir.path().join("stores")] {
            assert_eq!(mode(&p), 0o700, "{p:?}");
        }
        assert_eq!(mode(&dir.path().join("accounts.json")), 0o600);
        assert_ne!(config_dir(dir.path(), &account.id), stores_dir(dir.path(), &account.id));
        let (again, _) = AccountStore::open(dir.path());
        assert_eq!(again.list(), vec![account.clone()]);
        assert!(store.add("Work", 6).is_err(), "a name is used once");
        store.rename(&account.id, "Company", 7).unwrap();
        assert_eq!(AccountStore::open(dir.path()).0.get(&account.id).unwrap().name, "Company");
        assert_eq!(AccountStore::open(dir.path()).0.get(&account.id).unwrap().id, account.id, "the id does not change");
    }

    #[test]
    fn a_hand_written_file_cannot_name_a_path_or_repeat_an_id() {
        let dir = data();
        let good = "0011223344556677";
        let text = format!(
            r#"{{"v":1,"accounts":[{{"id":"../etc","name":"x","created":0,"updated":0}},{{"id":"{good}","name":"A","created":0,"updated":0}},{{"id":"{good}","name":"B","created":0,"updated":0}},{{"id":"8899aabbccddeeff","name":"","created":0,"updated":0}}]}}"#
        );
        std::fs::write(dir.path().join("accounts.json"), text).unwrap();
        let (store, notice) = AccountStore::open(dir.path());
        assert!(notice.is_none());
        let ids: Vec<String> = store.list().into_iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![good.to_owned()]);
    }

    #[test]
    fn an_unreadable_file_is_left_alone_and_reported() {
        let dir = data();
        std::fs::write(dir.path().join("accounts.json"), "not json").unwrap();
        let (store, notice) = AccountStore::open(dir.path());
        assert!(notice.is_some() && store.list().is_empty());
        assert_eq!(std::fs::read_to_string(dir.path().join("accounts.json")).unwrap(), "not json", "opening does not overwrite");
    }

    #[test]
    fn a_failed_save_keeps_nothing_and_leaves_no_folder_behind() {
        let dir = data();
        let (store, _) = AccountStore::open(dir.path());
        // A directory where the file goes: the write fails.
        std::fs::create_dir(dir.path().join("accounts.json")).unwrap();
        assert!(store.add("Work", 1).is_err());
        assert!(store.list().is_empty());
        assert_eq!(std::fs::read_dir(dir.path().join("accounts")).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(dir.path().join("stores")).unwrap().count(), 0);
    }

    #[test]
    fn the_folder_is_looked_at_again_every_time_and_a_wrong_one_is_refused() {
        let dir = data();
        let (store, _) = AccountStore::open(dir.path());
        let a = store.add("Work", 1).unwrap();
        let parent = dir.path().join("accounts");
        let path = config_dir(dir.path(), &a.id);
        let real = check_dir(&path, &parent, None).unwrap();
        assert_eq!(real, path.canonicalize().unwrap());
        // Not private.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(check_dir(&path, &parent, None).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Missing.
        assert!(check_dir(&parent.join("ffffffffffffffff"), &parent, None).is_err());
        // A link in its place, even to a private folder.
        let elsewhere = dir.path().join("elsewhere");
        crate::private::create_dir(&elsewhere).unwrap();
        let link = parent.join("1111111111111111");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        assert!(check_dir(&link, &parent, None).is_err());
        // The wrong parent.
        assert!(check_dir(&path, &dir.path().join("stores"), None).is_err());
        // The default account's folder, or one inside it.
        assert!(check_dir(&path, &parent, Some(&path)).is_err());
        assert!(check_dir(&path, &parent, Some(&parent)).is_err(), "a default folder that holds it is refused too");
        assert!(check_dir(&path, &parent, Some(&dir.path().join("stores"))).is_ok(), "a default folder elsewhere is fine");
    }

    #[test]
    fn the_default_folder_is_the_variable_else_the_home_one() {
        assert_eq!(default_config_dir(Some(Path::new("/x")), Some(Path::new("/h"))), Some(PathBuf::from("/x")));
        assert_eq!(default_config_dir(None, Some(Path::new("/h"))), Some(PathBuf::from("/h/.claude")));
        assert_eq!(default_config_dir(None, None), None);
    }

    #[test]
    fn the_status_is_read_for_five_fields_and_nothing_else() {
        let text = br#"{"loggedIn":true,"email":"a@b.c","orgName":"Org","subscriptionType":"max","configDirectory":"/c","apiProvider":"firstParty","loginExpiresAt":"x","secret":"s"}"#;
        let status = parse_status(text).unwrap();
        assert_eq!(status, Status { logged_in: true, email: Some("a@b.c".into()), org: Some("Org".into()), subscription: Some("max".into()), config_dir: Some("/c".into()) });
        assert!(!serde_json::to_string(&status).unwrap().contains("secret"));
        assert!(!parse_status(br#"{"loggedIn":false}"#).unwrap().logged_in);
        assert!(parse_status(b"nope").is_err());
        assert!(parse_status(br#"{"email":"a"}"#).is_err(), "no loggedIn is not an answer");
        assert_eq!(parse_status(br#"{"loggedIn":true,"email":"a\nb"}"#).unwrap().email.as_deref(), Some("ab"));
    }
    #[tokio::test]
    async fn a_logout_that_ends_with_a_failure_is_an_error() {
        let dir = data();
        let script = |name: &str, body: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.display().to_string()
        };
        let env = EnvPlan::default();
        assert!(logout(&script("ok", "exit 0"), dir.path(), &env).await.is_ok());
        let failed = logout(&script("bad", "exit 3"), dir.path(), &env).await;
        assert!(failed.is_err(), "{failed:?}");
        // A status that ends with a failure still gives its answer.
        let answer = status(&script("st", "echo '{\"loggedIn\":false}'; exit 1"), dir.path(), &env, None).await;
        assert!(answer.is_ok(), "{answer:?}");
    }
}
