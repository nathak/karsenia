//! The sessions of this app: created here, kept on disk (a meta file and a log each), listed for the sidebar.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use crate::accounts::{AccountRef, AccountStore};
use crate::claude::env::EnvPlan;
use crate::claude::perm::PermMode;
use crate::claude::process::{claude_bin, is_safe_cwd};
use crate::extract::ExtractConfig;
use crate::mcp::McpGate;
use crate::propose::ProposeConfig;
use crate::providers::ProviderStore;
use crate::random_hex;
use crate::realm::{Realm, Shared};
use crate::sched_store::SchedStore;
use crate::schedule::SystemClock;
use crate::scheduler::{SchedConfig, Scheduler};
use crate::settings::SettingsStore;
use crate::session::{self, Envelope, Info, Meta, SessionHandle, Setup};

/// A model name as `--model` may be given it: letters, digits and `-_.:/[]` (an alias, `opus[1m]` or a full model id), at most 100
/// characters. "default" and a blank name are no name: Claude Code's own default.
pub fn clean_model(name: &str) -> Option<String> {
    let name = name.trim();
    (name != "default" && !name.is_empty() && name.len() <= 100 && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/[]".contains(c))).then(|| name.to_owned())
}

/// What a permission mode may be combined with, as the key of the sentence that says why not (None: allowed). Every mode may be used
/// in every kind of chat, a provider chat included (the person chose that provider, and skipping the checks is asked about first).
/// Only Claude Code's own refusal is repeated here: it will not skip permission checks as root, so it is refused before a chat is made.
#[cfg_attr(not(unix), allow(unused_variables))]
pub fn mode_refusal(mode: PermMode, _provider: bool, _container: bool) -> Option<&'static str> {
    #[cfg(unix)]
    {
        // SAFETY: geteuid only reads the process's own id.
        if mode == PermMode::Bypass && unsafe { libc::geteuid() } == 0 {
            return Some("perm_err_bypass_root");
        }
    }
    None
}

/// See `mode_refusal`; the reason in English.
pub fn check_mode(mode: PermMode, provider: bool, container: bool) -> Result<(), String> {
    match mode_refusal(mode, provider, container) {
        None => Ok(()),
        Some(_) => Err("Claude Code refuses to skip permission checks when it runs as root".into()),
    }
}

/// The most named chats whose `claude` is started when the app starts.
const MAX_WARM: usize = 12;

/// What a chat carried to another account keeps of the old one.
struct Carry {
    model: Option<String>,
    permission_mode: PermMode,
    named: bool,
    source: &'static str,
    handed: Option<session::Handed>,
}

pub struct Registry {
    dir: PathBuf,
    bin: String,
    gate: Option<Arc<McpGate>>,
    settings: Arc<SettingsStore>,
    /// What the new-chat form starts with for each project (see `project_defaults`).
    defaults: Arc<crate::project_defaults::DefaultsStore>,
    /// Messages every page should hear (a note changed, an auto-save happened): `{v, kind, data}`.
    pub events: tokio::sync::broadcast::Sender<serde_json::Value>,
    /// The default account's notes, skills and history, and one realm for every extra account. `realm()` is the only way to one:
    /// it never answers with the default account's when an extra account's is asked for.
    default_realm: Arc<Realm>,
    realms: RwLock<HashMap<String, Arc<Realm>>>,
    shared: Shared,
    accounts: Arc<AccountStore>,
    /// Set when the accounts file could not be read, or an account's folders were not safe to open.
    pub accounts_notice: Option<String>,
    /// Set when the schedule file could not be read and was set aside.
    pub sched_notice: Option<String>,
    sched: Option<(Arc<SchedStore>, Arc<Scheduler>)>,
    providers: Arc<ProviderStore>,
    /// One random tag per run of the app (see `container::container_name`).
    instance: String,
    /// Set when the providers file could not be read.
    pub providers_notice: Option<String>,
    sessions: Arc<Mutex<HashMap<String, SessionHandle>>>,
}

pub fn data_dir() -> PathBuf {
    let named = std::env::var_os("TENREC_DATA").or_else(|| std::env::var_os("SWB_APP_DATA")).map(PathBuf::from);
    let home = crate::platform::home_dir().unwrap_or_else(|| PathBuf::from("."));
    data_dir_in(named, &home)
}

fn data_dir_in(named: Option<PathBuf>, home: &Path) -> PathBuf {
    if let Some(dir) = named {
        return dir;
    }
    let share = home.join(".local").join("share");
    let (new, old) = (share.join("tenrec"), share.join("smart-workbench-app"));
    // A folder the app kept under its first name is used where it is: moving it would change the hashes made from its path.
    if !new.exists() && old.is_dir() {
        return old;
    }
    new
}

/// The folder a session of the `claude` CLI was kept for, checked for an import: an absolute, existing folder that is not `/` or the
/// home folder, and whose canonical form makes the same CLI folder name (the CLI finds a session only from the folder it was made in,
/// so a link to the project will not do). Returns the canonical path.
pub fn import_folder(cwd: &str) -> Result<PathBuf, String> {
    let path = Path::new(cwd);
    if !path.is_absolute() {
        return Err("the folder must be an absolute path".into());
    }
    if !is_safe_cwd(path) {
        return Err("that folder does not exist, or is / or the home folder".into());
    }
    let real = crate::platform::canon(path).map_err(|e| e.to_string())?;
    if crate::cli_sessions::encode_dir(&real.to_string_lossy()) != crate::cli_sessions::encode_dir(cwd) {
        return Err("that folder is not where the session was kept".into());
    }
    Ok(real)
}

impl Registry {
    /// Opens the data folder and starts a task for every session found in it (none has a process until its next message).
    pub fn open(dir: PathBuf, bin: Option<String>) -> anyhow::Result<Self> {
        Self::open_with(dir, bin, None)
    }

    /// `mcp_exe` is the program `claude` runs as this app's MCP server (`<exe> mcp`); None leaves history search off.
    pub fn open_with(dir: PathBuf, bin: Option<String>, mcp_exe: Option<PathBuf>) -> anyhow::Result<Self> {
        Self::open_full(dir, bin, mcp_exe, ExtractConfig::default(), ProposeConfig::default(), SchedConfig::default())
    }

