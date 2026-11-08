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
use crate::index::Index;
use crate::memory::{recall_block, secret_hit, MemoryStore};
use crate::skills::SkillStore;
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
}

/// A tool call through which text from outside the person's own files and words can enter the chat: the web, a tool of another MCP
/// server, a subagent (it may have read the web), or a shell command that fetches (curl, wget, a URL).
pub fn is_external_tool(name: &str, input: &Value) -> bool {
    if matches!(name, "WebFetch" | "WebSearch" | "Task" | "Agent") || (name.starts_with("mcp__") && !name.starts_with("mcp__swb_app__")) {
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
}

#[derive(Debug, Clone, Serialize)]
pub struct Info {
    #[serde(flatten)]
    pub meta: Meta,
    #[serde(flatten)]
    pub state: State,
    pub pending: usize,
}

enum Cmd {
    Send { text: String, images: Vec<Image>, reply: oneshot::Sender<Result<(), String>> },
    Permission { request_id: String, allow: bool, message: Option<String>, remember: bool, reply: oneshot::Sender<Result<(), String>> },
    Interrupt { reply: oneshot::Sender<Result<(), String>> },
    Stop,
    Kill,
    UserTexts { after: u64, since: u64, reply: oneshot::Sender<Vec<(u64, String)>> },
    ProposalInput { since: u64, reply: oneshot::Sender<ProposalInput> },
    Subscribe { after: u64, reply: oneshot::Sender<(Vec<Envelope>, broadcast::Receiver<Envelope>)> },
    Info { reply: oneshot::Sender<Info> },
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
        ask(&self.tx, |reply| Cmd::Send { text, images, reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// `remember` asks Claude Code to allow this one read-only tool for the rest of this process; only the person's click sets it.
    pub async fn permission(&self, request_id: String, allow: bool, message: Option<String>, remember: bool) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::Permission { request_id, allow, message, remember, reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
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
}

pub struct Setup {
    pub meta: Meta,
    pub bin: String,
    pub dir: PathBuf,
    pub log: Vec<Envelope>,
    pub index: Option<Arc<Index>>,
    pub mcp: Option<Arc<McpGate>>,
    pub memory: Option<Arc<MemoryStore>>,
    /// Told when a turn ends well, so auto-save can read what the person wrote.
    pub turns: Option<mpsc::UnboundedSender<TurnEnd>>,
    pub skills: Option<Arc<SkillStore>>,
    /// Told the same, so an automatic skill proposal can be considered.
    pub skill_turns: Option<mpsc::UnboundedSender<TurnEnd>>,
    pub providers: Option<Arc<ProviderStore>>,
    /// Read when a container chat starts: the runtime and the image are the person's settings.
    pub settings: Option<Arc<crate::settings::SettingsStore>>,
    /// One tag per run of the app, part of every container's name.
    pub instance: String,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn new_meta(cwd: PathBuf, model: Option<String>, provider: Option<ProviderRef>, container: bool) -> Meta {
    Meta { sid: random_hex(8), cwd, model, claude_session_id: None, title: String::new(), created: now_ms(), provider, container }
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
    let mut actor = Actor { meta: setup.meta, bin: setup.bin, dir: setup.dir, log: setup.log, index: setup.index, mcp: setup.mcp, memory: setup.memory, turns: setup.turns, skills: setup.skills, skill_turns: setup.skill_turns, providers: setup.providers, settings: setup.settings, instance: setup.instance, secret: None, seq, bcast, state: State::Idle, process: None, pending: HashMap::new() };
    actor.settle_restored();
    tokio::spawn(actor.run(rx));
    SessionHandle { sid, tx }
}

struct Actor {
    meta: Meta,
    bin: String,
    dir: PathBuf,
    log: Vec<Envelope>,
    index: Option<Arc<Index>>,
    mcp: Option<Arc<McpGate>>,
    memory: Option<Arc<MemoryStore>>,
    turns: Option<mpsc::UnboundedSender<TurnEnd>>,
    skills: Option<Arc<SkillStore>>,
    skill_turns: Option<mpsc::UnboundedSender<TurnEnd>>,
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
        let data = self.scrub(data);
        self.seq += 1;
        let envelope = Envelope { v: 1, sid: self.meta.sid.clone(), seq: self.seq, ts: now_ms(), kind: kind.to_owned(), data };
        if let Ok(line) = serde_json::to_string(&envelope) {
            let _ = crate::private::append_line(&log_path(&self.dir, &self.meta.sid), &line);
        }
        if let Some(index) = &self.index {
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
        c::project_allowed(&real, &self.dir, std::env::var_os("HOME").map(PathBuf::from).as_deref(), std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).as_deref()).map_err(str::to_owned)?;
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
        let store = self.memory.clone()?;
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
            Cmd::Permission { request_id, allow, message, remember, reply } => {
                let _ = reply.send(self.permission(&request_id, allow, message, remember));
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
                let texts = self.log.iter().filter(|_| self.meta.provider.is_none()).filter(|e| e.kind == "user" && e.seq > after && e.ts >= since).filter_map(|e| e.data["text"].as_str().map(|t| (e.seq, t.to_owned()))).collect();
                let _ = reply.send(texts);
            }
            Cmd::Subscribe { after, reply } => {
                let missed: Vec<Envelope> = self.log.iter().filter(|e| e.seq > after).cloned().collect();
                let _ = reply.send((missed, self.bcast.subscribe()));
            }
            Cmd::Info { reply } => {
                let _ = reply.send(self.info());
            }
        }
        None
    }

    fn send(&mut self, text: String, images: Vec<Image>) -> (Result<(), String>, Option<mpsc::UnboundedReceiver<ProcEvent>>) {
        if self.state == State::Running {
            return (Err("a turn is already running".into()), None);
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

        let mut events = None;
        if self.process.is_none() {
            let provider = match self.current_provider() {
                Ok(provider) => provider,
                Err(message) => {
                    self.emit("error", json!({"message": message}));
                    return (Err(message), None);
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
                        return (Err(message), None);
                    }
                    container = match self.container_spec(provider, &config_dir) {
                        Ok(spec) => spec,
                        Err(message) => {
                            self.emit("error", json!({"message": message}));
                            return (Err(message), None);
                        }
                    };
                    let inherited: Vec<String> = std::env::vars_os().filter_map(|(name, _)| name.into_string().ok()).collect();
                    self.secret = provider.token.clone().filter(|token| token.len() >= 8);
                    (None, None, None, crate::claude::env::plan(&inherited, Some(provider), &config_dir), Some(provider.base_url.clone()), self.meta.model.clone().or_else(|| provider.model.clone()))
                }
                None if self.meta.container => {
                    let message = "a container chat needs a provider".to_owned();
                    self.emit("error", json!({"message": message}));
                    return (Err(message), None);
                }
                None => {
                    let mcp_config = match &self.mcp {
                        Some(gate) => match gate.grant(&self.meta.sid) {
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
                    (mcp_config, system_file, plugin_dir, Default::default(), None, self.meta.model.clone())
                }
            };
            let opts = SpawnOpts { bin: self.bin.clone(), cwd: self.meta.cwd.clone(), model, resume: self.meta.claude_session_id.clone(), mcp_config, system_file, plugin_dir, env, pinned_url, container };
            match ClaudeProcess::spawn(&opts) {
                Ok((process, rx)) => {
                    self.process = Some(process);
                    events = Some(rx);
                }
                Err(error) => {
                    if let Some(gate) = &self.mcp {
                        gate.revoke(&self.meta.sid);
                    }
                    let _ = std::fs::remove_file(self.recall_path());
                    let _ = std::fs::remove_dir_all(self.skills_path());
                    self.emit("error", json!({"message": format!("{error:#}")}));
                    return (Err(format!("{error:#}")), None);
                }
            }
        }

        if self.meta.title.is_empty() {
            self.meta.title = text.chars().take(60).collect();
            self.save_meta();
        }
        let shown: Vec<Value> = images.iter().map(|i| json!({"media_type": i.media_type, "data": i.data})).collect();
        self.emit("user", json!({"text": text, "images": shown}));
        let sent = self.process.as_ref().map(|p| p.send_line(protocol::user_message(&text, &images))).unwrap_or(false);
        if !sent {
            self.emit("error", json!({"message": "the claude process is gone; try again"}));
            self.process = None;
            return (Err("the claude process is gone".into()), None);
        }
        self.set_state(State::Running);
        (Ok(()), events)
    }

    fn permission(&mut self, request_id: &str, allow: bool, message: Option<String>, remember: bool) -> Result<(), String> {
        let Some(ask) = self.pending.remove(request_id) else {
            return Err("unknown or already answered".into());
        };
        // Only the one read-only tool of this app, only on a person's click, and Claude Code decides how long it holds (this process).
        let rules = (allow && remember && crate::mcp::is_read_only(&ask.tool_name))
            .then(|| json!([{"type": "addRules", "rules": [{"toolName": ask.tool_name}], "behavior": "allow", "destination": "session"}]));
        // A person allowed this exact note: the save that follows with the same input may be active at once (see mcp_host).
        if allow && ask.tool_name == crate::mcp::full_name(crate::mcp::MEMORY_SAVE) {
            if let Some(gate) = &self.mcp {
                gate.add_ticket(&self.meta.sid, &ask.input.to_string());
            }
        }
        let line = if allow { protocol::allow(request_id, &ask.input, rules) } else { protocol::deny(request_id, message.as_deref().unwrap_or("The user declined this.")) };
        let sent = self.process.as_ref().map(|p| p.send_line(line)).unwrap_or(false);
        self.emit("permission_resolved", json!({"request_id": request_id, "outcome": if allow { "allowed" } else { "denied" }}));
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
                let data = serde_json::to_value(&ask).unwrap_or(Value::Null);
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
                    if let Some(turns) = &self.turns {
                        let _ = turns.send(turn.clone());
                    }
                    if let Some(turns) = &self.skill_turns {
                        let _ = turns.send(turn);
                    }
                }
            }
            ProcEvent::Line(Event::ControlResponse { raw, .. }) => self.emit("claude", raw),
            ProcEvent::Line(Event::Raw(raw)) => {
                self.note_skill_use(&raw);
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

    /// A chat that calls one of this app's skills (the `Skill` tool with a `swb:` name) marks it as used.
    fn note_skill_use(&self, raw: &Value) {
        let Some(store) = &self.skills else { return };
        if raw["type"] != "assistant" || !raw["parent_tool_use_id"].is_null() {
            return;
        }
        for block in raw["message"]["content"].as_array().into_iter().flatten() {
            if block["type"] == "tool_use" && block["name"] == "Skill" {
                if let Some(name) = block["input"]["skill"].as_str().and_then(|s| s.strip_prefix("swb:")) {
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
        let store = self.skills.clone()?;
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
        for e in self.log.iter().filter(|e| e.ts >= since) {
            match e.kind.as_str() {
                "user" => {
                    if let Some(text) = e.data["text"].as_str() {
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
                                if !name.starts_with("mcp__swb_app__") {
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
        ProposalInput { cwd: self.meta.cwd.to_string_lossy().into_owned(), user_texts: texts, tool_lines: lines, last_turn_tools: turn_tools, external_used: external, last_seq: self.seq, provider: self.meta.provider.is_some() }
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

    fn exited(&mut self, code: Option<i32>) {
        self.cancel_pending();
        if let Some(gate) = &self.mcp {
            gate.revoke(&self.meta.sid);
        }
        let _ = std::fs::remove_file(self.recall_path());
        let _ = std::fs::remove_dir_all(self.skills_path());
        self.process = None;
        self.set_state(State::Exited { code });
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

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
