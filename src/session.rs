//! One conversation: a task that owns the `claude` process, the permission requests waiting for a person and the event log.
//!
//! The rule the whole host stands on: nothing is approved here. A permission request waits, without a timeout, until a
//! person answers it through `permission`, and the answer carries the request's own input back, never one a client sent.
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::claude::process::{ClaudeProcess, ProcEvent, SpawnOpts};
use crate::claude::protocol::{self, CanUseTool, Event, Image, IMAGE_TYPES, MAX_IMAGE_BYTES};
use crate::random_hex;

/// What the browser reads: a numbered event of one session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    pub sid: String,
    pub seq: u64,
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
    Permission { request_id: String, allow: bool, message: Option<String>, reply: oneshot::Sender<Result<(), String>> },
    Interrupt { reply: oneshot::Sender<Result<(), String>> },
    Stop,
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

    pub async fn permission(&self, request_id: String, allow: bool, message: Option<String>) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::Permission { request_id, allow, message, reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    pub async fn interrupt(&self) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::Interrupt { reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
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
    let actor = Actor { meta: setup.meta, bin: setup.bin, dir: setup.dir, log: setup.log, seq, bcast, state: State::Idle, process: None, pending: HashMap::new() };
    tokio::spawn(actor.run(rx));
    SessionHandle { sid, tx }
}

struct Actor {
    meta: Meta,
    bin: String,
    dir: PathBuf,
    log: Vec<Envelope>,
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
    }

    fn emit(&mut self, kind: &str, data: Value) {
        self.seq += 1;
        let envelope = Envelope { v: 1, sid: self.meta.sid.clone(), seq: self.seq, kind: kind.to_owned(), data };
        if let Ok(line) = serde_json::to_string(&envelope) {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path(&self.dir, &self.meta.sid)) {
                let _ = writeln!(file, "{line}");
            }
        }
        self.log.push(envelope.clone());
        let _ = self.bcast.send(envelope);
    }

    fn save_meta(&self) {
        if let Ok(text) = serde_json::to_string_pretty(&self.meta) {
            let _ = std::fs::write(meta_path(&self.dir, &self.meta.sid), text);
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
            Cmd::Permission { request_id, allow, message, reply } => {
                let _ = reply.send(self.permission(&request_id, allow, message));
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
            let opts = SpawnOpts { bin: self.bin.clone(), cwd: self.meta.cwd.clone(), model: self.meta.model.clone(), resume: self.meta.claude_session_id.clone() };
            match ClaudeProcess::spawn(&opts) {
                Ok((process, rx)) => {
                    self.process = Some(process);
                    events = Some(rx);
                }
                Err(error) => {
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

    fn permission(&mut self, request_id: &str, allow: bool, message: Option<String>) -> Result<(), String> {
        let Some(ask) = self.pending.remove(request_id) else {
            return Err("unknown or already answered".into());
        };
        let line = if allow { protocol::allow(request_id, &ask.input) } else { protocol::deny(request_id, message.as_deref().unwrap_or("The user declined this.")) };
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
            ProcEvent::Line(Event::Result { raw, .. }) => {
                self.emit("claude", raw);
                self.set_state(State::Idle);
            }
            ProcEvent::Line(Event::ControlResponse { raw, .. }) | ProcEvent::Line(Event::Raw(raw)) => self.emit("claude", raw),
            ProcEvent::Line(Event::Unparsed(text)) => self.emit("claude", json!({"type": "unparsed", "text": text})),
            ProcEvent::Stderr(text) => {
                if !text.trim().is_empty() {
                    self.emit("stderr", json!({"text": text}));
                }
            }
            ProcEvent::Exited(code) => {
                let waiting: Vec<String> = self.pending.drain().map(|(id, _)| id).collect();
                for request_id in waiting {
                    self.emit("permission_resolved", json!({"request_id": request_id, "outcome": "cancelled"}));
                }
                self.process = None;
                self.set_state(State::Exited { code });
            }
        }
    }
}