    /// As `open_with`, with the auto-save timing given (tests use short waits). Needs a running tokio runtime.
    pub fn open_full(dir: PathBuf, bin: Option<String>, mcp_exe: Option<PathBuf>, extract: ExtractConfig, propose: ProposeConfig, sched: SchedConfig) -> anyhow::Result<Self> {
        crate::private::create_dir(&dir)?;
        if cfg!(windows) {
            crate::worktree::set_no_hooks_dir(dir.join("run").join("no-hooks"))?;
        }
        let gate = mcp_exe.and_then(|exe| {
            let sock = socket_path(&dir)?;
            match McpGate::new(sock, exe, dir.join("run")) {
                Ok(gate) => Some(Arc::new(gate)),
                Err(error) => {
                    tracing::warn!("the MCP gate is off: {error:#}");
                    None
                }
            }
        });
        // Notes and skills handed to a chat live in files only while its process does: any left by a crash are removed.
        if let Ok(entries) = std::fs::read_dir(dir.join("run")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.ends_with(".mem.txt") {
                    let _ = std::fs::remove_file(entry.path());
                } else if name.ends_with(".skills") {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        let settings = Arc::new(SettingsStore::open(dir.join("settings.json")));
        let instance = random_hex(3);
        // Containers an earlier run of this app left (it was killed): removed, in the background.
        if let Ok(runtime) = crate::claude::container::validate_runtime(settings.get().container_runtime.as_deref().unwrap_or_default()) {
            let hash = crate::claude::container::data_hash(&dir);
            let tag = instance.clone();
            std::thread::spawn(move || crate::claude::container::sweep(&runtime, &hash, &tag));
        }
        let (events, _) = tokio::sync::broadcast::channel(64);
        let sessions: Arc<Mutex<HashMap<String, SessionHandle>>> = Arc::new(Mutex::new(HashMap::new()));
        let bin = bin.unwrap_or_else(claude_bin);
        // Windows: a program with an extension must be an .exe (a `.cmd` or `.bat` goes through the command interpreter's quoting).
        if std::path::Path::new(&bin).extension().is_some() && !crate::platform::runnable_claude(std::path::Path::new(&bin)) {
            anyhow::bail!("the claude program must be a .exe on Windows: {bin}");
        }
        let usage = Arc::new(crate::usage::UsageStore::open(&dir.join("usage.json"), events.clone()));
        let costs = Arc::new(crate::costs::CostStore::open(&dir.join("costs.json"), events.clone()));
        let shared = Shared { bin: bin.clone(), settings: settings.clone(), sessions: sessions.clone(), events: events.clone(), extract, propose, usage, costs };
        // The default account's files are the ones the app always had, in the data folder itself.
        let default_realm = Realm::open(None, &dir, EnvPlan::default(), &shared);
        let (accounts, mut accounts_notice) = AccountStore::open(&dir);
        let accounts = Arc::new(accounts);
        // The numbers of an account that an earlier run removed are not kept.
        shared.usage.retain(&accounts.list().into_iter().map(|a| a.id).collect());
        let mut realms = HashMap::new();
        for account in accounts.list() {
            if !account.kind.has_realm() {
                // A Codex account has no notes, skills or history of its own here: only its folder is looked at.
                if let Err(error) = crate::accounts::check_dir(&crate::accounts::config_dir(&dir, &account.id), &dir.join("accounts"), crate::accounts::default_home_of(account.kind).as_deref()) {
                    tracing::warn!("the account {} is off: {error}", account.name);
                    accounts_notice = Some(format!("the account \"{}\" is off: {error}", account.name));
                }
                continue;
            }
            match open_account_realm(&dir, &account.id, &shared) {
                Ok(realm) => {
                    realms.insert(account.id.clone(), realm);
                }
                Err(error) => {
                    tracing::warn!("the account {} is off: {error}", account.name);
                    accounts_notice = Some(format!("the account \"{}\" is off: {error}", account.name));
                }
            }
        }
        let (sched, sched_notice) = match SchedStore::open(&dir.join("schedule.sqlite")) {
            Ok((store, notice)) => {
                let store = Arc::new(store);
                let scheduler = Scheduler::new(store.clone(), settings.clone(), events.clone(), bin.clone(), dir.clone(), Arc::new(SystemClock), sched);
                scheduler.spawn_loop();
                (Some((store, scheduler)), notice)
            }
            Err(error) => {
                tracing::warn!("scheduled tasks are off: {error:#}");
                (None, Some(format!("scheduled tasks are off: {error:#}")))
            }
        };
        let (providers, providers_notice) = ProviderStore::open(dir.join("providers.json"));
        let defaults = Arc::new(crate::project_defaults::DefaultsStore::open(dir.join("project_defaults.json")));
        let registry = Self { dir: dir.clone(), bin, gate, settings, defaults, events, default_realm, realms: RwLock::new(realms), shared, accounts, accounts_notice, sched_notice, sched, providers: Arc::new(providers), providers_notice, instance, sessions };
        let mut metas: Vec<(String, Option<String>, bool)> = Vec::new();
        for entry in std::fs::read_dir(&dir)?.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            let Some(sid) = name.strip_suffix(".meta.json") else { continue };
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let Ok(meta) = serde_json::from_str::<Meta>(&text) else { continue };
            if meta.sid != sid {
                continue;
            }
            let log = read_log(&session::log_path(&dir, sid));
            // The chat's own account's realm, or none (a provider chat; an account that is not available). A chat is never given another one.
            let realm = registry.realm_of_meta(&meta);
            metas.push((sid.to_owned(), meta.account.as_ref().map(|a| a.id.clone()), meta.provider.as_ref().is_some_and(|p| !p.login)));
            // The index is brought up to date before the session can emit anything of its own.
            if let Some(index) = realm.as_ref().and_then(|r| r.index.as_ref()) {
                index.catch_up(sid, &meta.cwd.to_string_lossy(), meta.created, log.clone());
            }
            let handle = session::spawn(Setup { costs: Some(registry.shared.costs.clone()), meta, bin: registry.bin.clone(), dir: dir.clone(), log, realm, mcp: registry.gate.clone(), providers: Some(registry.providers.clone()), settings: Some(registry.settings.clone()), instance: registry.instance.clone() });
            registry.sessions.lock().expect("registry lock").insert(sid.to_owned(), handle);
        }
        // Each realm's index keeps only its own account's chats: an entry of another account that got there is removed.
        for realm in registry.all_realms() {
            if let Some(index) = &realm.index {
                index.keep_only(metas.iter().filter(|(_, account, provider)| !provider && *account == realm.account).map(|(sid, _, _)| sid.clone()).collect());
            }
        }
        Ok(registry)
    }

    /// What a permission mode may be combined with, checked when the chat is made (and by the host before it is asked for): see
    /// the free function of the same name.
    pub fn check_mode(mode: PermMode, provider: bool, container: bool) -> Result<(), String> {
        check_mode(mode, provider, container)
    }

    /// Starts the `claude` of the chats that have a name (see `Meta::cli_name`), one after the other and at most `MAX_WARM` of them, the
    /// ones a person named first and each group newest first: a chat's name is the address other Claude Code sessions find it by, and
    /// that exists only while its process runs. Nothing is sent to a model. A chat that is left out here starts with its next message.
    pub async fn warm_named(&self) {
        let mut named: Vec<Info> = self.list().await.into_iter().filter(|i| i.meta.cli_name().is_some() && i.meta.provider.is_none() && !i.meta.container).collect();
        named.sort_by_key(|i| (!i.meta.named, std::cmp::Reverse(i.meta.created)));
        if named.len() > MAX_WARM {
            tracing::info!("{} chats have a name; the first {MAX_WARM} are started, the others start with their next message", named.len());
        }
        for info in named.into_iter().take(MAX_WARM) {
            if let Some(handle) = self.get(&info.meta.sid) {
                if let Err(error) = handle.warm().await {
                    tracing::warn!("could not start a named chat: {error}");
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// The chat's workbench (its contract, pins and guard settings): a new one when it has none.
    pub fn workbench_get(&self, sid: &str) -> Result<crate::workbench::Workbench, &'static str> {
        self.get(sid).ok_or("wb_err_gone")?;
        crate::workbench::store::load(&self.dir, sid)
    }

    /// Changes the chat's workbench and saves it; the new state. One change at a time (a read and a write must not interleave).
    pub fn workbench_apply(&self, sid: &str, op: crate::workbench::Op) -> Result<crate::workbench::Workbench, &'static str> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _held = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        self.get(sid).ok_or("wb_err_gone")?;
        let mut wb = crate::workbench::store::load(&self.dir, sid)?;
        wb.apply(op)?;
        crate::workbench::store::save(&self.dir, sid, &wb)?;
        Ok(wb)
    }

    /// A change of the chat's team settings (team mode, the lead's effort, a helper's model or effort): saved in its workbench, then the
    /// chat is told so a running idle `claude` starts again with them. A provider chat that does not use an account's login has no team.
    pub async fn team_apply(&self, sid: &str, op: crate::workbench::Op) -> Result<crate::workbench::Workbench, &'static str> {
        self.team_apply_many(sid, vec![op]).await
    }

    /// Several team changes at once (a card's save button): each is applied in turn, and the chat is told once, at the end, so a running
    /// idle `claude` is ended once and the transcript says the team as it now is. The first change that is refused stops the rest.
    pub async fn team_apply_many(&self, sid: &str, ops: Vec<crate::workbench::Op>) -> Result<crate::workbench::Workbench, &'static str> {
        use crate::workbench::Op;
        if ops.is_empty() || ops.iter().any(|op| !matches!(op, Op::SetTeam { .. } | Op::SetLeadEffort { .. } | Op::SetRole { .. })) {
            return Err("team_err_op");
        }
        let handle = self.get(sid).ok_or("wb_err_gone")?;
        if handle.info().await.is_some_and(|i| i.meta.container || i.meta.provider.as_ref().is_some_and(|p| !p.login)) {
            return Err("team_err_provider");
        }
        let mut last = None;
        for op in ops {
            last = Some(self.workbench_apply(sid, op)?);
        }
        let _ = handle.team_changed().await;
        last.ok_or("team_err_op")
    }

    /// A new session in `cwd`; the folder must exist and must not be `/` or the home folder itself.
    pub fn create(&self, cwd: &str, model: Option<String>, provider: Option<&str>) -> Result<SessionHandle, String> {
        self.create_in(cwd, model, provider, false, None)
    }

    /// As `create`; `container` runs the chat's `claude` in a container (needs a provider, and the runtime and image set).
    /// `account` is one of the extra accounts (None: the default one); a chat has an account or a provider, not both.
    pub fn create_in(&self, cwd: &str, model: Option<String>, provider: Option<&str>, container: bool, account: Option<&str>) -> Result<SessionHandle, String> {
        self.create_in_mode(cwd, model, provider, container, account, PermMode::Default)
    }

    /// As `create_in`, with the permission mode the chat's `claude` starts with (fixed for the chat's life; see `check_mode`).
    pub fn create_in_mode(&self, cwd: &str, model: Option<String>, provider: Option<&str>, container: bool, account: Option<&str>, mode: PermMode) -> Result<SessionHandle, String> {
        let path = Path::new(cwd);
        if !path.is_absolute() {
            return Err("the folder must be an absolute path".into());
        }
        if !is_safe_cwd(path) {
            return Err("that folder does not exist, or is / or the home folder".into());
        }
        let real = path.canonicalize().map_err(|e| e.to_string())?;
        let model = model.and_then(|m| clean_model(&m));
        // A chat is bound to its provider here and never changes it. It is kept out of the history index: that index is read by the
        // account's chats (search_history), and what a third party's endpoint wrote must not be carried over to them.
        let bound = match provider.filter(|id| !id.is_empty()) {
            Some(id) => Some(self.providers.get(id).ok_or("no such provider")?.reference()),
            None => None,
        };
        if container {
            let provider = bound.as_ref().ok_or("a container chat needs a provider")?;
            let settings = self.settings.get();
            if settings.container_runtime.is_none() || settings.container_image.is_none() {
                return Err("set the container runtime and image first".into());
            }
            crate::claude::container::project_allowed(&real, &self.dir, crate::platform::home_dir().as_deref(), std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).as_deref()).map_err(str::to_owned)?;
            if crate::providers::is_local_host(&provider.host) {
                return Err("an address on this computer cannot be reached from inside a container".into());
            }
        }
        check_mode(mode, bound.is_some(), container)?;
        let bound_account = self.account_of_new_chat(bound.as_ref(), account, container)?;
        let mut meta = session::new_meta(real, model, bound, container, bound_account);
        meta.permission_mode = mode;
        let realm = self.realm_of_meta(&meta);
        let text = serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?;
        crate::private::write(&session::meta_path(&self.dir, &meta.sid), &text).map_err(|e| e.to_string())?;
        let handle = session::spawn(Setup { costs: Some(self.shared.costs.clone()), meta, bin: self.bin.clone(), dir: self.dir.clone(), log: Vec::new(), realm, mcp: self.gate.clone(), providers: Some(self.providers.clone()), settings: Some(self.settings.clone()), instance: self.instance.clone() });
        self.sessions.lock().expect("registry lock").insert(handle.sid.clone(), handle.clone());
        Ok(handle)
    }

