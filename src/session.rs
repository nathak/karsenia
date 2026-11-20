//! One conversation: a task that owns the `claude` process, the permission requests waiting for a person and the event log.
//!
//! The rule the whole host stands on: nothing is approved here. A permission request waits, without a timeout, until a
//! person answers it through `permission`, and the answer carries the request's own input back, never one a client sent.
use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::claude::process::{ClaudeProcess, ProcEvent, SpawnOpts};
use crate::claude::protocol::{self, CanUseTool, Event, Image, IMAGE_TYPES, MAX_IMAGE_BYTES};
use crate::extract::TurnEnd;
use crate::accounts::AccountRef;
use crate::memory::{recall_block, secret_hit};
use crate::realm::Realm;
use crate::mcp::McpGate;
use crate::providers::{ProviderRef, ProviderStore};
use crate::random_hex;
use std::sync::Arc;

/// What the skill proposer may read of a chat: the person's messages, one-line summaries of tool calls that succeeded (never their
/// results), how many tools the last turn used, and whether third-party text could have entered (a web tool, an outside MCP tool).
#[derive(Debug, Clone, Default)]
pub struct ProposalInput {
    pub cwd: String,
    pub user_texts: Vec<(u64, String)>,
    pub tool_lines: Vec<String>,
    pub last_turn_tools: usize,
    pub external_used: bool,
    pub last_seq: u64,
    /// The chat runs against a provider the person chose: nothing of it is read by a proposal (that would send it to the account).
    pub provider: bool,
    /// The account the chat runs as: a proposal is read only by the worker of that account.
    pub account: Option<AccountRef>,
}

/// A tool call through which text from outside the person's own files and words can enter the chat: the web, a tool of another MCP
/// server, a subagent (it may have read the web), or a shell command that fetches (curl, wget, a URL).
pub fn is_external_tool(name: &str, input: &Value) -> bool {
    if matches!(name, "WebFetch" | "WebSearch" | "Task" | "Agent") || (name.starts_with("mcp__") && !name.starts_with(crate::mcp::TOOL_PREFIX)) {
        return true;
    }
    if name == "Bash" {
        let command = input["command"].as_str().unwrap_or_default().to_lowercase();
        return ["curl", "wget", "http://", "https://"].iter().any(|word| command.contains(word));
    }
    false
}

