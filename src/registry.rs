//! The sessions of this app: created here, kept on disk (a meta file and a log each), listed for the sidebar.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::claude::process::{claude_bin, is_safe_cwd};
use crate::extract::{ExtractConfig, TurnEnd, Worker};
use crate::index::Index;
use crate::mcp::McpGate;
use crate::memory::MemoryStore;
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
    /// Set when the memory file could not be read and was set aside.
    pub memory_notice: Option<String>,
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
        Self::open_full(dir, bin, mcp_exe, ExtractConfig::default())
    }

    /// As `open_with`, with the auto-save timing given (tests use short waits). Needs a running tokio runtime.
    pub fn open_full(dir: PathBuf, bin: Option<String>, mcp_exe: Option<PathBuf>, extract: ExtractConfig) -> anyhow::Result<Self> {
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
        // Notes handed to a chat live in files only while its process does: any left by a crash are removed.
        if let Ok(entries) = std::fs::read_dir(dir.join("run")) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().ends_with(".mem.txt") {
                    let _ = std::fs::remove_file(entry.path());
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
        let (events, _) = tokio::sync::broadcast::channel(64);
        let sessions: Arc<Mutex<HashMap<String, SessionHandle>>> = Arc::new(Mutex::new(HashMap::new()));
        let bin = bin.unwrap_or_else(claude_bin);
        let turns = memory.as_ref().map(|store| {
            Worker { store: store.clone(), settings: settings.clone(), sessions: sessions.clone(), events: events.clone(), bin: bin.clone(), workdir: dir.join("extract"), config: extract }.spawn()
        });
        let registry = Self { dir: dir.clone(), bin, index, gate, memory, settings, events, turns, memory_notice, sessions };
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
            // The index is brought up to date before the session can emit anything of its own.
            if let Some(index) = &registry.index {
                index.catch_up(sid, &meta.cwd.to_string_lossy(), meta.created, log.clone());
            }
            let handle = session::spawn(Setup { meta, bin: registry.bin.clone(), dir: dir.clone(), log, index: registry.index.clone(), mcp: registry.gate.clone(), memory: registry.memory.clone(), turns: registry.turns.clone() });
            registry.sessions.lock().expect("registry lock").insert(sid.to_owned(), handle);
        }
        if let Some(index) = &registry.index {
            let keep = registry.sessions.lock().expect("registry lock").keys().cloned().collect();
            index.keep_only(keep);
        }
        Ok(registry)
    }

    /// A new session in `cwd`; the folder must exist and must not be `/` or the home folder itself.
    pub fn create(&self, cwd: &str, model: Option<String>) -> Result<SessionHandle, String> {
        let path = Path::new(cwd);
        if !path.is_absolute() {
            return Err("the folder must be an absolute path".into());
        }
        if !is_safe_cwd(path) {
            return Err("that folder does not exist, or is / or the home folder".into());
        }
        let real = path.canonicalize().map_err(|e| e.to_string())?;
        let model = model.filter(|m| !m.trim().is_empty() && m.len() <= 100 && m.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/[]".contains(c)));
        let meta = session::new_meta(real, model);
        let text = serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?;
        crate::private::write(&session::meta_path(&self.dir, &meta.sid), &text).map_err(|e| e.to_string())?;
        let handle = session::spawn(Setup { meta, bin: self.bin.clone(), dir: self.dir.clone(), log: Vec::new(), index: self.index.clone(), mcp: self.gate.clone(), memory: self.memory.clone(), turns: self.turns.clone() });
        self.sessions.lock().expect("registry lock").insert(handle.sid.clone(), handle.clone());
        Ok(handle)
    }

    /// Ends every session's process and its children at once (the host is going away).
    pub fn kill_all(&self) {
        for handle in self.sessions.lock().expect("registry lock").values() {
            handle.kill();
        }
    }

    pub fn index(&self) -> Option<Arc<Index>> {
        self.index.clone()
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