    /// A chat made from a session of the `claude` command line program: its conversation (`history`) is written to the chat's log
    /// first, and `claude` is not started now: the first message resumes the session with `--resume`. The folder must be the one
    /// the CLI kept the session under, or the resume cannot find it.
    pub async fn import_cli(&self, cwd: &str, cli_id: &str, title: &str, account: Option<&str>, history: crate::cli_sessions::History) -> Result<SessionHandle, String> {
        self.import_inner(cwd, cli_id, title, account, history, None).await
    }

    async fn import_inner(&self, cwd: &str, cli_id: &str, title: &str, account: Option<&str>, history: crate::cli_sessions::History, carry: Option<Carry>) -> Result<SessionHandle, String> {
        let uuid_like = cli_id.len() == 36 && cli_id.bytes().all(|b| b == b'-' || b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !uuid_like {
            return Err("not a session id".into());
        }
        let real = import_folder(cwd)?;
        // A session carried over to another account has the same id as the chat it came from; only an import is checked.
        for info in self.list().await.into_iter().filter(|_| carry.is_none()) {
            if info.meta.claude_session_id.as_deref() == Some(cli_id) || info.meta.imported_from.as_deref() == Some(cli_id) {
                return Err("that session is already imported".into());
            }
        }
        let bound_account = self.bind_account(account, false, false)?;
        let mut meta = session::new_meta(real, None, None, false, bound_account);
        meta.claude_session_id = Some(cli_id.to_owned());
        meta.imported_from = Some(cli_id.to_owned());
        meta.title = title.chars().take(60).collect();
        let source = carry.as_ref().map_or("claude-cli", |c| c.source);
        if let Some(c) = carry {
            meta.model = c.model;
            meta.permission_mode = c.permission_mode;
            meta.named = c.named;
            meta.handed = c.handed;
        }
        // The log is written first, then the meta: a chat has a meta only when its log is whole.
        let mut log: Vec<Envelope> = Vec::with_capacity(history.events.len() + 1);
        let first_ts = history.events.first().map_or(meta.created, |e| e.0);
        log.push(Envelope { v: 1, sid: meta.sid.clone(), seq: 1, ts: first_ts, kind: "imported".into(), data: serde_json::json!({"source": source, "total": history.total, "dropped": history.dropped}) });
        for (n, (ts, kind, data)) in history.events.into_iter().enumerate() {
            log.push(Envelope { v: 1, sid: meta.sid.clone(), seq: n as u64 + 2, ts, kind: kind.to_owned(), data });
        }
        let text: String = log.iter().filter_map(|e| serde_json::to_string(e).ok()).collect::<Vec<_>>().join("\n") + "\n";
        let log_file = session::log_path(&self.dir, &meta.sid);
        crate::private::write(&log_file, &text).map_err(|e| e.to_string())?;
        let meta_text = serde_json::to_string_pretty(&meta).map_err(|e| e.to_string());
        let wrote = meta_text.and_then(|t| crate::private::write(&session::meta_path(&self.dir, &meta.sid), &t).map_err(|e| e.to_string()));
        if let Err(error) = wrote {
            let _ = std::fs::remove_file(&log_file);
            return Err(error);
        }
        let realm = self.realm_of_meta(&meta);
        // As when the app starts: the account's history index is brought up to date before the chat can add to it.
        if let Some(index) = realm.as_ref().and_then(|r| r.index.as_ref()) {
            index.catch_up(&meta.sid, &meta.cwd.to_string_lossy(), meta.created, log.clone());
        }
        let handle = session::spawn(Setup { costs: Some(self.shared.costs.clone()), meta, bin: self.bin.clone(), dir: self.dir.clone(), log, realm, mcp: self.gate.clone(), providers: Some(self.providers.clone()), settings: Some(self.settings.clone()), instance: self.instance.clone() });
        self.sessions.lock().expect("registry lock").insert(handle.sid.clone(), handle.clone());
        Ok(handle)
    }

    /// A chat carried on under another account: the chat stays as it is (readable, under its own account) and a new chat of `account`
    /// (None: the default account) is made that shows the same conversation and resumes the same Claude session, whose file is copied
    /// to the other account's folder. Refused for a chat that has not had a turn, one that runs a turn, and provider, container and
    /// worktree chats. Whether the other account can still work is its own matter (its limits and its login).
    pub async fn continue_as(&self, sid: &str, account: Option<&str>) -> Result<SessionHandle, String> {
        let handle = self.get(sid).ok_or("no such chat")?;
        let info = handle.info().await.ok_or("no such chat")?;
        if info.state == session::State::Running || info.pending > 0 {
            return Err("a turn is running or a permission waits in this chat".into());
        }
        let meta = info.meta;
        if meta.provider.is_some() || meta.container || meta.worktree.is_some() {
            return Err("only a plain chat of a login account can be carried to another account".into());
        }
        let csid = meta.claude_session_id.clone().ok_or("this chat has not had a turn yet: make a new chat in the other account instead")?;
        // Read first: a chat whose workbench cannot be read is not carried over (its guard would come out as a default).
        let workbench = crate::workbench::store::load(&self.dir, &meta.sid).map_err(|key| format!("this chat's workbench cannot be read ({key})"))?;
        // One handover at a time: two of them could both pass the check below before either made its chat.
        static HANDOVER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _one_at_a_time = HANDOVER.lock().await;
        let target = account.filter(|a| !a.is_empty());
        let current = meta.account.as_ref().map(|a| a.id.as_str());
        if target == current {
            return Err("the chat already runs in that account".into());
        }
        self.bind_account(target, false, false)?;
        let home = crate::platform::home_dir();
        let default = crate::accounts::default_config_dir(std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).as_deref(), home.as_deref());
        let folder_of = |id: Option<&str>| -> Result<PathBuf, String> {
            match id {
                Some(id) => self.account_login(id).map(|found| found.0),
                None => default.clone().ok_or_else(|| "the default account's folder is not known".to_owned()),
            }
        };
        let (from, to) = (folder_of(current)?, folder_of(target)?);
        let cwd = meta.cwd.to_string_lossy().into_owned();
        // The other account may already carry this session (it was continued there before). Its chat has its own turns, so it is never
        // overwritten, with one exception that is checked: this chat was made from that chat, and that chat's session file is still what it was
        // when this one was made. Then this chat is simply ahead of it, and that chat is brought up to date (a fast-forward).
        let mut existing = None;
        for other in self.list().await {
            if other.meta.sid != meta.sid && other.meta.claude_session_id.as_deref() == Some(csid.as_str()) && other.meta.account.as_ref().map(|a| a.id.as_str()) == target {
                existing = Some(other);
            }
        }
        if let Some(older) = existing {
            return self.fast_forward(&handle, &meta, &workbench, older, (&from, &to), &cwd, &csid).await;
        }
        let parent_hash = crate::handover::fingerprint(&crate::handover::session_path(&from, &cwd, &csid));
        {
            let (cwd, csid, from, to) = (cwd.clone(), csid.clone(), from.clone(), to.clone());
            tokio::task::spawn_blocking(move || crate::handover::copy_session(&from, &to, &cwd, &csid)).await.map_err(|e| e.to_string())??;
        }
        // What the chat showed, taken from the app's own record of it (not from the session file: that one may be too big, and loses
        // images and tool cards). Kept to the tail that fits.
        let (log, _) = handle.subscribe(0).await.ok_or("no such chat")?;
        let mut events: Vec<(u64, &'static str, serde_json::Value)> = Vec::new();
        let mut bytes = 0usize;
        for e in log.iter().rev().filter(|e| e.kind == "user" || e.kind == "claude") {
            let mut data = e.data.clone();
            bytes += data.to_string().len();
            if bytes > (24 << 20) || events.len() >= 5000 {
                break;
            }
            data["imported"] = serde_json::Value::Bool(true);
            events.push((e.ts, if e.kind == "user" { "user" } else { "claude" }, data));
        }
        events.reverse();
        let total = log.iter().filter(|e| e.kind == "user" || e.kind == "claude").count();
        let history = crate::cli_sessions::History { total, dropped: total.saturating_sub(events.len()), events };
        let handed = parent_hash.map(|parent_hash| session::Handed { parent: meta.sid.clone(), parent_hash });
        let carry = Carry { model: meta.model.clone(), permission_mode: meta.permission_mode, named: meta.named, source: "account", handed };
        let made = self.import_inner(&cwd, &csid, &meta.title, target, history, Some(carry)).await?;
        // The guard settings, the contract and the pins go with the conversation: a chat that continues must not come out with a weaker guard.
        if let Err(key) = crate::workbench::store::save(&self.dir, &made.sid, &workbench) {
            let _ = self.delete(&made.sid).await;
            return Err(format!("the chat's workbench could not be carried over ({key}); nothing was made"));
        }
        Ok(made)
    }

    /// Coming back to an account that already has an older chat of this session: that chat is brought up to date with this one and
    /// returned, instead of a second chat being made. Only when this chat was made from it and its session file has not changed since.
    #[allow(clippy::too_many_arguments)]
    async fn fast_forward(&self, src: &SessionHandle, meta: &Meta, workbench: &crate::workbench::Workbench, older: Info, folders: (&Path, &Path), cwd: &str, csid: &str) -> Result<SessionHandle, String> {
        let (from, to) = folders;
        let reopen = "that account already has a chat of this session that is not an earlier state of this one; open it instead";
        let handed = meta.handed.as_ref().filter(|h| h.parent == older.meta.sid).ok_or(reopen)?;
        let target_file = crate::handover::session_path(to, cwd, csid);
        let still = {
            let file = target_file.clone();
            tokio::task::spawn_blocking(move || crate::handover::fingerprint(&file)).await.map_err(|e| e.to_string())?
        };
        if still != Some(handed.parent_hash) {
            return Err("that account's session has changed since this chat was made from it; open its chat instead".into());
        }
        if older.state == session::State::Running || older.pending > 0 {
            return Err("a turn is running or a permission waits in the older chat".into());
        }
        let older_handle = self.get(&older.meta.sid).ok_or("no such chat")?;
        // What this chat made on its own since it was carried over: the events that are not themselves taken over from another chat.
        let (log, _) = src.subscribe(0).await.ok_or("no such chat")?;
        let news: Vec<(u64, String, serde_json::Value)> = log.iter().filter(|e| (e.kind == "user" || e.kind == "claude") && e.data["imported"] != true).map(|e| (e.ts, e.kind.clone(), e.data.clone())).collect();
        // The older chat's process is ended first (it holds the old session in memory), then the file is replaced, then the events are added.
        older_handle.prepare().await?;
        {
            let (cwd, csid, from, to) = (cwd.to_owned(), csid.to_owned(), from.to_owned(), to.to_owned());
            tokio::task::spawn_blocking(move || crate::handover::replace_session(&from, &to, &cwd, &csid)).await.map_err(|e| e.to_string())??;
        }
        let parent_hash = crate::handover::fingerprint(&target_file).ok_or("the session file could not be read after it was placed")?;
        crate::workbench::store::save(&self.dir, &older.meta.sid, workbench).map_err(|key| format!("the chat's workbench could not be carried back ({key})"))?;
        older_handle.absorb(news, session::Handed { parent: meta.sid.clone(), parent_hash }).await?;
        Ok(older_handle)
    }

    /// A new session in its own git worktree: `cwd` must be a git project on a branch with a commit (not a worktree already). The chat's
    /// `cwd` is the new worktree on a new branch `tenrec/<sid>` made from the project's current commit. Returns the session and a note when the
    /// chat starts at the worktree's top because `cwd` was a folder inside the project. Nothing is left behind when it fails.
    pub async fn create_isolated(&self, cwd: &str, model: Option<String>, provider: Option<&str>, account: Option<&str>) -> Result<(SessionHandle, Option<&'static str>), String> {
        self.create_isolated_mode(cwd, model, provider, account, PermMode::Default).await
    }

    /// As `create_isolated`, with the permission mode the chat's `claude` starts with.
    pub async fn create_isolated_mode(&self, cwd: &str, model: Option<String>, provider: Option<&str>, account: Option<&str>, mode: PermMode) -> Result<(SessionHandle, Option<&'static str>), String> {
        let path = Path::new(cwd);
        if !path.is_absolute() {
            return Err("the folder must be an absolute path".into());
        }
        if !is_safe_cwd(path) {
            return Err("that folder does not exist, or is / or the home folder".into());
        }
        let model = model.filter(|m| !m.trim().is_empty() && m.len() <= 100 && m.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/[]".contains(c)));
        let bound = match provider.filter(|id| !id.is_empty()) {
            Some(id) => Some(self.providers.get(id).ok_or("no such provider")?.reference()),
            None => None,
        };
        check_mode(mode, bound.is_some(), false)?;
        let bound_account = self.account_of_new_chat(bound.as_ref(), account, false)?;
        let source = crate::worktree::inspect_source(path).await?;
        let home = crate::platform::home_dir();
        let root = crate::worktree::resolve_root(self.settings.get().worktree_root.as_deref(), home.as_deref(), &self.dir, &source.repo)?;
        let mut meta = session::new_meta(PathBuf::new(), model, bound, false, bound_account);
        meta.permission_mode = mode;
        let lock = crate::worktree::repo_lock(&source.repo);
        let _guard = lock.lock().await;
        let made = crate::worktree::create(&root, &source.repo, &meta.sid, &source.commit).await?;
        let branch = crate::worktree::branch_for(&meta.sid);
        meta.cwd = made.clone();
        let config_hash = match crate::worktree::fingerprint(&source.repo, &made).await {
            Ok(fingerprint) => fingerprint.hash,
            Err(error) => {
                crate::worktree::rollback(&source.repo, &made, &branch).await;
                return Err(error);
            }
        };
        meta.worktree = Some(crate::worktree::WorktreeRef { repo: source.repo.clone(), branch: branch.clone(), base_branch: source.branch, base_commit: source.commit, removed: false, root: Some(root.clone()), config_hash });
        let written = serde_json::to_string_pretty(&meta).map_err(|e| e.to_string()).and_then(|text| crate::private::write(&session::meta_path(&self.dir, &meta.sid), &text).map_err(|e| e.to_string()));
        if let Err(error) = written {
            crate::worktree::rollback(&source.repo, &made, &branch).await;
            return Err(error);
        }
        let realm = self.realm_of_meta(&meta);
        let handle = session::spawn(Setup { costs: Some(self.shared.costs.clone()), meta, bin: self.bin.clone(), dir: self.dir.clone(), log: Vec::new(), realm, mcp: self.gate.clone(), providers: Some(self.providers.clone()), settings: Some(self.settings.clone()), instance: self.instance.clone() });
        self.sessions.lock().expect("registry lock").insert(handle.sid.clone(), handle.clone());
        Ok((handle, source.below_top.then_some("below_top")))
    }

    /// Where a chat's worktree was made: the root it was made under (looked at again, nothing made), else the setting's value now.
    pub fn worktree_root(&self, repo: &Path, stored: Option<&Path>) -> Result<PathBuf, String> {
        let home = crate::platform::home_dir();
        match stored {
            Some(root) => crate::worktree::trusted_root(root, home.as_deref(), &self.dir, repo),
            None => crate::worktree::resolve_root(self.settings.get().worktree_root.as_deref(), home.as_deref(), &self.dir, repo),
        }
    }

    /// Ends every session's process and its children at once (the host is going away).
    pub fn kill_all(&self) {
        for handle in self.sessions.lock().expect("registry lock").values() {
            handle.kill();
        }
        if let Some((_, scheduler)) = &self.sched {
            scheduler.kill_all();
        }
    }

    pub fn providers(&self) -> Arc<ProviderStore> {
        self.providers.clone()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn sched(&self) -> Option<(Arc<SchedStore>, Arc<Scheduler>)> {
        self.sched.clone()
    }

    /// The notes, skills and history of one account: None is the default account. An extra account that is not open is an error, and
    /// there is no fallback to the default account's: that would hand one account's data to a chat or page of another.
    pub fn realm(&self, account: Option<&str>) -> Result<Arc<Realm>, String> {
        match account {
            None => Ok(self.default_realm.clone()),
            Some(id) => self.realms.read().unwrap_or_else(|p| p.into_inner()).get(id).cloned().ok_or_else(|| "no such account".to_owned()),
        }
    }

    /// Every open realm, the default one first.
    pub fn all_realms(&self) -> Vec<Arc<Realm>> {
        let mut all = vec![self.default_realm.clone()];
        all.extend(self.realms.read().unwrap_or_else(|p| p.into_inner()).values().cloned());
        all
    }

    /// The realm a chat works in: its account's; none for a provider chat (which gets nothing of the account's side of the app) and
    /// for a chat whose account is not open.
    fn realm_of_meta(&self, meta: &Meta) -> Option<Arc<Realm>> {
        // A chat of a provider that uses an account's login belongs to that account (its realm); any other provider chat has none.
        if meta.provider.as_ref().is_some_and(|p| !p.login) {
            return None;
        }
        self.realm(meta.account.as_ref().map(|a| a.id.as_str())).ok()
    }

    /// The account of a new chat. An ordinary chat has the one it was asked for (and no provider); a chat of a provider that uses an account's
    /// login runs in THAT account (the provider's own; another one asked for is refused) and never in a container.
    fn account_of_new_chat(&self, provider: Option<&crate::providers::ProviderRef>, account: Option<&str>, container: bool) -> Result<Option<AccountRef>, String> {
        let Some(bound) = provider.filter(|p| p.login) else { return self.bind_account(account, provider.is_some(), container) };
        if container {
            return Err("a provider that uses an account's login cannot run in a container".into());
        }
        let login_account = self.providers.get(&bound.id).and_then(|p| p.login_account);
        if let Some(asked) = account.filter(|a| !a.is_empty()) {
            if Some(asked) != login_account.as_deref() {
                return Err("a chat of a provider that uses an account's login runs in that account only".into());
            }
        }
        self.bind_account(login_account.as_deref(), false, false)
    }

    /// The account a new chat is bound to, checked: it must exist and be open, and cannot be mixed with a provider or a container.
    fn bind_account(&self, account: Option<&str>, has_provider: bool, container: bool) -> Result<Option<AccountRef>, String> {
        let Some(id) = account.filter(|id| !id.is_empty()) else { return Ok(None) };
        if has_provider || container {
            return Err("a chat is of an account or of a provider, not both".into());
        }
        let entry = self.accounts.get(id).ok_or("no such account")?;
        // Said before the realm is looked for: a Codex account has none, and "no such account" would be the wrong reason.
        match entry.kind {
            crate::accounts::AccountKind::Claude => {}
            crate::accounts::AccountKind::Codex => return Err("this is a Codex account: in this version a chat runs only in a Claude account".into()),
        }
        if self.realm(Some(id))?.is_retired() {
            return Err("this account is being removed".into());
        }
        Ok(Some(AccountRef { id: entry.id, name: entry.name }))
    }

    /// The limits `claude` reported for each account (the default one under "").
    pub fn usage(&self) -> &crate::usage::UsageStore {
        &self.shared.usage
    }

    /// What the chats cost, by day, account and provider.
    pub fn costs(&self) -> &crate::costs::CostStore {
        &self.shared.costs
    }

    /// Asks the program for the account's usage now (`/usage`, which it answers itself; see `usage_cli`) and keeps what it says as the
    /// stream's events are kept. `account` None (or empty) is the default login. Ok(changed); the Err is a word for the page: `busy` (one
    /// is running), `soon` (one ran a minute ago), `failed` (the program did not answer, or the account has no Claude login),
    /// `unreadable` (it answered, and nothing in it was understood: the stored numbers stay).
    pub async fn refresh_usage(&self, account: Option<&str>) -> Result<bool, &'static str> {
        let account = account.filter(|a| !a.is_empty());
        // The account is looked at first: an id that is none must not take a place among the asks that are held.
        let (dir, env) = match account {
            Some(id) => self.account_login(id).map_err(|_| "failed")?,
            None => (crate::accounts::default_claude_cwd(), EnvPlan::default()),
        };
        let now = crate::now_ms();
        let _held = self.shared.usage.begin_probe(account.unwrap_or_default(), now).map_err(|skip| match skip {
            crate::usage::Skip::Busy => "busy",
            crate::usage::Skip::Soon => "soon",
        })?;
        // One program at a time for the whole app: each run starts the program's own start-up hooks, and several accounts asked together
        // would be several of those at once (the person's security software may take notice of the burst).
        static ONE_AT_A_TIME: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
        let _turn = ONE_AT_A_TIME.acquire().await.map_err(|_| "failed")?;
        let text = crate::usage_cli::ask(&self.bin, &dir, &env).await.map_err(|error| {
            tracing::warn!("the usage could not be asked: {error}");
            "failed"
        })?;
        let info = crate::usage_cli::to_info(&text, crate::now_ms() / 1000, &chrono::Local).ok_or("unreadable")?;
        // The account may have been removed while the program was answering: its numbers must not come back.
        if let Some(id) = account {
            if self.accounts.get(id).is_none() || self.realm(Some(id)).map_or(true, |r| r.is_retired()) {
                return Err("failed");
            }
        }
        Ok(self.shared.usage.record(account, &info, crate::now_ms()))
    }

    pub fn accounts(&self) -> Arc<AccountStore> {
        self.accounts.clone()
    }

    pub fn bin(&self) -> &str {
        &self.bin
    }

    /// Adds an extra account: its folders, its entry in the accounts file and, for a Claude login, its own empty notes, skills and history.
    pub fn add_account(&self, name: &str, kind: crate::accounts::AccountKind) -> Result<crate::accounts::Account, String> {
        let account = self.accounts.add(name, kind, crate::now_ms())?;
        if !kind.has_realm() {
            return Ok(account);
        }
        match open_account_realm(&self.dir, &account.id, &self.shared) {
            Ok(realm) => {
                self.realms.write().unwrap_or_else(|p| p.into_inner()).insert(account.id.clone(), realm);
                Ok(account)
            }
            Err(error) => {
                let _ = self.accounts.remove_entry(&account.id);
                let _ = std::fs::remove_dir(crate::accounts::config_dir(&self.dir, &account.id));
                let _ = std::fs::remove_dir(crate::accounts::stores_dir(&self.dir, &account.id));
                Err(error)
            }
        }
    }

    /// Where an extra account's login lives and the environment its `claude` runs with, after looking at the folder again.
    pub fn account_login(&self, id: &str) -> Result<(PathBuf, EnvPlan), String> {
        let entry = self.accounts.get(id).ok_or("no such account")?;
        match entry.kind {
            crate::accounts::AccountKind::Claude => {}
            crate::accounts::AccountKind::Codex => return Err("this is a Codex account; it has no Claude login".into()),
        }
        let home = crate::platform::home_dir();
        let default = crate::accounts::default_config_dir(std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).as_deref(), home.as_deref());
        let real = crate::accounts::check_dir(&crate::accounts::config_dir(&self.dir, id), &self.dir.join("accounts"), default.as_deref())?;
        let env = crate::claude::env::account_plan(&crate::claude::env::inherited_names(), &real);
        Ok((real, env))
    }

    /// Where a Codex account's login lives (its `CODEX_HOME`), after looking at the folder again. An account of another kind is refused.
    pub fn codex_home(&self, id: &str) -> Result<PathBuf, String> {
        let entry = self.accounts.get(id).ok_or("no such account")?;
        match entry.kind {
            crate::accounts::AccountKind::Codex => {}
            crate::accounts::AccountKind::Claude => return Err("this is not a Codex account".into()),
        }
        crate::accounts::check_dir(&crate::accounts::config_dir(&self.dir, id), &self.dir.join("accounts"), crate::accounts::default_home_of(entry.kind).as_deref())
    }

    /// A Codex account has no chats, notes or history to take with it: its login is dropped through the program (best effort) and its
    /// folder is removed. Returns what could not be cleaned up.
    async fn delete_codex_account(&self, id: &str) -> Result<Vec<&'static str>, String> {
        let mut warnings = Vec::new();
        let path_var = std::env::var_os("PATH");
        match (self.codex_home(id), crate::codex::bin(path_var.as_deref()).and_then(|b| b.to_str().map(str::to_owned))) {
            (Ok(home), Some(bin)) => {
                let env = crate::codex::plan(&crate::claude::env::inherited_names(), &home);
                if crate::codex::logout(&bin, &home, &env).await.is_err() {
                    warnings.push("logout_failed");
                }
            }
            _ => warnings.push("logout_failed"),
        }
        self.accounts.remove_entry(id)?;
        self.shared.usage.forget(id);
        self.shared.costs.forget(id);
        let default = crate::accounts::default_home_of(crate::accounts::AccountKind::Codex);
        // Looked at again: only the real, private folder this app made is removed, never what a link points to.
        match crate::accounts::check_dir(&crate::accounts::config_dir(&self.dir, id), &self.dir.join("accounts"), default.as_deref()) {
            Ok(real) => {
                if std::fs::remove_dir_all(&real).is_err() {
                    warnings.push("folder_left");
                }
            }
            Err(_) => warnings.push("folder_left"),
        }
        Ok(warnings)
    }