/// What the browser reads: a numbered event of one session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    pub sid: String,
    pub seq: u64,
    /// When it happened (ms since the epoch); logs written before this field have 0.
    #[serde(default)]
    pub ts: u64,
    pub kind: String,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum State {
    Idle,
    Running,
    Exited { code: Option<i32> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub sid: String,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub claude_session_id: Option<String>,
    pub title: String,
    pub created: u64,
    /// The provider this chat was made against; fixed for the chat's life. Chats made before providers existed read as None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderRef>,
    /// The chat's `claude` runs in a container (only possible with a provider; EXPERIMENTAL).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub container: bool,
    /// The chat works in its own git worktree (its `cwd`), made and removed by this app (see `worktree`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<crate::worktree::WorktreeRef>,
    /// The account this chat runs as, fixed for the chat's life. None is the default account (what chats made before accounts
    /// existed read as). Its notes, skills and history are the ones of that account's realm only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<AccountRef>,
    /// The `claude` command line session this chat was made from (its conversation was copied into the log): an id the CLI may
    /// replace on resume, so that "already imported" is judged by this and by `claude_session_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
    /// The permission mode the chat's `claude` starts with, chosen when the chat was made and fixed for its life. A chat made before
    /// modes existed reads as the default (no argument).
    #[serde(default, skip_serializing_if = "crate::claude::perm::PermMode::is_default")]
    pub permission_mode: crate::claude::perm::PermMode,
    /// `title` was given by a person (when the chat was made or by renaming): it is then also the CLI's session name (`--name`), and a
    /// first message does not replace it. False: the title is the first message's words (see `Meta::cli_name` for what the CLI is told).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub named: bool,
    /// The chat this one continues in another account (see `Registry::continue_as`) and the fingerprint of that chat's session file when
    /// this one was made: coming back is a fast-forward of that chat only while its file is still what it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handed: Option<Handed>,
}

impl Meta {
    /// The name the CLI is told (`--name`), which is the address another Claude Code session finds the chat by while its process runs:
    /// what a person named it, or, for a chat that has a conversation, its title (the first message's words). A chat with neither has no
    /// name, and none is invented.
    pub fn cli_name(&self) -> Option<String> {
        (self.named || (self.claude_session_id.is_some() && !self.title.is_empty())).then(|| self.title.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handed {
    pub parent: String,
    pub parent_hash: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Info {
    #[serde(flatten)]
    pub meta: Meta,
    #[serde(flatten)]
    pub state: State,
    pub pending: usize,
}

/// What a person who allows a request also asks Claude Code to remember.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// Nothing: this one call.
    None,
    /// This app's own read-only tool, for the rest of the process (what the "allow in this chat" button does).
    Remember,
    /// The rule Claude suggested, for the running process (destination `session`).
    Session,
    /// The rule Claude suggested, written by Claude Code into the project's `.claude/settings.local.json` (destination `localSettings`).
    Always,
}

enum Cmd {
    Send { text: String, images: Vec<Image>, reply: oneshot::Sender<Result<(), String>> },
    Permission { request_id: String, allow: bool, message: Option<String>, grant: Grant, reply: oneshot::Sender<Result<(), String>> },
    Interrupt { reply: oneshot::Sender<Result<(), String>> },
    SetModel { model: Option<String>, reply: oneshot::Sender<Result<(), String>> },
    Rename { title: String, reply: oneshot::Sender<Result<(), String>> },
    Remove { reply: oneshot::Sender<Result<(), String>> },
    Warm { reply: oneshot::Sender<Result<(), String>> },
    /// Ends the idle process (refused while a turn runs or a question waits): the session file is about to be replaced.
    Prepare { reply: oneshot::Sender<Result<(), String>> },
    /// Takes the events another chat of the same session made since (shown as imported) and the new lineage.
    Absorb { events: Vec<(u64, String, Value)>, handed: Handed, reply: oneshot::Sender<Result<(), String>> },
    Stop,
    Kill,
    UserTexts { after: u64, since: u64, reply: oneshot::Sender<Vec<(u64, String)>> },
    ProposalInput { since: u64, reply: oneshot::Sender<ProposalInput> },
    Subscribe { after: u64, reply: oneshot::Sender<(Vec<Envelope>, broadcast::Receiver<Envelope>)> },
    Info { reply: oneshot::Sender<Info> },
    /// Before a chat of an extra account starts its process: what is needed to ask the program whether the account is logged in.
    Preflight { reply: oneshot::Sender<Result<Option<Login>, String>> },
    /// About to remove this chat's worktree: refused while a turn runs; otherwise the process ends and nothing more is sent.
    Retire { reply: oneshot::Sender<Result<(), String>> },
    /// How removing it went: on success the chat is marked, on failure it can go on.
    Retired { ok: bool },
    /// The person accepted the git settings of this chat's project as they are now (see `worktree::fingerprint`).
    SetConfigHash { hash: String },
}

/// What it takes to ask `claude auth status` about a chat's account.
#[derive(Debug, Clone)]
pub struct Login {
    pub bin: String,
    pub dir: PathBuf,
    pub env: crate::claude::env::EnvPlan,
}

#[derive(Clone)]
pub struct SessionHandle {
    pub sid: String,
    tx: mpsc::UnboundedSender<Cmd>,
}

async fn ask<T>(tx: &mpsc::UnboundedSender<Cmd>, build: impl FnOnce(oneshot::Sender<T>) -> Cmd) -> Option<T> {
    let (reply, answer) = oneshot::channel();
    tx.send(build(reply)).ok()?;
    answer.await.ok()
}

impl SessionHandle {
    pub async fn send(&self, text: String, images: Vec<Image>) -> Result<(), String> {
        // A chat of an extra account that is about to start its process: the account must be logged in. The question is asked here and
        // not inside the chat's task, so a slow answer holds up nothing else of the chat.
        if let Some(login) = ask(&self.tx, |reply| Cmd::Preflight { reply }).await.unwrap_or_else(|| Err("the session is gone".into()))? {
            match crate::accounts::status(&login.bin, &login.dir, &login.env, Some(&login.dir)).await {
                Ok(status) if !status.logged_in => return Err("this account is not logged in; log in from the Accounts panel, then send again".into()),
                Ok(_) => {}
                Err(error @ crate::accounts::StatusError::Redirected) => return Err(error.message()),
                // Not being able to ask is not a reason to refuse: the program has no other login to use here, so it fails by itself.
                Err(error) => tracing::warn!("could not read the login status of an account: {}", error.message()),
            }
        }
        ask(&self.tx, |reply| Cmd::Send { text, images, reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// See `Cmd::Prepare`.
    pub async fn prepare(&self) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::Prepare { reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// See `Cmd::Absorb`.
    pub async fn absorb(&self, events: Vec<(u64, String, Value)>, handed: Handed) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::Absorb { events, handed, reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// Starts the chat's `claude` now (a chat the person named only: see `Actor::warm`), so that another Claude Code session can find
    /// it by its name before any message is sent. Nothing is sent to the model.
    pub async fn warm(&self) -> Result<(), String> {
        if let Some(login) = ask(&self.tx, |reply| Cmd::Preflight { reply }).await.unwrap_or_else(|| Err("the session is gone".into()))? {
            match crate::accounts::status(&login.bin, &login.dir, &login.env, Some(&login.dir)).await {
                Ok(status) if !status.logged_in => return Err("this account is not logged in".into()),
                Ok(_) => {}
                Err(error @ crate::accounts::StatusError::Redirected) => return Err(error.message()),
                Err(error) => tracing::warn!("could not read the login status of an account: {}", error.message()),
            }
        }
        ask(&self.tx, |reply| Cmd::Warm { reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// `remember` asks Claude Code to allow this one read-only tool for the rest of this process; only the person's click sets it.
    pub async fn permission(&self, request_id: String, allow: bool, message: Option<String>, remember: bool) -> Result<(), String> {
        self.answer(request_id, allow, message, if remember { Grant::Remember } else { Grant::None }).await
    }

    /// The answer to a permission request, with what the person also asked Claude Code to remember (see `Grant`).
    pub async fn answer(&self, request_id: String, allow: bool, message: Option<String>, grant: Grant) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::Permission { request_id, allow, message, grant, reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// Changes the model the chat runs with (None: Claude Code's default). Only between turns: the running `claude` is ended and the
    /// next message starts it again with `--resume` and the new `--model`, so the conversation goes on with the other model.
    pub async fn set_model(&self, model: Option<String>) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::SetModel { model, reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// Gives the chat a name (blank is refused; control characters are dropped; 60 characters at most). A first message does not replace it.
    pub async fn rename(&self, title: String) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::Rename { title, reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// Deletes the chat for good: its process, its log, its settings file and what it left in the run folder. Not while a turn runs
    /// or a question waits, and not while it has a worktree that is not removed (remove that first).
    pub async fn remove(&self) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::Remove { reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    pub async fn interrupt(&self) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::Interrupt { reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }

    /// What the skill proposer may read of this chat, from events at or after time `since` (ms).
    pub async fn proposal_input(&self, since: u64) -> Option<ProposalInput> {
        ask(&self.tx, |reply| Cmd::ProposalInput { since, reply }).await
    }

    /// What the person wrote after sequence number `after` and at or after time `since` (ms), for auto-save.
    pub async fn user_texts(&self, after: u64, since: u64) -> Option<Vec<(u64, String)>> {
        ask(&self.tx, |reply| Cmd::UserTexts { after, since, reply }).await
    }

    /// SIGKILL to the process group now (the host is exiting).
    pub fn kill(&self) {
        let _ = self.tx.send(Cmd::Kill);
    }

    /// The events after `after`, and a receiver of the ones to come; taken together so none is missed or doubled.
    pub async fn subscribe(&self, after: u64) -> Option<(Vec<Envelope>, broadcast::Receiver<Envelope>)> {
        ask(&self.tx, |reply| Cmd::Subscribe { after, reply }).await
    }

    pub async fn info(&self) -> Option<Info> {
        ask(&self.tx, |reply| Cmd::Info { reply }).await
    }

    /// Ends the process and stops new messages so the worktree can be removed; Err("busy") while a turn runs.
    pub async fn retire(&self) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::Retire { reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// Tells the chat whether its worktree was removed (marks it) or not (lets it go on).
    pub fn retired(&self, ok: bool) {
        let _ = self.tx.send(Cmd::Retired { ok });
    }

    pub fn set_config_hash(&self, hash: String) {
        let _ = self.tx.send(Cmd::SetConfigHash { hash });
    }
}

pub struct Setup {
    pub meta: Meta,
    pub bin: String,
    pub dir: PathBuf,
    pub log: Vec<Envelope>,
    /// The notes, skills and history of this chat's account. None for a provider chat (it gets nothing of the account's side) and
    /// for a chat whose account is not available.
    pub realm: Option<Arc<Realm>>,
    pub mcp: Option<Arc<McpGate>>,
    pub providers: Option<Arc<ProviderStore>>,
    /// Read when a container chat starts: the runtime and the image are the person's settings.
    pub settings: Option<Arc<crate::settings::SettingsStore>>,
    /// One tag per run of the app, part of every container's name.
    pub instance: String,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn new_meta(cwd: PathBuf, model: Option<String>, provider: Option<ProviderRef>, container: bool, account: Option<AccountRef>) -> Meta {
    Meta { sid: random_hex(8), cwd, model, claude_session_id: None, title: String::new(), created: now_ms(), provider, container, worktree: None, account, imported_from: None, permission_mode: Default::default(), named: false, handed: None }
}

pub fn log_path(dir: &std::path::Path, sid: &str) -> PathBuf {
    dir.join(format!("{sid}.jsonl"))
}

pub fn meta_path(dir: &std::path::Path, sid: &str) -> PathBuf {
    dir.join(format!("{sid}.meta.json"))
}

pub fn spawn(setup: Setup) -> SessionHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let sid = setup.meta.sid.clone();
    let (bcast, _) = broadcast::channel(1024);
    let seq = setup.log.last().map(|e| e.seq).unwrap_or(0);
    let mut actor = Actor { meta: setup.meta, bin: setup.bin, dir: setup.dir, log: setup.log, realm: setup.realm, mcp: setup.mcp, providers: setup.providers, settings: setup.settings, instance: setup.instance, secret: None, seq, bcast, state: State::Idle, process: None, pending: HashMap::new(), retiring: false, gone: false, native_spawned: false, retire_reply: None };
    actor.settle_restored();
    tokio::spawn(actor.run(rx));
    SessionHandle { sid, tx }
}

struct Actor {
    meta: Meta,
    bin: String,
    dir: PathBuf,
    log: Vec<Envelope>,
    realm: Option<Arc<Realm>>,
    mcp: Option<Arc<McpGate>>,
    providers: Option<Arc<ProviderStore>>,
    settings: Option<Arc<crate::settings::SettingsStore>>,
    instance: String,
    /// The key of this chat's provider while its process runs: whatever the chat shows or stores has it replaced.
    secret: Option<String>,
    seq: u64,
    bcast: broadcast::Sender<Envelope>,
    state: State,
    process: Option<ClaudeProcess>,
    pending: HashMap<String, CanUseTool>,
    /// Its worktree is being removed: nothing is sent until that is settled.
    retiring: bool,
    /// The chat was deleted: the loop ends and nothing more is written for it.
    gone: bool,
    /// Whether the running process was started with the built-in workbench on (its plug-in switched off, its guard hook set).
    native_spawned: bool,
    /// Who waits for the process to end before the worktree is removed.
    retire_reply: Option<oneshot::Sender<Result<(), String>>>,
}

impl Actor {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Cmd>) {
        let mut events: Option<mpsc::UnboundedReceiver<ProcEvent>> = None;
        loop {
            tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else { break };
                    if let Some(new_events) = self.command(command) {
                        events = Some(new_events);
                    }
                    if self.gone {
                        return;
                    }
                }
                event = async { match events.as_mut() { Some(rx) => rx.recv().await, None => std::future::pending().await } } => {
                    match event {
                        Some(event) => self.event(event),
                        None => events = None,
                    }
                }
            }
        }
        if let Some(process) = &self.process {
            process.stop();
        }
        if let Some(gate) = &self.mcp {
            gate.revoke(&self.meta.sid);
        }
    }

    fn emit(&mut self, kind: &str, data: Value) {
        self.emit_at(kind, data, now_ms());
    }

    /// As `emit`, with the time the event happened (an event another chat made, taken over).
    fn emit_at(&mut self, kind: &str, data: Value, ts: u64) {
        let data = self.scrub(data);
        self.seq += 1;
        let envelope = Envelope { v: 1, sid: self.meta.sid.clone(), seq: self.seq, ts, kind: kind.to_owned(), data };
        if let Ok(line) = serde_json::to_string(&envelope) {
            let _ = crate::private::append_line(&log_path(&self.dir, &self.meta.sid), &line);
        }
        if let Some(index) = self.realm.as_ref().and_then(|r| r.index.as_ref()) {
            index.add(&self.meta.cwd.to_string_lossy(), self.meta.created, &envelope);
        }
        self.log.push(envelope.clone());
        let _ = self.bcast.send(envelope);
    }

    /// For what only matters while it happens (streaming text pieces): sent to whoever is attached, never kept, never numbered.
    fn emit_transient(&self, kind: &str, data: Value) {
        let data = self.scrub(data);
        let _ = self.bcast.send(Envelope { v: 1, sid: self.meta.sid.clone(), seq: self.seq, ts: now_ms(), kind: kind.to_owned(), data });
    }

    /// A provider chat's key, wherever it turns up in what the chat shows or stores (an error, a command's output, a tool result), is
    /// replaced. It catches the key as one piece of text; a key printed in an encoded or split form is not recognised.
    fn scrub(&self, data: Value) -> Value {
        match &self.secret {
            Some(secret) => redact(data, secret),
            None => data,
        }
    }

    /// The provider this chat is bound to, as it is now: Err when it was deleted or its address changed (the chat was made for
    /// another party, so it does not start; a new chat can be made).
    fn current_provider(&self) -> Result<Option<crate::providers::Provider>, String> {
        let Some(bound) = &self.meta.provider else { return Ok(None) };
        let store = self.providers.as_ref().ok_or("providers are not available")?;
        let provider = store.get(&bound.id).ok_or("the provider of this chat was deleted; make a new chat")?;
        if crate::providers::host_of(&provider.base_url) != bound.host {
            return Err("the address of this chat's provider was changed; make a new chat".into());
        }
        Ok(Some(provider))
    }

    /// The settings folder and the environment of this chat's account: None for the default account. The folder is looked at again at
    /// every start (a link or a changed mode put there since is refused), and the chat must still have its realm: an account that was
    /// removed leaves the chat readable but not startable, and it never falls back to the default account.
    fn account_env(&self) -> Result<Option<(PathBuf, crate::claude::env::EnvPlan)>, String> {
        let Some(bound) = &self.meta.account else { return Ok(None) };
        if self.meta.provider.is_some() {
            return Err("a chat is of an account or of a provider, not both".into());
        }
        if !crate::accounts::valid_id(&bound.id) || self.realm.as_ref().and_then(|r| r.account.as_deref()) != Some(bound.id.as_str()) || self.realm.as_ref().is_some_and(|r| r.is_retired()) {
            return Err("this chat's account is not available; make a new chat".into());
        }
        let home = crate::platform::home_dir();
        let default = crate::accounts::default_config_dir(std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).as_deref(), home.as_deref());
        let real = crate::accounts::check_dir(&crate::accounts::config_dir(&self.dir, &bound.id), &self.dir.join("accounts"), default.as_deref())?;
        let env = crate::claude::env::account_plan(&crate::claude::env::inherited_names(), &real);
        Ok(Some((real, env)))
    }

    /// The container this chat starts in, when it is a container chat: needs the person's runtime and image, and an address the
    /// container can reach (its own `localhost` is not this computer's).
    fn container_spec(&self, provider: &crate::providers::Provider, config_dir: &std::path::Path) -> Result<Option<crate::claude::container::Spec>, String> {
        use crate::claude::container as c;
        if !self.meta.container {
            return Ok(None);
        }
        if crate::providers::is_local_host(&crate::providers::host_of(&provider.base_url)) {
            return Err("an address on this computer cannot be reached from inside a container".into());
        }
        // The folder is looked at again now: a link put in its place since the chat was made must not turn it into another folder.
        let real = self.meta.cwd.canonicalize().map_err(|e| format!("the project folder cannot be read: {e}"))?;
        if real != self.meta.cwd {
            return Err("the project folder is not where this chat was made (a link or a move); make a new chat".into());
        }
        c::project_allowed(&real, &self.dir, crate::platform::home_dir().as_deref(), std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).as_deref()).map_err(str::to_owned)?;
        let settings = self.settings.as_ref().map(|s| s.get()).unwrap_or_default();
        let program = c::validate_runtime(settings.container_runtime.as_deref().unwrap_or_default()).map_err(|e| format!("container settings: {e}"))?;
        let image = c::validate_image(settings.container_image.as_deref().unwrap_or_default()).map_err(|e| format!("container settings: {e}"))?;
        Ok(Some(c::Spec { runtime: c::Runtime { program, image }, name: c::container_name(&self.meta.sid, &self.instance, &random_hex(3)), data_hash: c::data_hash(&self.dir), config_dir: config_dir.to_path_buf() }))
    }

    fn recall_path(&self) -> PathBuf {
        self.dir.join("run").join(format!("{}.mem.txt", self.meta.sid))
    }

    /// The notes this chat starts with, written to a 0600 file for `--append-system-prompt-file` (never into argv).
    /// It is rebuilt at every start, so a resumed chat gets the notes as they are now.
    fn write_recall(&mut self) -> Option<PathBuf> {
        let store = self.realm.as_ref()?.memory.clone()?;
        let notes = store.recall(&self.meta.cwd.to_string_lossy());
        let block = recall_block(&notes)?;
        let path = self.recall_path();
        crate::private::create_dir(path.parent()?).ok()?;
        crate::private::write(&path, &block).ok()?;
        let ids: Vec<i64> = notes.iter().map(|n| n.id).collect();
        self.emit("memory_used", json!({"ids": ids, "bytes": block.len()}));
        Some(path)
    }

    /// A session read back from disk has no process: whatever the log left open (a waiting permission, a running turn) is over.
    fn settle_restored(&mut self) {
        let mut open: Vec<String> = Vec::new();
        let mut last_state: Option<String> = None;
        for e in &self.log {
            match e.kind.as_str() {
                "permission" => {
                    if let Some(id) = e.data["request_id"].as_str() {
                        open.push(id.to_owned());
                    }
                }
                "permission_resolved" => {
                    if let Some(id) = e.data["request_id"].as_str() {
                        open.retain(|x| x != id);
                    }
                }
                "state" => last_state = e.data["state"].as_str().map(str::to_owned),
                _ => {}
            }
        }
        for request_id in open {
            self.emit("permission_resolved", json!({"request_id": request_id, "outcome": "cancelled"}));
        }
        if last_state.as_deref() == Some("running") {
            self.set_state(State::Idle);
        }
    }

    /// Every request still waiting is dead (the turn ended or the process went).
    fn cancel_pending(&mut self) {
        let waiting: Vec<String> = self.pending.drain().map(|(id, _)| id).collect();
        for request_id in waiting {
            self.emit("permission_resolved", json!({"request_id": request_id, "outcome": "cancelled"}));
        }
    }

    fn save_meta(&self) {
        if let Ok(text) = serde_json::to_string_pretty(&self.meta) {
            let _ = crate::private::write(&meta_path(&self.dir, &self.meta.sid), &text);
        }
    }

    fn set_state(&mut self, state: State) {
        self.state = state.clone();
        let data = serde_json::to_value(&state).unwrap_or(Value::Null);
        self.emit("state", data);
    }

    fn info(&self) -> Info {
        Info { meta: self.meta.clone(), state: self.state.clone(), pending: self.pending.len() }
    }

    /// Handles one command; returns the event stream when it started a process.
    fn command(&mut self, command: Cmd) -> Option<mpsc::UnboundedReceiver<ProcEvent>> {
        match command {
            Cmd::Send { text, images, reply } => {
                let (result, events) = self.send(text, images);
                let _ = reply.send(result);
                return events;
            }
            Cmd::Permission { request_id, allow, message, grant, reply } => {
                let _ = reply.send(self.permission(&request_id, allow, message, grant));
            }
            Cmd::SetModel { model, reply } => {
                let (result, restarted) = self.set_model(model);
                let _ = reply.send(result);
                if restarted {
                    // The old process is gone: what it still had to say is not heard (a closed stream replaces its stream). A named chat
                    // gets its new process at once (see `warm`).
                    let (_result, events) = self.warm();
                    return Some(events.unwrap_or_else(|| mpsc::unbounded_channel().1));
                }
            }
            Cmd::Rename { title, reply } => {
                let clean: String = title.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(60).collect();
                let result = if clean.is_empty() {
                    Err("a name cannot be blank".to_owned())
                } else {
                    self.meta.title = clean.clone();
                    self.meta.named = true;
                    self.save_meta();
                    self.emit("renamed", json!({"title": clean}));
                    // A running-but-idle `claude` is ended so the next message starts it with `--resume` and the new `--name`.
                    if self.state != State::Running && self.pending.is_empty() && self.process.is_some() {
                        if let Some(process) = self.process.take() {
                            process.stop();
                        }
                        if let Some(gate) = &self.mcp {
                            gate.revoke(&self.meta.sid);
                        }
                        let _ = std::fs::remove_file(self.recall_path());
                        let _ = std::fs::remove_dir_all(self.skills_path());
                        self.set_state(State::Idle);
                        let _ = reply.send(Ok(()));
                        // The new name is the process's own: it starts again at once, so the name keeps pointing at a live chat.
                        let (_result, events) = self.warm();
                        return Some(events.unwrap_or_else(|| mpsc::unbounded_channel().1));
                    }
                    Ok(())
                };
                let named = result.is_ok();
                let _ = reply.send(result);
                if named {
                    let (_result, events) = self.warm();
                    return events;
                }
            }
            Cmd::Prepare { reply } => {
                if self.state == State::Running || !self.pending.is_empty() {
                    let _ = reply.send(Err("a turn is running or a permission waits in that chat".to_owned()));
                } else {
                    if let Some(process) = self.process.take() {
                        process.stop();
                        if let Some(gate) = &self.mcp {
                            gate.revoke(&self.meta.sid);
                        }
                        let _ = std::fs::remove_file(self.recall_path());
                        let _ = std::fs::remove_dir_all(self.skills_path());
                        self.set_state(State::Idle);
                    }
                    let _ = reply.send(Ok(()));
                }
            }
            Cmd::Absorb { events, handed, reply } => {
                for (ts, kind, mut data) in events {
                    data["imported"] = Value::Bool(true);
                    self.emit_at(&kind, data, ts);
                }
                self.meta.handed = Some(handed);
                self.save_meta();
                let _ = reply.send(Ok(()));
            }
            Cmd::Warm { reply } => {
                let (result, events) = self.warm();
                let _ = reply.send(result);
                return events;
            }
            Cmd::Remove { reply } => {
                let result = self.remove();
                let _ = reply.send(result);
            }
            Cmd::Interrupt { reply } => {
                let result = match (&self.state, &self.process) {
                    (State::Running, Some(process)) => {
                        process.send_line(protocol::interrupt(&random_hex(16)));
                        Ok(())
                    }
                    _ => Err("nothing is running".to_owned()),
                };
                let _ = reply.send(result);
            }
            Cmd::Stop => {
                if let Some(process) = &self.process {
                    process.stop();
                }
            }
            Cmd::Kill => {
                if let Some(process) = &self.process {
                    process.kill_now();
                }
            }
            Cmd::ProposalInput { since, reply } => {
                let _ = reply.send(self.proposal_input(since));
            }
            Cmd::UserTexts { after, since, reply } => {
                let texts = self.log.iter().filter(|_| self.meta.provider.is_none()).filter(|e| e.kind == "user" && e.seq > after && e.ts >= since && e.data["imported"] != true).filter_map(|e| e.data["text"].as_str().map(|t| (e.seq, t.to_owned()))).collect();
                let _ = reply.send(texts);
            }
            Cmd::Subscribe { after, reply } => {
                let missed: Vec<Envelope> = self.log.iter().filter(|e| e.seq > after).cloned().collect();
                let _ = reply.send((missed, self.bcast.subscribe()));
            }
            Cmd::Info { reply } => {
                let _ = reply.send(self.info());
            }
            Cmd::Preflight { reply } => {
                let answer = if self.process.is_some() {
                    Ok(None)
                } else {
                    self.account_env().map(|found| found.map(|(dir, env)| Login { bin: self.bin.clone(), dir, env }))
                };
                let _ = reply.send(answer);
            }
            Cmd::Retire { reply } => {
                if self.meta.worktree.is_none() {
                    let _ = reply.send(Err("this chat has no worktree".to_owned()));
                } else if self.state == State::Running || self.retire_reply.is_some() {
                    let _ = reply.send(Err("busy".to_owned()));
                } else {
                    self.retiring = true;
                    match &self.process {
                        // The answer waits for the process to be gone: a dying claude must not write into the folder being removed.
                        Some(process) => {
                            process.stop();
                            self.retire_reply = Some(reply);
                        }
                        None => {
                            let _ = reply.send(Ok(()));
                        }
                    }
                }
            }
            Cmd::Retired { ok } => {
                self.retiring = false;
                self.retire_reply = None;
                if ok {
                    if let Some(wt) = self.meta.worktree.as_mut() {
                        wt.removed = true;
                    }
                    self.save_meta();
                    // Every page that has this chat open hears it.
                    self.emit("worktree_removed", json!({}));
                }
            }
            Cmd::SetConfigHash { hash } => {
                if let Some(wt) = self.meta.worktree.as_mut() {
                    wt.config_hash = hash;
                }
                self.save_meta();
            }
        }
        None
    }

    /// A chat that has a name (one a person gave it, or its title once it has a conversation) keeps a `claude` running between
    /// messages, so its name is a live address: another Claude Code session finds it (and can message it) without a message having
    /// been sent first. No provider or container chat, nothing while a turn runs, and no cost: the process waits for its first message.
    fn warm(&mut self) -> (Result<(), String>, Option<mpsc::UnboundedReceiver<ProcEvent>>) {
        let held = self.process.is_some() || self.state == State::Running || self.gone || self.retiring || !self.pending.is_empty();
        let gone = self.realm.as_ref().is_some_and(|r| r.is_retired()) || self.meta.worktree.as_ref().is_some_and(|w| w.removed) || !self.meta.cwd.is_dir();
        if self.meta.cli_name().is_none() || self.meta.provider.is_some() || self.meta.container || held || gone {
            return (Ok(()), None);
        }
        match self.ensure_process() {
            Ok(events) => (Ok(()), events),
            Err(message) => (Err(message), None),
        }
    }

    /// Starts this chat's `claude` when none is running (resuming its session when it has one); Ok(Some) is the new process's event
    /// stream, Ok(None) means one was running already.
    fn ensure_process(&mut self) -> Result<Option<mpsc::UnboundedReceiver<ProcEvent>>, String> {
        let mut events = None;
        if self.process.is_none() {
            let provider = match self.current_provider() {
                Ok(provider) => provider,
                Err(message) => {
                    self.emit("error", json!({"message": message}));
                    return Err(message);
                }
            };
            // A provider chat gets nothing of the account's side of the app: no notes, no skills, no history search. What the
            // account's chats know must not reach a third party, and what the third party writes must not reach them.
            let mut container = None;
            let (mcp_config, system_file, plugin_dir, env, pinned_url, model) = match &provider {
                Some(provider) => {
                    let config_dir = self.dir.join("chatcfg").join(&self.meta.sid);
                    if let Err(error) = crate::private::create_dir(&self.dir.join("chatcfg")).and_then(|()| crate::private::create_dir(&config_dir)) {
                        let message = format!("could not make this chat's own settings folder: {error}");
                        self.emit("error", json!({"message": message}));
                        return Err(message);
                    }
                    container = match self.container_spec(provider, &config_dir) {
                        Ok(spec) => spec,
                        Err(message) => {
                            self.emit("error", json!({"message": message}));
                            return Err(message);
                        }
                    };
                    let inherited: Vec<String> = std::env::vars_os().filter_map(|(name, _)| name.into_string().ok()).collect();
                    self.secret = provider.token.clone().filter(|token| token.len() >= 8);
                    (None, None, None, crate::claude::env::plan(&inherited, Some(provider), &config_dir), Some(provider.base_url.clone()), self.meta.model.clone().or_else(|| provider.model.clone()))
                }
                None if self.meta.container => {
                    let message = "a container chat needs a provider".to_owned();
                    self.emit("error", json!({"message": message}));
                    return Err(message);
                }
                None => {
                    let env = match self.account_env() {
                        Ok(found) => found.map(|(_, env)| env).unwrap_or_default(),
                        Err(message) => {
                            self.emit("error", json!({"message": message}));
                            return Err(message);
                        }
                    };
                    let account = self.meta.account.as_ref().map(|a| a.id.clone());
                    let mcp_config = match &self.mcp {
                        Some(gate) => match gate.grant(&self.meta.sid, account.as_deref()) {
                            Ok(path) => Some(path),
                            Err(error) => {
                                self.emit("error", json!({"message": format!("history search is off for this chat: {error:#}")}));
                                None
                            }
                        },
                        None => None,
                    };
                    let system_file = self.write_recall();
                    let plugin_dir = self.write_skills();
                    (mcp_config, system_file, plugin_dir, env, None, self.meta.model.clone())
                }
            };
            // A chat whose guard cannot be set up does not start: running without it would be silent.
            let guard_hook = match self.guard_hook() {
                Ok(hook) => hook,
                Err(message) => {
                    self.emit("error", json!({"message": message}));
                    return Err(message);
                }
            };
            // The Stop hook goes where the guard hook goes (it does nothing unless the person turned the gate on in the workbench).
            let stop_hook = if guard_hook.is_some() { self.mcp.as_ref().and_then(|gate| crate::workbench::stop_command(&gate.exe, &self.dir, &self.meta.sid).ok()) } else { None };
            let opts = SpawnOpts { bin: self.bin.clone(), cwd: self.meta.cwd.clone(), model, resume: self.meta.claude_session_id.clone(), mcp_config, system_file, plugin_dir, env, pinned_url, container, permission_mode: self.meta.permission_mode, name: self.meta.cli_name(), plugins_off: self.workbench_native(), guard_hook, stop_hook };
            match ClaudeProcess::spawn(&opts) {
                Ok((process, rx)) => {
                    self.process = Some(process);
                    self.native_spawned = opts.plugins_off;
                    // What was started, for the debug view: the argument list (no key is ever in it), the environment with every credential
                    // hidden, and what was taken away. The key replacement of `emit` is a second guard.
                    let env: Vec<Value> = opts.env.describe().into_iter().map(|(name, shown)| json!([name, shown])).collect();
                    self.emit("spawn", json!({"bin": opts.bin, "cwd": opts.cwd, "args": crate::claude::process::args(&opts), "env": env, "env_removed": opts.env.remove, "container": opts.container.is_some()}));
                    events = Some(rx);
                }
                Err(error) => {
                    if let Some(gate) = &self.mcp {
                        gate.revoke(&self.meta.sid);
                    }
                    let _ = std::fs::remove_file(self.recall_path());
                    let _ = std::fs::remove_dir_all(self.skills_path());
                    self.emit("error", json!({"message": format!("{error:#}")}));
                    return Err(format!("{error:#}"));
                }
            }
        }
        Ok(events)
    }

    fn send(&mut self, text: String, images: Vec<Image>) -> (Result<(), String>, Option<mpsc::UnboundedReceiver<ProcEvent>>) {
        // Also with a process that is already running: the account was deleted, and a turn must not start under the deletion.
        if self.realm.as_ref().is_some_and(|r| r.is_retired()) {
            return (Err("this chat's account is not available; make a new chat".into()), None);
        }
        if self.state == State::Running {
            return (Err("a turn is already running".into()), None);
        }
        if let Some(wt) = &self.meta.worktree {
            if wt.removed || self.retiring {
                return (Err("this chat's worktree was removed; the chat can only be read".into()), None);
            }
            if !self.meta.cwd.is_dir() {
                return (Err("this chat's worktree folder is gone; remove the worktree from the chat's header".into()), None);
            }
        }
        if text.trim().is_empty() && images.is_empty() {
            return (Err("nothing to send".into()), None);
        }
        if images.len() > 5 {
            return (Err("at most five images per message".into()), None);
        }
        for image in &images {
            if !IMAGE_TYPES.contains(&image.media_type.as_str()) {
                return (Err(format!("{} is not an image type the host sends", image.media_type)), None);
            }
            if image.data.len() / 4 * 3 > MAX_IMAGE_BYTES {
                return (Err("an image is larger than 5 MB".into()), None);
            }
        }

        // The switch of the built-in workbench changed since this process started: it still has the old plug-in settings and no (or a stale)
        // guard hook, so it is ended and the message starts a new one.
        if self.process.is_some() && self.native_spawned != self.workbench_native() {
            if let Some(process) = self.process.take() {
                process.stop();
            }
            if let Some(gate) = &self.mcp {
                gate.revoke(&self.meta.sid);
            }
            let _ = std::fs::remove_file(self.recall_path());
            let _ = std::fs::remove_dir_all(self.skills_path());
        }
        let events = match self.ensure_process() {
            Ok(events) => events,
            Err(message) => return (Err(message), None),
        };

        // A command (`/swb team on`) is not what the chat is about: the next message names it.
        if self.meta.title.is_empty() && !is_slash(&text) {
            self.meta.title = text.chars().take(60).collect();
            self.save_meta();
        }
        let shown: Vec<Value> = images.iter().map(|i| json!({"media_type": i.media_type, "data": i.data})).collect();
        self.emit("user", json!({"text": text, "images": shown}));
        // What the person has set in the chat's workbench is added for the model; the transcript above shows only the person's words.
        let context = if self.workbench_native() { crate::workbench::inject::blocks(&self.dir, &self.meta.sid, &self.meta.cwd) } else { Vec::new() };
        let sent = self.process.as_ref().map(|p| p.send_line(protocol::user_message(&text, &images, &context))).unwrap_or(false);
        if !sent {
            self.emit("error", json!({"message": "the claude process is gone; try again"}));
            self.process = None;
            return (Err("the claude process is gone".into()), None);
        }
        self.set_state(State::Running);
        (Ok(()), events)
    }

    /// What a request may be remembered as, for this chat: the rules come from the request's own suggestion only (see `protocol::grants`).
    /// The command of the guard hook for this chat's process: the app's own binary with `guard`, the data folder and the chat. None when
    /// the workbench is off, when no binary is known (no MCP gate), or for a container chat (the binary is not in the container).
    fn guard_hook(&self) -> Result<Option<String>, String> {
        if !self.workbench_native() || self.meta.container {
            return Ok(None);
        }
        let Some(gate) = &self.mcp else { return Ok(None) };
        crate::workbench::hook_command(&gate.exe, &self.dir, &self.meta.cwd, &self.meta.sid).map(Some)
    }

    /// Whether this app's own workbench is on for this chat: the switch is on and the chat can have its guard hook (a container chat has no
    /// program of the app inside it, and without the app's own binary there is no hook). Otherwise the plug-in is left as it is, so a chat
    /// never ends up with neither.
    fn workbench_native(&self) -> bool {
        self.settings.as_ref().is_some_and(|s| s.get().workbench_native) && !self.meta.container && self.mcp.is_some()
    }

    /// A provider chat's or a container chat's rules are not written into the project (what such a chat wrote there outlives it and is
    /// read later by chats of the account), and a request that carries the undocumented `server_policy_ask_only` gets no rule at all.
    fn grants_for(&self, ask: &protocol::CanUseTool) -> protocol::Grants {
        if ask.server_policy_ask_only {
            return protocol::Grants::default();
        }
        let mut grants = protocol::grants(&ask.permission_suggestions, &ask.tool_name);
        if self.meta.provider.is_some() || self.meta.container {
            grants.always = None;
        }
        grants
    }

    fn permission(&mut self, request_id: &str, allow: bool, message: Option<String>, grant: Grant) -> Result<(), String> {
        let Some(ask) = self.pending.get(request_id).cloned() else {
            return Err("unknown or already answered".into());
        };
        // A rule to remember is checked before the request is taken off the list: a refusal leaves it waiting for a plain answer.
        let (rules, granted) = match (allow, grant) {
            (true, Grant::Session) => (self.grants_for(&ask).session.ok_or("there is no rule to remember for this request")?, Some("session")),
            (true, Grant::Always) => (self.grants_for(&ask).always.ok_or("this request cannot be remembered for the project")?, Some("always")),
            // Only the one read-only tool of this app, only on a person's click, and Claude Code decides how long it holds (this process).
            (true, Grant::Remember) if crate::mcp::is_read_only(&ask.tool_name) => (json!([{"type": "addRules", "rules": [{"toolName": ask.tool_name}], "behavior": "allow", "destination": "session"}]), None),
            _ => (Value::Null, None),
        };
        let rules = (!rules.is_null()).then_some(rules);
        self.pending.remove(request_id);
        // A person allowed this exact note: the save that follows with the same input may be active at once (see mcp_host).
        if allow && ask.tool_name == crate::mcp::full_name(crate::mcp::MEMORY_SAVE) {
            if let Some(gate) = &self.mcp {
                gate.add_ticket(&self.meta.sid, &ask.input.to_string());
            }
        }
        let line = if allow { protocol::allow(request_id, &ask.input, rules) } else { protocol::deny(request_id, message.as_deref().unwrap_or("The user declined this.")) };
        let sent = self.process.as_ref().map(|p| p.send_line(line)).unwrap_or(false);
        let mut resolved = json!({"request_id": request_id, "outcome": if allow { "allowed" } else { "denied" }});
        if let Some(granted) = granted {
            resolved["grant"] = json!(granted);
            resolved["rule"] = json!(self.grants_for(&ask).rule_text);
        }
        self.emit("permission_resolved", resolved);
        if sent {
            Ok(())
        } else {
            Err("the claude process is gone".into())
        }
    }

    fn event(&mut self, event: ProcEvent) {
        match event {
            ProcEvent::Line(Event::Init { session_id, raw, .. }) => {
                if self.meta.claude_session_id.as_deref() != Some(session_id.as_str()) {
                    self.meta.claude_session_id = Some(session_id);
                    self.save_meta();
                }
                self.emit("claude", raw);
            }
            ProcEvent::Line(Event::CanUseTool(ask)) => {
                let mut data = serde_json::to_value(&ask).unwrap_or(Value::Null);
                // What can be remembered for this request, for the dialogs to offer (they never build a rule themselves).
                let grants = self.grants_for(&ask);
                data["grants"] = json!({"session": grants.session.is_some().then(|| grants.rule_text.clone()).flatten(), "always": grants.always.is_some().then(|| grants.rule_text.clone()).flatten(), "exact": grants.exact});
                self.pending.insert(ask.request_id.clone(), ask);
                self.emit("permission", data);
            }
            ProcEvent::Line(Event::Result { raw, subtype, .. }) => {
                self.cancel_pending();
                self.emit("claude", raw);
                self.set_state(State::Idle);
                // A provider chat is never read by auto-save or skill proposals: their runs go to the account.
                if subtype == "success" && self.meta.provider.is_none() {
                    let turn = TurnEnd { sid: self.meta.sid.clone(), cwd: self.meta.cwd.to_string_lossy().into_owned() };
                    if let Some(realm) = &self.realm {
                        if let Some(turns) = &realm.turns {
                            let _ = turns.send(turn.clone());
                        }
                        if let Some(turns) = &realm.skill_turns {
                            let _ = turns.send(turn);
                        }
                    }
                }
            }
            ProcEvent::Line(Event::ControlResponse { raw, .. }) => self.emit("claude", raw),
            ProcEvent::Line(Event::Raw(raw)) => {
                self.note_skill_use(&raw);
                self.note_usage(&raw);
                self.raw_event(raw)
            }
            ProcEvent::Line(Event::Unparsed(text)) => self.emit("claude", json!({"type": "unparsed", "text": text})),
            ProcEvent::Stderr(text) => {
                if !text.trim().is_empty() {
                    self.emit("stderr", json!({"text": text}));
                }
            }
            ProcEvent::Exited(code) => self.exited(code),
        }
    }

    /// The limits a `rate_limit_event` of this chat's `claude` reports are kept for its account. A provider chat has no Anthropic
    /// limits, and a live line is the only way in (a replayed log or an imported session never reaches this).
    fn note_usage(&self, raw: &Value) {
        if raw["type"] != "rate_limit_event" || self.meta.provider.is_some() {
            return;
        }
        let Some(realm) = &self.realm else { return };
        if !realm.is_retired() {
            realm.usage.record(realm.account.as_deref(), &raw["rate_limit_info"], now_ms());
        }
    }

    /// A chat that calls one of this app's skills (the `Skill` tool with a `tenrec:` name) marks it as used.
    fn note_skill_use(&self, raw: &Value) {
        let Some(store) = self.realm.as_ref().and_then(|r| r.skills.as_ref()) else { return };
        if raw["type"] != "assistant" || !raw["parent_tool_use_id"].is_null() {
            return;
        }
        for block in raw["message"]["content"].as_array().into_iter().flatten() {
            if block["type"] == "tool_use" && block["name"] == "Skill" {
                if let Some(name) = block["input"]["skill"].as_str().and_then(|s| s.strip_prefix("tenrec:")) {
                    store.touch(name);
                }
            }
        }
    }

    fn skills_path(&self) -> PathBuf {
        self.dir.join("run").join(format!("{}.skills", self.meta.sid))
    }

    /// This chat's approved skills as a one-off plugin folder (files 0600, folders 0700), rebuilt at every start and removed when the
    /// process ends: a skill disabled or deleted since is simply not in it, and a running chat is never changed under its feet.
    fn write_skills(&mut self) -> Option<PathBuf> {
        let store = self.realm.as_ref()?.skills.clone()?;
        let path = self.skills_path();
        let names = store.materialize(&self.meta.cwd.to_string_lossy(), &path).ok()?;
        if names.is_empty() {
            return None;
        }
        self.emit("skills_given", json!({"names": names}));
        Some(path)
    }

    fn proposal_input(&self, since: u64) -> ProposalInput {
        let mut texts = Vec::new();
        let mut calls: Vec<(String, String)> = Vec::new();
        let mut failed: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut turn_tools = 0usize;
        // Outside text is looked for in the WHOLE log, subagents included, whenever it came: it stays in the chat's context.
        let external = self.log.iter().any(|e| {
            e.kind == "claude" && e.data["type"] == "assistant" && e.data["message"]["content"].as_array().into_iter().flatten().any(|b| b["type"] == "tool_use" && is_external_tool(b["name"].as_str().unwrap_or_default(), &b["input"]))
        });
        // What was imported from an earlier session (its messages, tool calls and results) is not this chat's own work.
        for e in self.log.iter().filter(|e| e.ts >= since && e.data["imported"] != true) {
            match e.kind.as_str() {
                "user" => {
                    // What was imported from an earlier session is not read for a skill proposal: it is not this chat's own work.
                    if let Some(text) = e.data["text"].as_str().filter(|_| e.data["imported"] != true) {
                        texts.push((e.seq, text.to_owned()));
                    }
                    turn_tools = 0;
                }
                "claude" if e.data["parent_tool_use_id"].is_null() => {
                    let blocks = e.data["message"]["content"].as_array();
                    match e.data["type"].as_str() {
                        Some("assistant") => {
                            for block in blocks.into_iter().flatten().filter(|b| b["type"] == "tool_use") {
                                let name = block["name"].as_str().unwrap_or_default();
                                turn_tools += 1;
                                if !name.starts_with(crate::mcp::TOOL_PREFIX) {
                                    calls.push((block["id"].as_str().unwrap_or_default().to_owned(), format!("{name}: {}", crate::index::summary(&block["input"]))));
                                }
                            }
                        }
                        Some("user") => {
                            for block in blocks.into_iter().flatten().filter(|b| b["type"] == "tool_result" && b["is_error"] == true) {
                                if let Some(id) = block["tool_use_id"].as_str() {
                                    failed.insert(id.to_owned());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        let mut lines: Vec<String> = calls.into_iter().filter(|(id, _)| !failed.contains(id)).map(|(_, line)| line.chars().take(160).collect::<String>()).filter(|line| !secret_hit(line)).collect();
        if lines.len() > 40 {
            lines.drain(..lines.len() - 40);
        }
        ProposalInput { cwd: self.meta.cwd.to_string_lossy().into_owned(), user_texts: texts, tool_lines: lines, last_turn_tools: turn_tools, external_used: external, last_seq: self.seq, provider: self.meta.provider.is_some(), account: self.meta.account.clone() }
    }

    fn raw_event(&mut self, raw: Value) {
        match raw["type"].as_str() {
            Some("stream_event") => self.emit_transient("claude", raw),
            Some("control_cancel_request") => {
                if let Some(request_id) = raw["request_id"].as_str().map(str::to_owned) {
                    if self.pending.remove(&request_id).is_some() {
                        self.emit("permission_resolved", json!({"request_id": request_id, "outcome": "cancelled"}));
                    }
                }
                self.emit("claude", raw);
            }
            _ => self.emit("claude", raw),
        }
    }

    /// See `SessionHandle::remove`. After it the actor stops (`gone`) and writes nothing more.
    fn remove(&mut self) -> Result<(), String> {
        if self.state == State::Running || !self.pending.is_empty() || self.retire_reply.is_some() || self.retiring {
            return Err("a chat can be deleted between turns, not while one runs or a question waits".into());
        }
        if self.meta.worktree.as_ref().is_some_and(|w| !w.removed) {
            return Err("this chat has a worktree: remove it from the chat's header first".into());
        }
        if let Some(process) = self.process.take() {
            process.stop();
        }
        if let Some(gate) = &self.mcp {
            gate.revoke(&self.meta.sid);
        }
        let sid = self.meta.sid.clone();
        if !sid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("not a chat id".into());
        }
        let _ = std::fs::remove_file(log_path(&self.dir, &sid));
        let _ = std::fs::remove_file(meta_path(&self.dir, &sid));
        // What it left in the run folder (`<sid>.mem.txt`, `<sid>.skills`, its MCP file...).
        if let Ok(entries) = std::fs::read_dir(self.dir.join("run")) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(&format!("{sid}.")) {
                    let path = entry.path();
                    let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
                }
            }
        }
        self.gone = true;
        Ok(())
    }

    /// See `SessionHandle::set_model`. The second value is true when a process was ended.
    fn set_model(&mut self, model: Option<String>) -> (Result<(), String>, bool) {
        if self.state == State::Running || !self.pending.is_empty() {
            return (Err("the model can be changed between turns, not while one runs or a question waits".into()), false);
        }
        if self.meta.provider.is_some() {
            return (Err("a provider chat's models are set by its provider".into()), false);
        }
        if self.realm.as_ref().is_some_and(|r| r.is_retired()) {
            return (Err("this chat's account is not available; make a new chat".into()), false);
        }
        if self.meta.worktree.as_ref().is_some_and(|w| w.removed) {
            return (Err("this chat's worktree was removed; the chat can only be read".into()), false);
        }
        if self.meta.model == model {
            return (Ok(()), false);
        }
        let had = self.process.take();
        let restarted = had.is_some();
        if let Some(process) = had {
            process.stop();
            if let Some(gate) = &self.mcp {
                gate.revoke(&self.meta.sid);
            }
            let _ = std::fs::remove_file(self.recall_path());
            let _ = std::fs::remove_dir_all(self.skills_path());
        }
        self.meta.model = model.clone();
        self.save_meta();
        self.emit("model", json!({"model": model}));
        self.set_state(State::Idle);
        (Ok(()), restarted)
    }

    fn exited(&mut self, code: Option<i32>) {
        self.cancel_pending();
        if let Some(gate) = &self.mcp {
            gate.revoke(&self.meta.sid);
        }
        let _ = std::fs::remove_file(self.recall_path());
        let _ = std::fs::remove_dir_all(self.skills_path());
        self.process = None;
        self.set_state(State::Exited { code });
        if let Some(reply) = self.retire_reply.take() {
            let _ = reply.send(Ok(()));
        }
    }
}

/// `secret` replaced by a marker in every string of `value`.
fn redact(value: Value, secret: &str) -> Value {
    match value {
        Value::String(text) if text.contains(secret) => Value::String(text.replace(secret, "[provider key]")),
        Value::Array(items) => Value::Array(items.into_iter().map(|v| redact(v, secret)).collect()),
        Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, redact(v, secret))).collect()),
        other => other,
    }
}

/// A command for Claude Code or a plug-in (`/swb team on`, `/plugin:cmd`), not a topic: `/` and a word, then a space or the
/// end. A path (`/home/me/a.rs fix this`) is a topic.
pub fn is_slash(text: &str) -> bool {
    let Some(rest) = text.trim_start().strip_prefix('/') else { return false };
    let word: &str = rest.split(char::is_whitespace).next().unwrap_or_default();
    !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_not_a_topic() {
        assert!(is_slash("/swb team on") && is_slash("  /cost"));
        assert!(is_slash("/swb") && is_slash("/plugin:cmd arg"));
        assert!(!is_slash("look at /etc/hosts") && !is_slash("") && !is_slash("/") && !is_slash("/home/me/a.rs fix this"));
    }

    #[test]
    fn a_key_is_replaced_in_every_string_however_deep_and_other_values_are_left_alone() {
        let data = json!({"a": "x sk-secret-1234 y", "n": 5, "list": ["sk-secret-1234", {"deep": "no key here", "more": "sk-secret-1234sk-secret-1234"}], "ok": true});
        let out = redact(data, "sk-secret-1234").to_string();
        assert!(!out.contains("sk-secret-1234"), "{out}");
        assert!(out.contains("[provider key]") && out.contains("no key here") && out.contains("\"n\":5"));
    }

    #[test]
    fn a_chat_made_before_providers_existed_reads_as_a_normal_chat_and_a_normal_chat_writes_no_provider_field() {
        let old = r#"{"sid":"a1","cwd":"/tmp","model":null,"claude_session_id":null,"title":"t","created":1}"#;
        let meta: Meta = serde_json::from_str(old).unwrap();
        assert!(meta.provider.is_none());
        assert!(!serde_json::to_string(&meta).unwrap().contains("provider"));
        assert!(meta.worktree.is_none(), "an older chat is a plain chat, not a worktree chat");
        assert!(!serde_json::to_string(&meta).unwrap().contains("worktree"));
    }
}
