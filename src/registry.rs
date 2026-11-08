//! The sessions of this app: created here, kept on disk (a meta file and a log each), listed for the sidebar.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::claude::process::{claude_bin, is_safe_cwd};
use crate::extract::{ExtractConfig, TurnEnd, Worker};
use crate::index::Index;
use crate::mcp::McpGate;
use crate::memory::MemoryStore;
use crate::propose::{spawn_worker, ProposeConfig, ProposeCtx};
use crate::providers::ProviderStore;
use crate::random_hex;
use crate::sched_store::SchedStore;
use crate::schedule::SystemClock;
use crate::scheduler::{SchedConfig, Scheduler};
use crate::skills::SkillStore;
use crate::settings::SettingsStore;
use crate::session::{self, Envelope, Info, Meta, SessionHandle, Setup};

pub struct Registry {
    dir: PathBuf,
    bin: String,
    index: Option<Arc<Index>>,
    gate: Option<Arc<McpGate>>,
    memory: Option<Arc<MemoryStore>>,
    settings: Arc<SettingsStore>,
    /// Messages every page should hear (a note changed, an auto-save happened): `{v, kind, data}`.
    pub events: tokio::sync::broadcast::Sender<serde_json::Value>,
    turns: Option<tokio::sync::mpsc::UnboundedSender<TurnEnd>>,
    skills: Option<Arc<SkillStore>>,
    propose_ctx: Option<Arc<ProposeCtx>>,
    skill_turns: Option<tokio::sync::mpsc::UnboundedSender<TurnEnd>>,
    /// Set when the skills file could not be read and was set aside.
    pub skills_notice: Option<String>,
    /// Set when the memory file could not be read and was set aside.
    pub memory_notice: Option<String>,
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
    if let Some(dir) = std::env::var_os("SWB_APP_DATA") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".local").join("share").join("smart-workbench-app")
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
        let index = match Index::open(&dir.join("index.sqlite")) {
            Ok(index) => Some(index),
            Err(error) => {
                tracing::warn!("history search is off: {error:#}");
                None
            }
        };
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
        let (memory, memory_notice) = match MemoryStore::open(&dir.join("memory.sqlite")) {
            Ok((store, notice)) => (Some(Arc::new(store)), notice),
            Err(error) => {
                tracing::warn!("memory is off: {error:#}");
                (None, Some(format!("memory is off: {error:#}")))
            }
        };
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
        let turns = memory.as_ref().map(|store| {
            Worker { store: store.clone(), settings: settings.clone(), sessions: sessions.clone(), events: events.clone(), bin: bin.clone(), workdir: dir.join("extract"), config: extract }.spawn()
        });
        let (skills, skills_notice) = match SkillStore::open(&dir.join("skills.sqlite")) {
            Ok((store, notice)) => (Some(Arc::new(store)), notice),
            Err(error) => {
                tracing::warn!("skills are off: {error:#}");
                (None, Some(format!("skills are off: {error:#}")))
            }
        };
        let propose_ctx = skills.as_ref().map(|store| Arc::new(ProposeCtx { store: store.clone(), settings: settings.clone(), sessions: sessions.clone(), events: events.clone(), bin: bin.clone(), workdir: dir.join("propose"), config: propose }));
        let skill_turns = propose_ctx.as_ref().map(|ctx| spawn_worker(ctx.clone()));
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
        let registry = Self { dir: dir.clone(), bin, index, gate, memory, settings, events, turns, skills, propose_ctx, skill_turns, skills_notice, memory_notice, sched_notice, sched, providers: Arc::new(providers), providers_notice, instance, sessions };
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
            let meta_has_no_provider = meta.provider.is_none();
            // The index is brought up to date before the session can emit anything of its own. A provider chat is not in it.
            if let (Some(index), None) = (&registry.index, &meta.provider) {
                index.catch_up(sid, &meta.cwd.to_string_lossy(), meta.created, log.clone());
            }
            let handle = session::spawn(Setup { meta, bin: registry.bin.clone(), dir: dir.clone(), log, index: registry.index.clone().filter(|_| meta_has_no_provider), mcp: registry.gate.clone(), memory: registry.memory.clone(), turns: registry.turns.clone(), skills: registry.skills.clone(), skill_turns: registry.skill_turns.clone(), providers: Some(registry.providers.clone()), settings: Some(registry.settings.clone()), instance: registry.instance.clone() });
            registry.sessions.lock().expect("registry lock").insert(sid.to_owned(), handle);
        }
        if let Some(index) = &registry.index {
            let keep = registry.sessions.lock().expect("registry lock").keys().cloned().collect();
            index.keep_only(keep);
        }
        Ok(registry)
    }

    /// A new session in `cwd`; the folder must exist and must not be `/` or the home folder itself.
    pub fn create(&self, cwd: &str, model: Option<String>, provider: Option<&str>) -> Result<SessionHandle, String> {
        self.create_in(cwd, model, provider, false)
    }

    /// As `create`; `container` runs the chat's `claude` in a container (needs a provider, and the runtime and image set).
    pub fn create_in(&self, cwd: &str, model: Option<String>, provider: Option<&str>, container: bool) -> Result<SessionHandle, String> {
        let path = Path::new(cwd);
        if !path.is_absolute() {
            return Err("the folder must be an absolute path".into());
        }
        if !is_safe_cwd(path) {
            return Err("that folder does not exist, or is / or the home folder".into());
        }
        let real = path.canonicalize().map_err(|e| e.to_string())?;
        let model = model.filter(|m| !m.trim().is_empty() && m.len() <= 100 && m.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/[]".contains(c)));
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
            crate::claude::container::project_allowed(&real, &self.dir, std::env::var_os("HOME").map(PathBuf::from).as_deref()).map_err(str::to_owned)?;
            if crate::providers::is_local_host(&provider.host) {
                return Err("an address on this computer cannot be reached from inside a container".into());
            }
        }
        let meta = session::new_meta(real, model, bound, container);
        let meta_is_normal = meta.provider.is_none();
        let text = serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?;
        crate::private::write(&session::meta_path(&self.dir, &meta.sid), &text).map_err(|e| e.to_string())?;
        let handle = session::spawn(Setup { meta, bin: self.bin.clone(), dir: self.dir.clone(), log: Vec::new(), index: self.index.clone().filter(|_| meta_is_normal), mcp: self.gate.clone(), memory: self.memory.clone(), turns: self.turns.clone(), skills: self.skills.clone(), skill_turns: self.skill_turns.clone(), providers: Some(self.providers.clone()), settings: Some(self.settings.clone()), instance: self.instance.clone() });
        self.sessions.lock().expect("registry lock").insert(handle.sid.clone(), handle.clone());
        Ok(handle)
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

    pub fn index(&self) -> Option<Arc<Index>> {
        self.index.clone()
    }

    pub fn skills(&self) -> Option<Arc<SkillStore>> {
        self.skills.clone()
    }

    pub fn propose_ctx(&self) -> Option<Arc<ProposeCtx>> {
        self.propose_ctx.clone()
    }

    pub fn memory(&self) -> Option<Arc<MemoryStore>> {
        self.memory.clone()
    }

    pub fn settings(&self) -> Arc<SettingsStore> {
        self.settings.clone()
    }

    pub fn gate(&self) -> Option<Arc<McpGate>> {
        self.gate.clone()
    }

    pub fn get(&self, sid: &str) -> Option<SessionHandle> {
        self.sessions.lock().expect("registry lock").get(sid).cloned()
    }

    /// Every session, newest first.
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

/// Where the MCP socket goes: the data folder when its path fits a Unix socket (108 bytes), else the user's runtime folder.
fn socket_path(dir: &Path) -> Option<PathBuf> {
    let here = dir.join("mcp.sock");
    if here.as_os_str().len() <= 100 {
        return Some(here);
    }
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    // The same data folder always gets the same name, so runs do not leave a socket each.
    let hash = dir.to_string_lossy().bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    let there = runtime.join(format!("swb-app-{hash:016x}.sock"));
    (there.as_os_str().len() <= 100).then_some(there)
}

fn read_log(path: &Path) -> Vec<Envelope> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    text.lines().filter_map(|line| serde_json::from_str::<Envelope>(line).ok()).collect()
}