    /// Removes an extra account together with its notes, skills, history and login. Its chats stay readable but can no longer be
    /// started (they are told to make a new chat). Refused while one of them is running a turn. Returns what could not be cleaned up.
    pub async fn delete_account(&self, id: &str) -> Result<Vec<&'static str>, String> {
        let entry = self.accounts.get(id).ok_or("no such account")?;
        if entry.kind == crate::accounts::AccountKind::Codex {
            return self.delete_codex_account(id).await;
        }
        let realm = self.realms.read().unwrap_or_else(|p| p.into_inner()).get(id).cloned();
        // Retired before anything is looked at: a chat that starts from now on, and every worker, is refused by the flag, which the
        // chats of the account (they hold the realm) and its workers see; taking the realm out of the map would not reach them.
        if let Some(realm) = &realm {
            realm.set_retired(true);
        }
        let bound: Vec<Info> = self.list().await.into_iter().filter(|i| i.meta.account.as_ref().map(|a| a.id.as_str()) == Some(id)).collect();
        if bound.iter().any(|i| i.state == session::State::Running) {
            if let Some(realm) = &realm {
                realm.set_retired(false);
            }
            return Err("a turn is running in a chat of this account; stop it first".into());
        }
        self.realms.write().unwrap_or_else(|p| p.into_inner()).remove(id);
        for info in &bound {
            if let Some(handle) = self.get(&info.meta.sid) {
                handle.stop();
            }
        }
        // The processes are given a moment to end before their folder is taken away (a dying `claude` may still write there). A chat
        // that never started one stays idle, so this ends by its time limit.
        for _ in 0..30 {
            let mut all_ended = true;
            for info in &bound {
                if let Some(now) = match self.get(&info.meta.sid) {
                    Some(handle) => handle.info().await,
                    None => None,
                } {
                    all_ended &= matches!(now.state, session::State::Exited { .. });
                }
            }
            if all_ended {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        if let Some(index) = realm.as_ref().and_then(|r| r.index.as_ref()) {
            index.flush();
        }
        let mut warnings = Vec::new();
        match self.account_login(id) {
            Ok((dir, env)) => {
                if crate::accounts::logout(&self.bin, &dir, &env).await.is_err() {
                    warnings.push("logout_failed");
                }
            }
            Err(_) => warnings.push("logout_failed"),
        }
        if let Err(error) = self.accounts.remove_entry(id) {
            if let Some(realm) = realm {
                realm.set_retired(false);
                self.realms.write().unwrap_or_else(|p| p.into_inner()).insert(id.to_owned(), realm);
            }
            return Err(error);
        }
        drop(realm);
        self.shared.usage.forget(id);
        self.shared.costs.forget(id);
        let home = crate::platform::home_dir();
        let default = crate::accounts::default_config_dir(std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).as_deref(), home.as_deref());
        for (parent, leaf) in [("accounts", crate::accounts::config_dir(&self.dir, id)), ("stores", crate::accounts::stores_dir(&self.dir, id))] {
            // Looked at again: only the real, private folder this app made is removed, never what a link points to.
            match crate::accounts::check_dir(&leaf, &self.dir.join(parent), default.as_deref()) {
                Ok(real) => {
                    if std::fs::remove_dir_all(&real).is_err() {
                        warnings.push("folder_left");
                    }
                }
                Err(_) => warnings.push("folder_left"),
            }
        }
        Ok(warnings)
    }

