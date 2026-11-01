//! The sessions of this app: created here, kept on disk (a meta file and a log each), listed for the sidebar.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::claude::process::{claude_bin, is_safe_cwd};
use crate::session::{self, Envelope, Info, Meta, SessionHandle, Setup};

pub struct Registry {
    dir: PathBuf,
    bin: String,
    sessions: Mutex<HashMap<String, SessionHandle>>,
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
        std::fs::create_dir_all(&dir)?;
        let registry = Self { dir: dir.clone(), bin: bin.unwrap_or_else(claude_bin), sessions: Mutex::new(HashMap::new()) };
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
            let handle = session::spawn(Setup { meta, bin: registry.bin.clone(), dir: dir.clone(), log });
            registry.sessions.lock().expect("registry lock").insert(sid.to_owned(), handle);
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
        std::fs::write(session::meta_path(&self.dir, &meta.sid), text).map_err(|e| e.to_string())?;
        let handle = session::spawn(Setup { meta, bin: self.bin.clone(), dir: self.dir.clone(), log: Vec::new() });
        self.sessions.lock().expect("registry lock").insert(handle.sid.clone(), handle.clone());
        Ok(handle)
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

fn read_log(path: &Path) -> Vec<Envelope> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    text.lines().filter_map(|line| serde_json::from_str::<Envelope>(line).ok()).collect()
}
