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
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn new_meta(cwd: PathBuf, model: Option<String>) -> Meta {
    Meta { sid: random_hex(8), cwd, model, claude_session_id: None, title: String::new(), created: now_ms() }
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
    let mut actor = Actor { meta: setup.meta, bin: setup.bin, dir: setup.dir, log: setup.log, index: setup.index, mcp: setup.mcp, memory: setup.memory, turns: setup.turns, skills: setup.skills, skill_turns: setup.skill_turns, seq, bcast, state: State::Idle, process: None, pending: HashMap::new() };
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
        let _ = self.bcast.send(Envelope { v: 1, sid: self.meta.sid.clone(), seq: self.seq, ts: now_ms(), kind: kind.to_owned(), data });
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
                let texts = self.log.iter().filter(|e| e.kind == "user" && e.seq > after && e.ts >= since).filter_map(|e| e.data["text"].as_str().map(|t| (e.seq, t.to_owned()))).collect();
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
            let opts = SpawnOpts { bin: self.bin.clone(), cwd: self.meta.cwd.clone(), model: self.meta.model.clone(), resume: self.meta.claude_session_id.clone(), mcp_config, system_file, plugin_dir };
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
                if subtype == "success" {
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
        let mut external = false;
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
                                external |= name == "WebFetch" || name == "WebSearch" || (name.starts_with("mcp__") && !name.starts_with("mcp__swb_app__"));
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
        ProposalInput { cwd: self.meta.cwd.to_string_lossy().into_owned(), user_texts: texts, tool_lines: lines, last_turn_tools: turn_tools, external_used: external, last_seq: self.seq }
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