    pub fn settings(&self) -> Arc<SettingsStore> {
        self.settings.clone()
    }

    pub fn gate(&self) -> Option<Arc<McpGate>> {
        self.gate.clone()
    }

    /// Remembers what a chat made from the new-chat form was made with, for the next form of the same project. Not for a chat on a provider
    /// or in a container (not put on a third party or in a box by what was chosen last time), and never the mode that skips every check.
    /// `asked` is the folder the form was made for (a worktree chat may have been made from a subfolder of its repository: the next form for
    /// that subfolder must find it); without it, or when it is not a folder that is there, the chat's project is used.
    pub fn remember_defaults(&self, meta: &Meta, asked: Option<&str>) {
        if meta.provider.is_some() || meta.container {
            return;
        }
        let asked = asked.map(std::path::Path::new).filter(|p| p.is_absolute()).and_then(crate::project_defaults::key_of);
        let key = asked.or_else(|| crate::project_defaults::key_of(&meta.project_dir())).unwrap_or_else(|| meta.project_key());
        let entry = crate::project_defaults::Entry {
            model: meta.model.clone(),
            account: meta.account.as_ref().map(|a| a.id.clone()),
            permission: Some(meta.permission_mode.name().to_owned()),
            worktree: meta.worktree.is_some(),
            used: 0,
        };
        self.defaults.record(&key, entry);
    }

