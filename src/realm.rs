//! Everything the app keeps for one account: the history index, the notes, the skills, and the two workers that make notes and skill
//! proposals from that account's chats (each runs `claude` as that account).
//!
//! The boundary between accounts is this type. A chat, an MCP request, a page's message or a worker is handed one `Realm` and has no
//! way to reach another one's files, so no query has a filter that could be forgotten. The default account's realm uses the files the
//! app always had (`index.sqlite`, `memory.sqlite`, `skills.sqlite`, the `extract` and `propose` folders in the data folder); every
//! other account has its own in `<data>/stores/<id>`.
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, mpsc};

use crate::claude::env::EnvPlan;
use crate::extract::{ExtractConfig, TurnEnd, Worker};
use crate::index::Index;
use crate::memory::MemoryStore;
use crate::propose::{spawn_worker, ProposeConfig, ProposeCtx};
use crate::session::SessionHandle;
use crate::settings::SettingsStore;
use crate::skills::SkillStore;
use crate::usage::UsageStore;

/// What every realm is built with and shares: the program, the settings, the open chats and the page-wide events.
#[derive(Clone)]
pub struct Shared {
    pub bin: String,
    pub settings: Arc<SettingsStore>,
    pub sessions: Arc<Mutex<HashMap<String, SessionHandle>>>,
    pub events: broadcast::Sender<serde_json::Value>,
    pub extract: ExtractConfig,
    pub propose: ProposeConfig,
    /// The limits `claude` reports, kept per account.
    pub usage: Arc<UsageStore>,
}

pub struct Realm {
    /// None for the default account.
    pub account: Option<String>,
    pub index: Option<Arc<Index>>,
    pub memory: Option<Arc<MemoryStore>>,
    pub skills: Option<Arc<SkillStore>>,
    pub propose: Option<Arc<ProposeCtx>>,
    /// Where a chat of this account records the limits its stream reports.
    pub usage: Arc<UsageStore>,
    /// Told when a chat of this account ends a turn well, so auto-save can read what the person wrote.
    pub turns: Option<mpsc::UnboundedSender<TurnEnd>>,
    /// Told the same, so an automatic skill proposal can be considered.
    pub skill_turns: Option<mpsc::UnboundedSender<TurnEnd>>,
    /// Set when the memory (skills) file could not be read and was set aside.
    pub memory_notice: Option<String>,
    pub skills_notice: Option<String>,
    /// Set when the account is being removed (and kept set once it is): nothing starts for it, and its workers read nothing. It is
    /// shared with the workers, which hold no realm of their own. The chats of the account hold this realm until they end, so the
    /// flag, not the map it was taken out of, is what stops them.
    retired: Arc<AtomicBool>,
}

impl Realm {
    /// Opens the files of one account in `store_dir` and starts its workers (needs a running tokio runtime). A store that cannot be
    /// opened turns that feature off for this account only; it never falls back to another account's.
    pub fn open(account: Option<String>, store_dir: &Path, env: EnvPlan, shared: &Shared) -> Arc<Self> {
        let label = account.as_deref().unwrap_or("default");
        let index = match Index::open(&store_dir.join("index.sqlite")) {
            Ok(index) => Some(index),
            Err(error) => {
                tracing::warn!("history search is off for the {label} account: {error:#}");
                None
            }
        };
        let (memory, memory_notice) = match MemoryStore::open(&store_dir.join("memory.sqlite")) {
            Ok((store, notice)) => (Some(Arc::new(store)), notice),
            Err(error) => {
                tracing::warn!("memory is off for the {label} account: {error:#}");
                (None, Some(format!("memory is off: {error:#}")))
            }
        };
        let retired = Arc::new(AtomicBool::new(false));
        let turns = memory.as_ref().map(|store| {
            Worker {
                store: store.clone(),
                settings: shared.settings.clone(),
                sessions: shared.sessions.clone(),
                events: shared.events.clone(),
                bin: shared.bin.clone(),
                workdir: store_dir.join("extract"),
                config: shared.extract.clone(),
                account: account.clone(),
                env: env.clone(),
                retired: retired.clone(),
            }
            .spawn()
        });
        let (skills, skills_notice) = match SkillStore::open(&store_dir.join("skills.sqlite")) {
            Ok((store, notice)) => (Some(Arc::new(store)), notice),
            Err(error) => {
                tracing::warn!("skills are off for the {label} account: {error:#}");
                (None, Some(format!("skills are off: {error:#}")))
            }
        };
        let propose = skills.as_ref().map(|store| {
            Arc::new(ProposeCtx {
                store: store.clone(),
                settings: shared.settings.clone(),
                sessions: shared.sessions.clone(),
                events: shared.events.clone(),
                bin: shared.bin.clone(),
                workdir: store_dir.join("propose"),
                config: shared.propose.clone(),
                account: account.clone(),
                env: env.clone(),
                retired: retired.clone(),
            })
        });
        let skill_turns = propose.as_ref().map(|ctx| spawn_worker(ctx.clone()));
        Arc::new(Self { account, index, memory, skills, propose, usage: shared.usage.clone(), turns, skill_turns, memory_notice, skills_notice, retired })
    }

    pub fn is_retired(&self) -> bool {
        self.retired.load(Ordering::SeqCst)
    }

    /// Marks the account as being removed; `false` puts it back (a removal that was refused).
    pub fn set_retired(&self, on: bool) {
        self.retired.store(on, Ordering::SeqCst);
    }
}