    /// What the form starts with for the folder `cwd` (see `project_defaults::form_json`); `found: false` for a folder that is not there, not
    /// absolute, or never made a chat from the form.
    pub fn project_defaults(&self, cwd: &str) -> serde_json::Value {
        let path = std::path::Path::new(cwd);
        let entry = path.is_absolute().then(|| crate::project_defaults::key_of(path)).flatten().and_then(|key| self.defaults.get(&key));
        crate::project_defaults::form_json(cwd, entry, |id| self.accounts.get(id).is_some_and(|a| a.kind.has_realm()))
    }

    pub fn get(&self, sid: &str) -> Option<SessionHandle> {
        self.sessions.lock().expect("registry lock").get(sid).cloned()
    }

    /// Every session, newest first.
    /// Deletes a chat for good (see `SessionHandle::remove`); the history index of its account forgets it too.
    pub async fn delete(&self, sid: &str) -> Result<(), String> {
        let handle = self.get(sid).ok_or("no such session")?;
        let meta = handle.info().await.ok_or("the session is gone")?.meta;
        handle.remove().await?;
        crate::workbench::store::remove(&self.dir, sid);
        crate::attachments::remove_all(&self.dir, sid);
        self.sessions.lock().expect("registry lock").remove(sid);
        if let Some(index) = self.realm_of_meta(&meta).and_then(|r| r.index.clone()) {
            let account = meta.account.as_ref().map(|a| a.id.clone());
            let keep: std::collections::HashSet<String> = self.list().await.into_iter().filter(|i| i.meta.provider.is_none() && i.meta.account.as_ref().map(|a| a.id.clone()) == account).map(|i| i.meta.sid).collect();
            index.keep_only(keep);
        }
        Ok(())
    }

    pub async fn list(&self) -> Vec<Info> {
        let handles: Vec<SessionHandle> = self.sessions.lock().expect("registry lock").values().cloned().collect();
        let mut all = Vec::new();
        for handle in handles {
            if let Some(info) = handle.info().await {
                all.push(info);
            }
        }
        all.sort_by_key(|s| std::cmp::Reverse(s.meta.created));
        all
    }
}

/// Opens one extra account's realm after looking at both of its folders again: they must be the private, real folders this app made.
fn open_account_realm(data: &Path, id: &str, shared: &Shared) -> Result<Arc<Realm>, String> {
    let home = crate::platform::home_dir();
    let default = crate::accounts::default_config_dir(std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).as_deref(), home.as_deref());
    let config = crate::accounts::check_dir(&crate::accounts::config_dir(data, id), &data.join("accounts"), default.as_deref())?;
    let stores = crate::accounts::check_dir(&crate::accounts::stores_dir(data, id), &data.join("stores"), default.as_deref())?;
    let env = crate::claude::env::account_plan(&crate::claude::env::inherited_names(), &config);
    Ok(Realm::open(Some(id.to_owned()), &stores, env, shared))
}

/// Where the MCP socket goes: the data folder when its path fits a Unix socket (108 bytes), else the user's runtime folder.
fn socket_path(dir: &Path) -> Option<PathBuf> {
    // Windows: a named pipe with a new random name each run, open to this user and the system only (see `mcp_host`).
    if cfg!(windows) {
        return Some(PathBuf::from(format!(r"\\.\pipe\tenrec-{}", crate::random_hex(16))));
    }
    let here = dir.join("mcp.sock");
    if here.as_os_str().len() <= 100 {
        return Some(here);
    }
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    // The same data folder always gets the same name, so runs do not leave a socket each.
    let hash = dir.to_string_lossy().bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    let there = runtime.join(format!("tenrec-{hash:016x}.sock"));
    (there.as_os_str().len() <= 100).then_some(there)
}

/// The events of a chat's log, one per line. A line that cannot be read (a write cut short can leave half a line, even in the middle of a
/// letter) is skipped and every other line is kept; one bad line never costs the chat its history.
fn read_log(path: &Path) -> Vec<Envelope> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    bytes.split(|b| *b == b'\n').filter_map(|raw| log_event(&String::from_utf8_lossy(raw))).collect()
}

/// One log line as an event. A torn tail that the next event was written straight after is not a line of its own: the event after it is
/// found by where an event begins (`{"v":`, the first field of an event; inside the text of one it is escaped).
fn log_event(line: &str) -> Option<Envelope> {
    if let Ok(event) = serde_json::from_str::<Envelope>(line) {
        return Some(event);
    }
    line.rmatch_indices("{\"v\":").find_map(|(at, _)| serde_json::from_str::<Envelope>(&line[at..]).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_line(seq: u64, text: &str) -> String {
        serde_json::to_string(&session::Envelope { v: 1, sid: "s".into(), seq, ts: 1, kind: "user".into(), data: serde_json::json!({"text": text}) }).unwrap()
    }

    fn seqs(path: &Path) -> Vec<u64> {
        read_log(path).iter().map(|e| e.seq).collect()
    }

    #[test]
    fn a_torn_utf8_tail_keeps_every_good_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let mut bytes = format!("{}\n{}\n", log_line(1, "a"), log_line(2, "한글")).into_bytes();
        let torn = log_line(3, "한글");
        // Cut in the middle of the first Korean letter: the tail is not valid UTF-8.
        bytes.extend_from_slice(&torn.as_bytes()[..torn.find('한').unwrap() + 1]);
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(seqs(&path), [1, 2]);
    }

    #[test]
    fn a_glued_line_keeps_the_event_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let torn = log_line(2, "한글");
        let torn = &torn[..torn.find('한').unwrap() + 3];
        std::fs::write(&path, format!("{}\n{torn}{}\n{}\n", log_line(1, "a"), log_line(3, "c"), log_line(4, "d"))).unwrap();
        assert_eq!(seqs(&path), [1, 3, 4]);
    }

    #[test]
    fn unparsable_lines_in_the_middle_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, format!("{}\nnot json\n\n{{\"v\":1}}\n{}\n", log_line(1, "a"), log_line(2, "b"))).unwrap();
        assert_eq!(seqs(&path), [1, 2]);
        assert!(read_log(&dir.path().join("missing.jsonl")).is_empty());
    }

    #[test]
    fn the_data_folder_is_named_tenrec_and_a_folder_of_the_first_name_is_kept_where_it_is() {
        let home = tempfile::tempdir().unwrap();
        let share = home.path().join(".local").join("share");
        assert_eq!(data_dir_in(None, home.path()), share.join("tenrec"), "nothing exists yet: the new name");
        std::fs::create_dir_all(share.join("smart-workbench-app")).unwrap();
        assert_eq!(data_dir_in(None, home.path()), share.join("smart-workbench-app"), "a folder of the first name is used, not moved");
        std::fs::create_dir_all(share.join("tenrec")).unwrap();
        assert_eq!(data_dir_in(None, home.path()), share.join("tenrec"), "once the new one exists it wins");
        assert_eq!(data_dir_in(Some(PathBuf::from("/chosen")), home.path()), PathBuf::from("/chosen"), "a folder named by the person always wins");
    }
}
