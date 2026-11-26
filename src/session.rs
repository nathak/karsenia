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
use crate::claude::protocol::{self, CanUseTool, Event, Image};
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
/// Whether what was said through a gateway is read by a proposal: an automatic one never reads it; a manual one does, once the person
/// has confirmed that it goes to their Claude account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gateway {
    Exclude,
    Include,
}

#[derive(Debug, Clone, Default)]
pub struct ProposalInput {
    pub cwd: String,
    pub user_texts: Vec<(u64, String)>,
    pub tool_lines: Vec<String>,
    pub last_turn_tools: usize,
    pub external_used: bool,
    pub last_seq: u64,
    /// The chat runs against a provider the person chose: nothing of it is read by a proposal (that would send it to the account).
    /// A gateway that uses an account's login (`login`) is one too, but a manual proposal may read it after a confirmation.
    pub provider: bool,
    /// The provider takes the account's login (a gateway) and is on now.
    pub login: bool,
    /// Something in the log was said while a gateway was on.
    pub via_gateway: bool,
    /// The account the chat runs as: a proposal is read only by the worker of that account.
    pub account: Option<AccountRef>,
}

impl ProposalInput {
    /// Gateway text is, or would be, in what a manual proposal reads (a chat on a provider that is not a gateway is never read at all).
    pub fn gateway_text(&self) -> bool {
        !(self.provider && !self.login) && (self.login || self.via_gateway)
    }
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
    /// Put away by the person: left out of the chat list unless the archive is shown (its conversation, search entries and notes are kept).
    /// Only set between turns, and cleared by a message sent to the chat.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    /// The chat this one continues in another account (see `Registry::continue_as`) and the fingerprint of that chat's session file when
    /// this one was made: coming back is a fast-forward of that chat only while its file is still what it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handed: Option<Handed>,
    /// The command names the program listed in its last `init` (skills, plug-in and custom commands, built-ins) minus the ones that only
    /// work in a terminal: the "/" menu offers them. Empty until the first message of the chat has started a process.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slash_commands: Vec<String>,
    /// What the person decided about the risky parts of this project's own Claude Code configuration (see `project_check`); None: nothing
    /// was asked yet (or nothing risky was there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_cfg: Option<ProjectChoice>,
}

/// The decision about a project's own configuration, kept per chat (as the git settings of a worktree are).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectChoice {
    /// The configuration was seen as it is (`hash` of its risky parts, and `strong`, the SHA-256 of the same material) and accepted; a change of
    /// those parts asks again. A trust written before `strong` existed has none and is held to `hash` alone.
    Trusted {
        hash: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        strong: Option<String>,
    },
    /// The chat's `claude` reads only the person's own settings, never the project's.
    Without,
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
    /// Changes the permission mode of the chat (its next process starts with it; a running idle one is told over the control channel).
    /// `SetModel` by ending the process (the way that always works).
    SetModelSlow { model: Option<String>, reply: oneshot::Sender<Result<(), String>> },
    /// The person's decision about the project's configuration the last `project_check` event showed (`hash`), or to leave it out.
    /// `remember`: also trust this configuration for new chats of the project (only for a decision to trust, never to leave it out).
    ProjectDecide { hash: String, without: bool, remember: bool, reply: oneshot::Sender<Result<(), String>> },
    SetPermMode { mode: crate::claude::perm::PermMode, reply: oneshot::Sender<Result<(), String>> },
    /// Puts the chat away (or brings it back); refused while a turn runs or a question waits.
    Archive { on: bool, reply: oneshot::Sender<Result<(), String>> },
    /// Stops one background job of the chat's process (ids come from `background_tasks_changed`).
    StopTask { task_id: String, reply: oneshot::Sender<Result<(), String>> },
    /// A control request got no answer in time.
    CtlExpired { id: String },
    /// The chat's team settings were changed (in its workbench file): a running idle process is ended so the next message starts it with them.
    TeamChanged { reply: oneshot::Sender<Result<(), String>> },
    /// Turns the gateway of an account's chat on (a provider that uses an account's login) or off (None), between turns.
    SetProvider { provider: Option<String>, reply: oneshot::Sender<Result<(), String>> },
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
    ProposalInput { since: u64, gateway: Gateway, reply: oneshot::Sender<ProposalInput> },
    Subscribe { after: u64, reply: oneshot::Sender<(Vec<Envelope>, broadcast::Receiver<Envelope>)> },
    Info { reply: oneshot::Sender<Info> },
    /// The end of the CLI's own debug log of this chat (see `Settings::debug_api`), with the provider key replaced.
    DebugLog { reply: oneshot::Sender<Option<String>> },
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

    /// Tells the chat that its team settings changed: between turns the running `claude` is ended and started again with the new ones
    /// (`--resume`), and the transcript says so; during a turn the change applies from the message after it.
    pub async fn team_changed(&self) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::TeamChanged { reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// Answers a `project_check` event: trust the configuration as it was shown (refused when it changed since), or start without it.
    pub async fn project_decide(&self, hash: String, without: bool, remember: bool) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::ProjectDecide { hash, without, remember, reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// Puts the chat away from the list, or brings it back. Putting away is refused while a turn runs or a question waits, so a chat that needs
    /// the person is never hidden.
    pub async fn set_archived(&self, on: bool) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::Archive { on, reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// Changes the permission mode of the chat; between turns only.
    pub async fn set_permission_mode(&self, mode: crate::claude::perm::PermMode) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::SetPermMode { mode, reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// Stops one background job of the chat.
    pub async fn stop_task(&self, task_id: String) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::StopTask { task_id, reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// Changes the model the chat runs with (None: Claude Code's default). Only between turns: the running `claude` is ended and the
    /// next message starts it again with `--resume` and the new `--model`, so the conversation goes on with the other model.
    pub async fn set_model(&self, model: Option<String>) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::SetModel { model, reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
    }

    /// Switches the gateway of a chat that belongs to an account: to a provider that uses an account's login, or off (None). Only between
    /// turns: the running `claude` is ended and the next message starts it again with `--resume` in the same account folder, so the
    /// conversation goes on, with or without the gateway. The chat keeps its own account.
    pub async fn set_provider(&self, provider: Option<String>) -> Result<(), String> {
        ask(&self.tx, |reply| Cmd::SetProvider { provider, reply }).await.unwrap_or_else(|| Err("the session is gone".into()))
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
        self.proposal_input_with(since, Gateway::Exclude).await
    }

    /// As `proposal_input`, saying whether what was said through a gateway is read too.
    pub async fn proposal_input_with(&self, since: u64, gateway: Gateway) -> Option<ProposalInput> {
        ask(&self.tx, |reply| Cmd::ProposalInput { since, gateway, reply }).await
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

    /// The end of the CLI's debug log of this chat, when it writes one (None: it does not, or nothing was written).
    pub async fn debug_log(&self) -> Option<String> {
        ask(&self.tx, |reply| Cmd::DebugLog { reply }).await.flatten()
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
    /// Where the cost of each turn is added (see `costs`).
    pub costs: Option<Arc<crate::costs::CostStore>>,
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
    /// What the person trusted in projects' own configuration for new chats (None: nothing is ever remembered).
    pub trust: Option<Arc<crate::project_trust::TrustStore>>,
    /// One tag per run of the app, part of every container's name.
    pub instance: String,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Meta {
    /// The project this chat belongs to: the folder a worktree was made from, else the chat's own folder (as the sidebar groups chats). Not
    /// stored; worked out where it is needed. `cwd` itself is what the chat runs in and is never changed by this.
    pub fn project_dir(&self) -> PathBuf {
        let dir = self.worktree.as_ref().map_or(&self.cwd, |w| &w.repo);
        dir.to_str().map_or_else(|| dir.clone(), |s| PathBuf::from(crate::platform::strip_verbatim(s)))
    }

    /// `project_dir` as the text the notes and skills of a project are filed under (the `\\?\` of Windows is left out, so a plain chat and a
    /// worktree chat of one project file under the same text).
    pub fn project_key(&self) -> String {
        self.project_dir().to_string_lossy().into_owned()
    }
}

pub fn new_meta(cwd: PathBuf, model: Option<String>, provider: Option<ProviderRef>, container: bool, account: Option<AccountRef>) -> Meta {
    Meta { sid: random_hex(8), cwd, model, claude_session_id: None, title: String::new(), created: now_ms(), provider, container, worktree: None, account, imported_from: None, permission_mode: Default::default(), named: false, archived: false, handed: None, slash_commands: Vec::new(), project_cfg: None }
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
    let mut actor = Actor { meta: setup.meta, bin: setup.bin, dir: setup.dir, log: setup.log, realm: setup.realm, mcp: setup.mcp, providers: setup.providers, settings: setup.settings, trust: setup.trust, shown_strong: None, instance: setup.instance, secrets: Vec::new(), cli_key: None, log_failed: false, init_seen: false, spawned_resume: None, last_stderr: String::new(), seq, bcast, state: State::Idle, process: None, pending: HashMap::new(), retiring: false, gone: false, native_spawned: false, team_spawned: None, bg_live: 0, costs: setup.costs, cost_base: None, cost_alt: None, cost_chat: 0.0, retire_reply: None, ctl: HashMap::new(), me: tx.downgrade() };
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
    trust: Option<Arc<crate::project_trust::TrustStore>>,
    /// `Scan::strong` of what the last `project_check` event showed.
    shown_strong: Option<String>,
    instance: String,
    /// The key of this chat's provider while its process runs: whatever the chat shows or stores has it replaced.
    /// The key and the subscription token of this chat's provider: replaced wherever the chat would show or store them.
    secrets: Vec<String>,
    /// Which `claude` program the chat last started (see `cli_version::key`), so its `init` can say what version it is.
    cli_key: Option<String>,
    /// The last write of the chat's log failed (and the person was told).
    log_failed: bool,
    /// The program of this chat sent its `init` (a program that ends before it, having been told to resume, has lost the conversation).
    init_seen: bool,
    /// The conversation the running program was told to resume, if any.
    spawned_resume: Option<String>,
    /// What the program wrote on stderr, from its start.
    last_stderr: String,
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
    /// The team settings the running process was started with (None: team mode was off).
    team_spawned: Option<crate::workbench::team::Team>,
    /// How many background jobs of the chat the program last said it had (ambient ones left out): they live in its process, so a
    /// restart of the process ends them, and the chat says so.
    bg_live: usize,
    /// Where the cost of each turn is added.
    costs: Option<Arc<crate::costs::CostStore>>,
    /// The running total of the program's last result (the baseline the next turn's cost is the difference from); None for a chat that
    /// came from a session whose spend we never saw.
    cost_base: Option<f64>,
    /// Where the baseline stood before a session was taken back from another account (see `Cmd::Absorb`): the first result after it may
    /// have restored either that total or the returned session's, and is read against the one that fits.
    cost_alt: Option<f64>,
    /// What this chat has cost so far.
    cost_chat: f64,
    /// Who waits for the process to end before the worktree is removed.
    retire_reply: Option<oneshot::Sender<Result<(), String>>>,
    /// Control requests sent to the process and not answered yet, by request id.
    ctl: HashMap<String, Ctl>,
    /// The chat's own command channel (weak: it must not keep the chat alive once every handle is gone): a control request that gets no
    /// answer expires through it.
    me: mpsc::WeakUnboundedSender<Cmd>,
}

/// What waits for the answer of a control request.
enum Ctl {
    SetModel { model: Option<String>, reply: oneshot::Sender<Result<(), String>> },
    PermMode { mode: crate::claude::perm::PermMode, reply: oneshot::Sender<Result<(), String>> },
    StopTask { reply: oneshot::Sender<Result<(), String>> },
}

/// How long a control request may wait for its answer before the slower path is taken.
const CTL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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
        let mut data = data;
        // What was said while a gateway was on is marked: the history index, auto-save and automatic skill proposals leave it out (the third party's
        // text is not carried over to the account's own notes); a skill proposal the person asks for reads it only after they confirm it. Whichever way the chat is switched later.
        if self.meta.provider.as_ref().is_some_and(|p| p.login) && matches!(kind, "user" | "claude") {
            if let Value::Object(map) = &mut data {
                map.insert("via_gateway".into(), Value::Bool(true));
            }
        }
        // The big parts of a tool's structured result (a whole file for every edit) are not logged, indexed or sent on.
        if kind == "claude" {
            protocol::slim_tool_result(&mut data);
        }
        let data = self.scrub(data);
        self.seq += 1;
        let envelope = Envelope { v: 1, sid: self.meta.sid.clone(), seq: self.seq, ts, kind: kind.to_owned(), data };
        let written = serde_json::to_string(&envelope).map_err(std::io::Error::other).and_then(|line| crate::private::append_line(&log_path(&self.dir, &self.meta.sid), &line));
        if let Some(index) = self.realm.as_ref().and_then(|r| r.index.as_ref()) {
            index.add(&self.meta.cwd.to_string_lossy(), self.meta.created, &envelope);
        }
        self.log.push(envelope.clone());
        let _ = self.bcast.send(envelope);
        // A log that cannot be written is said once, when it starts (not per event: the say-so is an event itself and fails the same way).
        match written {
            Ok(()) => self.log_failed = false,
            Err(error) if !self.log_failed => {
                self.log_failed = true;
                self.emit("error", json!({"message": format!("the chat's log could not be written: {error}")}));
            }
            Err(_) => {}
        }
    }

    /// For what only matters while it happens (streaming text pieces): sent to whoever is attached, never kept, never numbered.
    fn emit_transient(&self, kind: &str, data: Value) {
        let data = self.scrub(data);
        let _ = self.bcast.send(Envelope { v: 1, sid: self.meta.sid.clone(), seq: self.seq, ts: now_ms(), kind: kind.to_owned(), data });
    }

    /// A provider chat's key, wherever it turns up in what the chat shows or stores (an error, a command's output, a tool result), is
    /// replaced. It catches the key as one piece of text; a key printed in an encoded or split form is not recognised.
    fn scrub(&self, data: Value) -> Value {
        self.secrets.iter().fold(data, |data, secret| redact(data, secret))
    }

    fn debug_path(&self) -> PathBuf {
        self.dir.join("run").join(format!("{}.debug.log", self.meta.sid))
    }

    /// The file the CLI is to write its debug log to, when the person turned that on: made empty here, so a log is always of one process.
    /// Not for a container chat (the path is on this side of it).
    fn debug_file(&self) -> Option<PathBuf> {
        if self.meta.container || !self.settings.as_ref().is_some_and(|s| s.get().debug_api) {
            return None;
        }
        let path = self.debug_path();
        crate::private::create_dir(&self.dir.join("run")).ok()?;
        crate::private::write(&path, "").ok()?;
        Some(path)
    }

    /// The last part of the debug log (whole lines), with the provider key replaced; None when there is no log.
    fn debug_tail(&self) -> Option<String> {
        const KEEP: usize = 256 * 1024;
        let bytes = std::fs::read(self.debug_path()).ok()?;
        let cut = bytes.len().saturating_sub(KEEP);
        let text = String::from_utf8_lossy(&bytes[cut..]).into_owned();
        // A cut in the middle of a line leaves half of it: that line goes.
        let text = if cut > 0 { text.split_once('\n').map(|(_, rest)| rest.to_owned()).unwrap_or_default() } else { text };
        let scrubbed = self.scrub(Value::String(text));
        scrubbed.as_str().map(str::to_owned).filter(|t| !t.is_empty())
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
        if bound.login && provider.auth != crate::providers::Auth::AccountLogin {
            return Err("this chat's provider no longer uses an account's login; turn the gateway off in the chat's header".into());
        }
        Ok(Some(provider))
    }

    /// The settings folder and the environment of this chat's account: None for the default account. The folder is looked at again at
    /// every start (a link or a changed mode put there since is refused), and the chat must still have its realm: an account that was
    /// removed leaves the chat readable but not startable, and it never falls back to the default account.
    fn account_env(&self) -> Result<Option<(PathBuf, crate::claude::env::EnvPlan)>, String> {
        let Some(bound) = &self.meta.account else { return Ok(None) };
        // Only a provider that uses an account's login may go with an account.
        if self.meta.provider.as_ref().is_some_and(|p| !p.login) {
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
        let project = self.meta.project_key();
        // A note saved before worktree chats shared their project's notes is filed under the worktree's folder: it moves to the project.
        if self.meta.worktree.is_some() {
            store.rehome(&self.meta.cwd.to_string_lossy(), &project);
        }
        let notes = store.recall(&project);
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
        // Whether the program last said something was running in the background.
        let mut background = false;
        for e in &self.log {
            match e.kind.as_str() {
                "claude" => match e.data["subtype"].as_str() {
                    Some("background_tasks_changed") => background = e.data["tasks"].as_array().is_some_and(|t| !t.is_empty()),
                    _ => {}
                },
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
        // The baseline of the cost: the last number this chat's own log holds (events imported from another chat are not its own).
        self.restore_cost();
        for request_id in open {
            self.emit("permission_resolved", json!({"request_id": request_id, "outcome": "cancelled"}));
        }
        if last_state.as_deref() == Some("running") {
            self.set_state(State::Idle);
        }
        // The process that ran those jobs is gone with the app: the chat must not show that it waits for them.
        if background {
            self.emit("claude", json!({"type": "system", "subtype": "background_tasks_changed", "tasks": []}));
        }
    }

    /// Sets where the cost counts from, out of the log: the last `cost` event (the program's running total and the chat's sum); with none
    /// (a log from before costs were kept) the last result's total, with the chat's sum unknown (0); with neither, a chat that came from
    /// a session we never saw has no baseline (its first result sets it, counting nothing) and any other starts from nothing.
    fn restore_cost(&mut self) {
        let own = |e: &&Envelope| e.data["imported"] != true;
        let last_cost = self.log.iter().filter(own).rev().find(|e| e.kind == "cost");
        if let Some(e) = last_cost {
            self.cost_base = e.data["total"].as_f64();
            self.cost_chat = e.data["chat"].as_f64().unwrap_or(0.0);
            return;
        }
        let last_result = self.log.iter().filter(own).rev().find(|e| e.kind == "claude" && e.data["type"] == "result" && e.data["total_cost_usd"].is_number());
        self.cost_base = match last_result {
            Some(e) => e.data["total_cost_usd"].as_f64(),
            None if self.meta.imported_from.is_some() => None,
            None => Some(0.0),
        };
        // A log from before costs were kept: the program's last total is the best figure for what the chat has cost.
        self.cost_chat = self.cost_base.unwrap_or(0.0);
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
                // A chat that is being used is not put away.
                if self.meta.archived {
                    self.set_archive_flag(false);
                }
                // `/model name` is done by the app itself (the header and the saved model stay right), not left to the program.
                if images.is_empty() {
                    if let crate::view::slash::Routed::Model(name) = crate::view::slash::route(&text) {
                        if let Some(why) = self.model_refusal() {
                            let _ = reply.send(Err(why.into()));
                            return None;
                        }
                        self.emit("user", json!({"text": text, "images": [], "files": []}));
                        return self.command(Cmd::SetModel { model: Some(name), reply });
                    }
                }
                let (result, events) = self.send(text, images);
                let _ = reply.send(result);
                return events;
            }
            Cmd::Permission { request_id, allow, message, grant, reply } => {
                let _ = reply.send(self.permission(&request_id, allow, message, grant));
            }
            Cmd::SetProvider { provider, reply } => {
                let (result, restarted) = self.set_provider(provider);
                let _ = reply.send(result);
                if restarted {
                    let (_result, events) = self.warm();
                    return Some(events.unwrap_or_else(|| mpsc::unbounded_channel().1));
                }
            }
            Cmd::TeamChanged { reply } => {
                let (result, restarted) = self.team_changed();
                let _ = reply.send(result);
                if restarted {
                    let (_result, events) = self.warm();
                    return Some(events.unwrap_or_else(|| mpsc::unbounded_channel().1));
                }
            }
            Cmd::SetModel { model, reply } => {
                // A running idle process takes the new model over the control channel: no restart, the conversation goes on at once.
                if model.is_some() && self.meta.model != model && self.process.is_some() && self.model_refusal().is_none() {
                    let name = model.clone().unwrap_or_default();
                    match self.control(|id| protocol::set_model(id, &name), Ctl::SetModel { model, reply }) {
                        Ok(()) => return None,
                        Err(Ctl::SetModel { model, reply }) => return self.command(Cmd::SetModelSlow { model, reply }),
                        Err(_) => return None,
                    }
                }
                return self.command(Cmd::SetModelSlow { model, reply });
            }
            Cmd::SetPermMode { mode, reply } => self.set_permission_mode(mode, reply),
            Cmd::Archive { on, reply } => {
                let result = if on && (self.state == State::Running || !self.pending.is_empty() || self.bg_live > 0) {
                    Err("a turn is running, a question waits or background work is going on: archive the chat when it is idle".to_owned())
                } else {
                    self.set_archive_flag(on);
                    Ok(())
                };
                let _ = reply.send(result);
            }
            Cmd::ProjectDecide { hash, without, remember, reply } => {
                let _ = reply.send(self.project_decide(&hash, without, remember));
            }
            Cmd::StopTask { task_id, reply } => {
                if let Err(Ctl::StopTask { reply }) = self.control(|id| protocol::stop_task(id, &task_id), Ctl::StopTask { reply }) {
                    let _ = reply.send(Err("no process runs for this chat".into()));
                }
            }
            Cmd::CtlExpired { id } => {
                if let Some(waiting) = self.ctl.remove(&id) {
                    self.ctl_done(waiting, Err("no answer in time".into()));
                }
            }
            Cmd::SetModelSlow { model, reply } => {
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
                        self.forget_background();
                        self.ctl_drop();
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
                    self.forget_background();
                    self.ctl_drop();
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
                // The session that comes back has been used elsewhere: its running total includes that spend, which was counted under the
                // other account when it happened. The baseline moves to where that session stands now, so the next turn costs only itself.
                let mut stands: Option<f64> = None;
                for (_, kind, data) in &events {
                    let total = match kind.as_str() {
                        "cost" => data["total"].as_f64(),
                        "claude" if data["type"] == "result" => data["total_cost_usd"].as_f64(),
                        _ => None,
                    };
                    stands = total.filter(|t| t.is_finite() && *t >= 0.0).or(stands);
                }
                for (ts, kind, mut data) in events {
                    data["imported"] = Value::Bool(true);
                    self.emit_at(&kind, data, ts);
                }
                if let Some(total) = stands {
                    self.cost_alt = self.cost_base;
                    self.cost_base = Some(total);
                    // Written down as the chat's own (not imported) so a restart restores the same baseline; nothing is added to the ledger.
                    self.emit("cost", json!({"turn": 0.0, "total": total, "chat": self.cost_chat}));
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
            Cmd::ProposalInput { since, gateway, reply } => {
                let _ = reply.send(self.proposal_input(since, gateway));
            }
            Cmd::UserTexts { after, since, reply } => {
                let texts = self.log.iter().filter(|_| self.meta.provider.is_none()).filter(|e| e.kind == "user" && e.seq > after && e.ts >= since && e.data["imported"] != true && e.data["via_gateway"] != true).filter_map(|e| e.data["text"].as_str().map(|t| (e.seq, t.to_owned()))).collect();
                let _ = reply.send(texts);
            }
            Cmd::Subscribe { after, reply } => {
                let missed: Vec<Envelope> = self.log.iter().filter(|e| e.seq > after).cloned().collect();
                let _ = reply.send((missed, self.bcast.subscribe()));
            }
            Cmd::Info { reply } => {
                let _ = reply.send(self.info());
            }
            Cmd::DebugLog { reply } => {
                let _ = reply.send(self.debug_tail());
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
            if provider.is_none() {
                self.project_gate()?;
            }
            // A provider chat gets nothing of the account's side of the app: no notes, no skills, no history search. What the
            // account's chats know must not reach a third party, and what the third party writes must not reach them.
            let mut container = None;
            let (mcp_config, system_file, plugin_dir, env, pinned_url, model) = match &provider {
                Some(provider) => {
                    let inherited: Vec<String> = std::env::vars_os().filter_map(|(name, _)| name.into_string().ok()).collect();
                    let env = if provider.auth == crate::providers::Auth::AccountLogin {
                        // The login of one of the person's accounts: the chat runs in that account's folder, in no container of its own (the
                        // login would have to be inside it), and still gets nothing of the account's notes, skills or history.
                        if self.meta.container {
                            let message = "a provider that uses an account's login cannot run in a container".to_owned();
                            self.emit("error", json!({"message": message}));
                            return Err(message);
                        }
                        let account_dir = if self.meta.provider.as_ref().is_some_and(|p| p.login) {
                            // A chat that belongs to an account: that account's folder (None is the default account), looked at again as for any
                            // chat of an account.
                            match self.account_env() {
                                Ok(found) => found.map(|(dir, _)| dir),
                                Err(message) => {
                                    self.emit("error", json!({"message": message}));
                                    return Err(message);
                                }
                            }
                        } else {
                            match provider.login_account.as_deref() {
                                None => None,
                                Some(id) => {
                                    let home = crate::platform::home_dir();
                                    let default = crate::accounts::default_config_dir(std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).as_deref(), home.as_deref());
                                    let found = if crate::accounts::valid_id(id) { crate::accounts::check_dir(&crate::accounts::config_dir(&self.dir, id), &self.dir.join("accounts"), default.as_deref()) } else { Err("not a valid account".to_owned()) };
                                    match found {
                                        Ok(dir) => Some(dir),
                                        Err(why) => {
                                            let message = format!("the account whose login this provider uses is not available ({why}); change the provider or make a new chat");
                                            self.emit("error", json!({"message": message}));
                                            return Err(message);
                                        }
                                    }
                                }
                            }
                        };
                        crate::claude::env::login_plan(&inherited, provider, account_dir.as_deref())
                    } else {
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
                        crate::claude::env::plan(&inherited, Some(provider), &config_dir)
                    };
                    // The program's own version (from an earlier `init` of the same program) lets the app's name follow its User-Agent.
                    let cli_key = crate::claude::cli_version::key(&self.bin, container.as_ref());
                    let env = crate::claude::env::with_user_agent(env, Some(provider), crate::claude::cli_version::known(&cli_key).as_deref());
                    self.secrets = provider.secrets();
                    (None, None, None, env, Some(provider.base_url.clone()), self.meta.model.clone().or_else(|| provider.model.clone()))
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
            let debug_file = self.debug_file();
            let team = self.team_now();
            let (agents, effort) = (team.as_ref().map(crate::workbench::team::agents_json), team.as_ref().and_then(|t| t.lead_effort).map(|e| e.as_str().to_owned()));
            self.cli_key = Some(crate::claude::cli_version::key(&self.bin, container.as_ref()));
            let opts = SpawnOpts { bin: self.bin.clone(), cwd: self.meta.cwd.clone(), model, resume: self.meta.claude_session_id.clone(), mcp_config, system_file, plugin_dir, env, pinned_url, container, permission_mode: self.meta.permission_mode, name: self.meta.cli_name(), plugins_off: self.plugins_off(), guard_hook, stop_hook, debug_file, agents, effort, user_settings_only: matches!(self.meta.project_cfg, Some(ProjectChoice::Without)) };
            match ClaudeProcess::spawn(&opts) {
                Ok((process, rx)) => {
                    self.process = Some(process);
                    self.native_spawned = opts.plugins_off;
                    self.team_spawned = team;
                    // A process that does not resume a session counts its cost from nothing.
                    if opts.resume.is_none() {
                        self.cost_base = Some(0.0);
                    }
                    // What was started, for the debug view: the argument list (no key is ever in it), the environment with every credential
                    // hidden, and what was taken away. The key replacement of `emit` is a second guard.
                    let env: Vec<Value> = opts.env.describe().into_iter().map(|(name, shown)| json!([name, shown])).collect();
                    self.emit("spawn", json!({"bin": opts.bin, "cwd": opts.cwd, "args": crate::claude::process::args(&opts), "env": env, "env_removed": opts.env.remove, "container": opts.container.is_some()}));
                    self.init_seen = false;
                    self.spawned_resume = opts.resume.clone();
                    self.last_stderr.clear();
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
        // The files are checked before anything is started or written: what is wrong with them is told as it is.
        let checked = match crate::attachments::check(&images, self.meta.container) {
            Ok(checked) => checked,
            Err(message) => return (Err(message), None),
        };

        // The switch of the built-in workbench changed since this process started: it still has the old plug-in settings and no (or a stale)
        // guard hook, so it is ended and the message starts a new one.
        if self.process.is_some() && (self.native_spawned != self.plugins_off() || self.team_spawned != self.team_now()) {
            self.forget_background();
            self.ctl_drop();
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
        // Saved files are written only now that the process is there; the event holds the images (for thumbnails) and, of the other
        // files, only name, size and kind.
        let (parts, shown, files) = match crate::attachments::materialize(&self.dir, &self.meta.sid, checked) {
            Ok(made) => made,
            Err(message) => {
                self.emit("error", json!({"message": message}));
                // The process was started above: its event stream goes back to the actor, or nothing would read what it says (the chat
                // would stay running for good).
                return (Err(message), events);
            }
        };
        self.emit("user", json!({"text": text, "images": shown, "files": files}));
        // What the person has set in the chat's workbench is added for the model; the transcript above shows only the person's words.
        // A command (`/compact`, `/cost`, a skill or plug-in command) goes alone: the program runs it only when it is the message's only
        // block, and one more text block after it makes the whole message a prompt for the model (measured with claude 2.1.326: the model
        // then answers that it cannot run the command, at the cost of a model call). With a text or other file attached the file's block
        // follows the command, so it is a prompt anyway and keeps the blocks (pictures and PDFs go before the text: not measured).
        let alone = parts.is_empty() && is_slash(&text);
        let context = if !alone && self.workbench_native() { crate::workbench::inject::blocks(&self.dir, &self.meta.sid, &self.meta.cwd, self.team_spawned.is_some()) } else { Vec::new() };
        let sent = self.process.as_ref().map(|p| p.send_line(protocol::user_message(&text, &parts, &context))).unwrap_or(false);
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
    /// program of the app inside it, and without the app's own binary there is no hook). Where the plug-in is not forced off (not the
    /// desktop app) it is then left as it is, so a chat never ends up with neither; in the desktop app a chat that cannot have the hook
    /// runs without the plug-in and without the guard.
    fn workbench_native(&self) -> bool {
        self.settings.as_ref().is_some_and(|s| s.native()) && !self.meta.container && self.mcp.is_some()
    }

    /// Whether this chat's `claude` runs without the SmartWorkbench plug-in: always in the desktop app (the plug-in is never used there),
    /// else when the app's own workbench is on for the chat.
    fn plugins_off(&self) -> bool {
        crate::settings::native_forced() || self.workbench_native()
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
                self.init_seen = true;
                if let (Some(key), Some(version)) = (&self.cli_key, crate::claude::cli_version::from_init(&raw)) {
                    crate::claude::cli_version::remember(key, version);
                }
                let mut changed = false;
                if self.meta.claude_session_id.as_deref() != Some(session_id.as_str()) {
                    self.meta.claude_session_id = Some(session_id);
                    changed = true;
                }
                let names = crate::view::slash::init_names(&raw);
                if !names.is_empty() && names != self.meta.slash_commands {
                    self.meta.slash_commands = names;
                    changed = true;
                }
                if changed {
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
                // The cost of the turn first: the transcript reads it when it meets the result.
                self.note_cost(&raw);
                self.emit("claude", raw);
                self.set_state(State::Idle);
                // A provider chat is never read by auto-save or skill proposals: their runs go to the account.
                if subtype == "success" && self.meta.provider.is_none() {
                    let turn = TurnEnd { sid: self.meta.sid.clone(), cwd: self.meta.project_key() };
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
            ProcEvent::Line(Event::ControlResponse { request_id, raw }) => match self.ctl.remove(&request_id) {
                Some(waiting) => self.ctl_done(waiting, protocol::control_outcome(&raw)),
                None => self.emit("claude", raw),
            },
            ProcEvent::Line(Event::UnhandledControl { request_id, subtype, raw }) => {
                // The program waits for an answer to this, for good: it is refused (the app approves nothing it cannot show) and the person
                // is told what was asked, so a turn never hangs in silence when a newer program asks for something new.
                if let (Some(id), Some(process)) = (&request_id, self.process.as_ref()) {
                    process.send_line(protocol::control_error(id, "not handled by this app"));
                }
                self.emit("claude", raw);
                self.emit("error", json!({"message": format!("Claude Code asked for '{subtype}', which this app does not handle; it was refused"), "control": subtype}));
            }
            ProcEvent::Line(Event::Raw(raw)) => {
                self.note_skill_use(&raw);
                self.note_usage(&raw);
                self.raw_event(raw)
            }
            ProcEvent::Line(Event::Unparsed(text)) => self.emit("claude", json!({"type": "unparsed", "text": text})),
            ProcEvent::Stderr(text) => {
                if self.last_stderr.len() < 4000 {
                    self.last_stderr.push_str(&text);
                }
                if !text.trim().is_empty() {
                    self.emit("stderr", json!({"text": text}));
                }
            }
            ProcEvent::Exited(code) => self.exited(code),
        }
    }

    /// The limits a `rate_limit_event` of this chat's `claude` reports are kept for its account. A provider chat has no Anthropic
    /// limits, and a live line is the only way in (a replayed log or an imported session never reaches this).
    /// The cost of the turn a live result ends: the difference from the result before (see `costs::turn_cost`), told in the chat's log as
    /// numbers (`cost`: the turn, the running total of the program, what the chat has cost) and added to the day's sum of the account (or,
    /// for a chat of a provider, of the provider: its cost is not the account's). Only live results come here: a replay never counts twice.
    fn note_cost(&mut self, raw: &Value) {
        let crate::costs::Step::Count { mut delta, total } = crate::costs::turn_cost(self.cost_base, raw) else { return };
        // After a session came back from another account the program may have restored the total it had before (not the returned
        // session's): a total below the moved baseline that fits the old one is that turn's difference from the old one.
        if let (Some(base), Some(alt)) = (self.cost_base, self.cost_alt) {
            if total < base && total >= alt {
                delta = total - alt;
            }
        }
        self.cost_alt = None;
        self.cost_base = Some(total);
        self.cost_chat += delta;
        self.emit("cost", json!({"turn": delta, "total": total, "chat": self.cost_chat}));
        if delta <= 0.0 || self.realm.as_ref().is_some_and(|r| r.is_retired()) {
            return;
        }
        let key = match &self.meta.provider {
            Some(p) => format!("p:{}", p.id),
            None => self.meta.account.as_ref().map(|a| a.id.clone()).unwrap_or_default(),
        };
        if let Some(costs) = &self.costs {
            costs.add(&key, &crate::costs::local_day(), delta);
        }
    }

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
        // A skill approved before worktree chats shared their project's skills is filed under the worktree's folder: it moves to the project.
        if self.meta.worktree.is_some() {
            store.rehome(&self.meta.cwd.to_string_lossy(), &self.meta.project_key());
        }
        let names = store.materialize(&self.meta.project_key(), &path).ok()?;
        if names.is_empty() {
            return None;
        }
        self.emit("skills_given", json!({"names": names}));
        Some(path)
    }

    fn proposal_input(&self, since: u64, gateway: Gateway) -> ProposalInput {
        let mut texts = Vec::new();
        let mut calls: Vec<(String, String)> = Vec::new();
        let mut failed: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut turn_tools = 0usize;
        // Outside text is looked for in the WHOLE log, subagents included, whenever it came: it stays in the chat's context.
        // Text a gateway wrote stays in the chat's context after a switch as well: it counts as outside text.
        let external = self.log.iter().any(|e| e.data["via_gateway"] == true) || self.log.iter().any(|e| {
            e.kind == "claude" && e.data["type"] == "assistant" && e.data["message"]["content"].as_array().into_iter().flatten().any(|b| b["type"] == "tool_use" && is_external_tool(b["name"].as_str().unwrap_or_default(), &b["input"]))
        });
        // What was imported from an earlier session (its messages, tool calls and results) is not this chat's own work.
        for e in self.log.iter().filter(|e| e.ts >= since && e.data["imported"] != true && (gateway == Gateway::Include || e.data["via_gateway"] != true)) {
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
        ProposalInput { cwd: self.meta.project_key(), user_texts: texts, tool_lines: lines, last_turn_tools: turn_tools, external_used: external, last_seq: self.seq, provider: self.meta.provider.is_some(), login: self.meta.provider.as_ref().is_some_and(|p| p.login), via_gateway: self.log.iter().any(|e| e.data["via_gateway"] == true), account: self.meta.account.clone() }
    }

    /// The model speaks with no message of ours before it (a background job ended and the program told it, so it goes on by itself): the
    /// chat is running again, and shows so until that turn's result.
    fn wake(&mut self) {
        if self.state == State::Idle && self.process.is_some() {
            self.set_state(State::Running);
        }
    }

    /// The jobs the program has in the background went with their process (it was ended to be started again): said in the transcript, and
    /// the list is cleared, so the chat does not go on waiting for them.
    fn forget_background(&mut self) {
        if self.bg_live > 0 {
            let n = self.bg_live;
            self.bg_live = 0;
            self.emit("claude", json!({"type": "system", "subtype": "background_tasks_changed", "tasks": []}));
            self.emit("bg_lost", json!({"n": n}));
        }
    }

    fn raw_event(&mut self, raw: Value) {
        // A frame of a sub-agent (it names its parent call) is not the model going on by itself.
        let main = raw.get("parent_tool_use_id").is_none_or(Value::is_null);
        match raw["type"].as_str() {
            Some("stream_event") => {
                if main {
                    self.wake();
                }
                self.emit_transient("claude", raw)
            }
            Some("assistant") => {
                if main {
                    self.wake();
                }
                self.emit("claude", raw)
            }
            Some("control_cancel_request") => {
                if let Some(request_id) = raw["request_id"].as_str().map(str::to_owned) {
                    if self.pending.remove(&request_id).is_some() {
                        self.emit("permission_resolved", json!({"request_id": request_id, "outcome": "cancelled"}));
                    }
                }
                self.emit("claude", raw);
            }
            Some("system") if raw["subtype"] == "background_tasks_changed" => {
                self.bg_live = raw["tasks"].as_array().map_or(0, |t| t.iter().filter(|t| t["ambient"].as_bool() != Some(true)).count());
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
        self.ctl_drop();
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

    /// The team settings this chat's next process gets: the chat's workbench file says team mode is on and the app's own workbench is the
    /// one in use. A provider chat that does not use an account's login is left out (its provider decides what it can run).
    fn team_now(&self) -> Option<crate::workbench::team::Team> {
        if !self.workbench_native() || self.meta.provider.as_ref().is_some_and(|p| !p.login) {
            return None;
        }
        crate::workbench::store::load(&self.dir, &self.meta.sid).ok().map(|wb| wb.team).filter(|t| t.on)
    }

    /// See `SessionHandle::team_changed`. The second value is true when a process was ended.
    fn team_changed(&mut self) -> (Result<(), String>, bool) {
        if self.state == State::Running || !self.pending.is_empty() || self.process.is_none() || self.team_spawned == self.team_now() {
            return (Ok(()), false);
        }
        self.forget_background();
        self.ctl_drop();
        if let Some(process) = self.process.take() {
            process.stop();
        }
        if let Some(gate) = &self.mcp {
            gate.revoke(&self.meta.sid);
        }
        let _ = std::fs::remove_file(self.recall_path());
        let _ = std::fs::remove_dir_all(self.skills_path());
        let summary = self.team_now().map(|t| crate::workbench::team::summary(&t)).unwrap_or_else(|| "off".into());
        self.emit("team", json!({"summary": summary}));
        self.set_state(State::Idle);
        (Ok(()), true)
    }

    fn model_refusal(&self) -> Option<&'static str> {
        if self.state == State::Running || !self.pending.is_empty() {
            return Some("the model can be changed between turns, not while one runs or a question waits");
        }
        if self.meta.provider.as_ref().is_some_and(|p| !p.login) {
            return Some("a provider chat's models are set by its provider");
        }
        if self.realm.as_ref().is_some_and(|r| r.is_retired()) {
            return Some("this chat's account is not available; make a new chat");
        }
        if self.meta.worktree.as_ref().is_some_and(|w| w.removed) {
            return Some("this chat's worktree was removed; the chat can only be read");
        }
        None
    }

    /// Sends a control request to the process and remembers what waits for its answer; false when there is no process to ask.
    fn control(&mut self, build: impl FnOnce(&str) -> String, waiting: Ctl) -> Result<(), Ctl> {
        let id = random_hex(16);
        let sent = self.process.as_ref().map(|p| p.send_line(build(&id))).unwrap_or(false);
        if !sent {
            return Err(waiting);
        }
        self.ctl.insert(id.clone(), waiting);
        let me = self.me.clone();
        tokio::spawn(async move {
            tokio::time::sleep(CTL_TIMEOUT).await;
            if let Some(me) = me.upgrade() {
                let _ = me.send(Cmd::CtlExpired { id });
            }
        });
        Ok(())
    }

    /// The answer of a control request (or its failure: a refusal, no answer in time, the process ended).
    fn ctl_done(&mut self, waiting: Ctl, outcome: Result<Value, String>) {
        match waiting {
            Ctl::SetModel { model, reply } => match outcome {
                Ok(_) => {
                    self.meta.model = model.clone();
                    self.save_meta();
                    self.emit("model", json!({"model": model, "live": true}));
                    let _ = reply.send(Ok(()));
                }
                Err(why) => {
                    // The slower way, which always works: the process is ended and the next message starts it with the new model.
                    // It goes back through the command channel so the restart is handled like any other (the old process's remaining
                    // events are dropped with its stream, and a named chat gets its new process at once).
                    tracing::info!("set_model was not taken live ({why}); restarting");
                    match self.me.upgrade() {
                        Some(me) => {
                            if let Err(back) = me.send(Cmd::SetModelSlow { model, reply }) {
                                if let Cmd::SetModelSlow { reply, .. } = back.0 {
                                    let _ = reply.send(Err("the session is gone".into()));
                                }
                            }
                        }
                        None => {
                            let _ = reply.send(Err("the session is gone".into()));
                        }
                    }
                }
            },
            Ctl::PermMode { mode, reply } => match outcome {
                Ok(_) => {
                    self.meta.permission_mode = mode;
                    self.save_meta();
                    self.emit("perm_mode", json!({"mode": mode.name()}));
                    let _ = reply.send(Ok(()));
                }
                Err(why) => {
                    let _ = reply.send(Err(why));
                }
            },
            Ctl::StopTask { reply } => {
                let _ = reply.send(outcome.map(|_| ()));
            }
        }
    }

    /// The process is gone (ended, replaced or exited): what still waits for its answer is settled without it. A model or a mode that was
    /// asked for is kept (the next process starts with it); a job stop cannot be done.
    fn ctl_drop(&mut self) {
        for (_, waiting) in std::mem::take(&mut self.ctl) {
            match waiting {
                Ctl::SetModel { model, reply } => {
                    self.meta.model = model.clone();
                    self.save_meta();
                    self.emit("model", json!({"model": model}));
                    let _ = reply.send(Ok(()));
                }
                Ctl::PermMode { mode, reply } => {
                    self.meta.permission_mode = mode;
                    self.save_meta();
                    self.emit("perm_mode", json!({"mode": mode.name()}));
                    let _ = reply.send(Ok(()));
                }
                Ctl::StopTask { reply } => {
                    let _ = reply.send(Err("the process ended".into()));
                }
            }
        }
    }

    /// A normal chat does not start its `claude` in a project whose own configuration can run code, redirect the program or widen what
    /// it may do until the person has decided (provider chats never read it). The findings go out as a transient event the window draws.
    fn project_gate(&mut self) -> Result<(), String> {
        if matches!(self.meta.project_cfg, Some(ProjectChoice::Without)) {
            return Ok(());
        }
        let scan = crate::project_check::scan(&self.meta.cwd);
        if !scan.needs_decision() {
            return Ok(());
        }
        // A link is not followed, so what it leads to may change with the hash staying the same: it is never trusted, only left out.
        if !scan.has_link() && matches!(&self.meta.project_cfg, Some(ProjectChoice::Trusted { hash, strong }) if *hash == scan.hash && strong.as_ref().is_none_or(|s| *s == scan.strong)) {
            return Ok(());
        }
        // Trust the person chose to remember for this project: it skips the question only for the files exactly as they were shown (the hashes are
        // those of the files as they are now, not what the stored entry says), and the chat then holds it like a decision of its own.
        if self.remembered_trust(&scan) {
            self.meta.project_cfg = Some(ProjectChoice::Trusted { hash: scan.hash, strong: Some(scan.strong) });
            self.save_meta();
            self.emit("project_cfg", json!({"choice": "trusted", "from": "project"}));
            return Ok(());
        }
        self.emit_project_check(&scan);
        Err(format!("this project's Claude Code settings need a decision before claude starts (decide in the desktop app): {}", scan.summary()))
    }

    fn emit_project_check(&mut self, scan: &crate::project_check::Scan) {
        // What the window is now showing: a decision is only taken for exactly this (see `project_decide`).
        self.shown_strong = Some(scan.strong.clone());
        let findings: Vec<Value> = scan.findings.iter().map(|f| json!({"severity": f.severity.name(), "file": f.file, "key": f.key, "text": f.text})).collect();
        self.emit_transient("project_check", json!({"hash": scan.hash, "findings": findings}));
    }

    /// The key this chat's project is filed under in the trust store (its account and its project folder), None for a chat that cannot use it
    /// (a provider or container chat reads nothing of the project, and a folder that is not there has no key).
    fn trust_key(&self) -> Option<String> {
        // A chat in a worktree reads the worktree's files, not the project folder's, and the form that would show and forget the trust cannot tell
        // which subfolder it was made from: it keeps asking per chat.
        if self.meta.provider.is_some() || self.meta.container || self.meta.worktree.is_some() {
            return None;
        }
        let project = crate::project_defaults::key_of(&self.meta.project_dir())?;
        Some(crate::project_trust::key(self.meta.account.as_ref().map(|a| a.id.as_str()), &project))
    }

    fn remembered_trust(&self, scan: &crate::project_check::Scan) -> bool {
        match (&self.trust, self.trust_key()) {
            (Some(store), Some(key)) => store.trusts(&key, scan),
            _ => false,
        }
    }

    fn project_decide(&mut self, hash: &str, without: bool, remember: bool) -> Result<(), String> {
        if self.meta.provider.is_some() {
            return Err("a provider chat does not read the project's settings".into());
        }
        if without {
            // A running process has read the project's settings already: leaving them out is for a process that has not started.
            if self.process.is_some() {
                return Err("this chat's claude is already running with the project's settings; start a new chat to leave them out".into());
            }
            self.meta.project_cfg = Some(ProjectChoice::Without);
            self.save_meta();
            self.emit("project_cfg", json!({"choice": "without"}));
            return Ok(());
        }
        let scan = crate::project_check::scan(&self.meta.cwd);
        if scan.has_link() {
            self.emit_project_check(&scan);
            return Err("a settings file or folder of this project is a link, which is not followed and cannot be trusted; start without the project's settings, or replace the link by the file".into());
        }
        // The hash the window sends is the short one; the strong one must be that of what the window last showed too, so what is trusted (and
        // remembered) is what the person looked at even if the short hash is made to agree.
        if scan.needs_decision() && (scan.hash != hash || self.shown_strong.as_deref() != Some(scan.strong.as_str())) {
            self.emit_project_check(&scan);
            return Err("the settings changed again; look at them again".into());
        }
        // "Remember for this project": recorded from the scan just made (the hashes of what is on disk now, which is what was shown), and only
        // for what `remember_allowed` lets be remembered; asking for it when it may not be is refused rather than quietly ignored.
        if remember {
            let (Some(store), Some(key)) = (&self.trust, self.trust_key()) else { return Err("trust cannot be remembered for this chat".into()) };
            if !store.record(&key, &scan) {
                return Err("this configuration cannot be remembered for the project; trust it for this chat only".into());
            }
        }
        self.meta.project_cfg = Some(ProjectChoice::Trusted { hash: scan.hash, strong: Some(scan.strong) });
        self.save_meta();
        self.emit("project_cfg", json!({"choice": "trusted", "remembered": remember}));
        Ok(())
    }

    fn set_archive_flag(&mut self, on: bool) {
        if self.meta.archived != on {
            self.meta.archived = on;
            self.save_meta();
            self.emit("archived", json!({"archived": on}));
        }
    }

    fn set_permission_mode(&mut self, mode: crate::claude::perm::PermMode, reply: oneshot::Sender<Result<(), String>>) {
        if self.state == State::Running || !self.pending.is_empty() {
            let _ = reply.send(Err("the permission mode can be changed between turns, not while one runs or a question waits".into()));
            return;
        }
        if self.meta.worktree.as_ref().is_some_and(|w| w.removed) || self.realm.as_ref().is_some_and(|r| r.is_retired()) {
            let _ = reply.send(Err("this chat can only be read".into()));
            return;
        }
        if self.meta.permission_mode == mode {
            let _ = reply.send(Ok(()));
            return;
        }
        // Everything-without-asking is only open to a chat that was made with it (the program may refuse it otherwise).
        if mode == crate::claude::perm::PermMode::Bypass && self.meta.permission_mode != mode {
            let _ = reply.send(Err("bypassing every question can only be chosen when a chat is made".into()));
            return;
        }
        let name = mode.name();
        match self.control(|id| protocol::set_permission_mode(id, name), Ctl::PermMode { mode, reply }) {
            Ok(()) => {}
            // No process yet: the next one starts with it.
            Err(Ctl::PermMode { mode, reply }) => {
                self.meta.permission_mode = mode;
                self.save_meta();
                self.emit("perm_mode", json!({"mode": mode.name()}));
                let _ = reply.send(Ok(()));
            }
            Err(_) => {}
        }
    }

    /// See `SessionHandle::set_model`. The second value is true when a process was ended.
    fn set_model(&mut self, model: Option<String>) -> (Result<(), String>, bool) {
        if let Some(why) = self.model_refusal() {
            return (Err(why.into()), false);
        }
        if self.meta.model == model {
            return (Ok(()), false);
        }
        self.ctl_drop();
        let had = self.process.take();
        let restarted = had.is_some();
        if had.is_some() {
            self.forget_background();
        }
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

    fn set_provider(&mut self, provider: Option<String>) -> (Result<(), String>, bool) {
        if self.state == State::Running || !self.pending.is_empty() {
            return (Err("the gateway can be changed between turns, not while one runs or a question waits".into()), false);
        }
        if self.retiring || self.gone {
            return (Err("this chat is being removed".into()), false);
        }
        if self.meta.container {
            return (Err("a chat in a container keeps its provider".into()), false);
        }
        // A provider chat that does not use an account's login stays with its provider: its isolation is the point of it.
        if self.meta.provider.as_ref().is_some_and(|p| !p.login) {
            return (Err("a provider chat stays with its provider".into()), false);
        }
        if self.realm.as_ref().is_none_or(|r| r.is_retired()) {
            return (Err("this chat's account is not available; make a new chat".into()), false);
        }
        if self.meta.worktree.as_ref().is_some_and(|w| w.removed) {
            return (Err("this chat's worktree was removed; the chat can only be read".into()), false);
        }
        let new_ref = match provider.as_deref().filter(|id| !id.is_empty()) {
            None => None,
            Some(id) => {
                let Some(store) = &self.providers else { return (Err("providers are not available".into()), false) };
                let Some(found) = store.get(id) else { return (Err("no such provider".into()), false) };
                if found.auth != crate::providers::Auth::AccountLogin {
                    return (Err("only a provider that uses an account's login can be switched to in a chat that has begun".into()), false);
                }
                Some(found.reference())
            }
        };
        if self.meta.provider.as_ref().map(|p| &p.id) == new_ref.as_ref().map(|p| &p.id) {
            return (Ok(()), false);
        }
        self.ctl_drop();
        let had = self.process.take();
        let restarted = had.is_some();
        if had.is_some() {
            self.forget_background();
        }
        if let Some(process) = had {
            process.stop();
            if let Some(gate) = &self.mcp {
                gate.revoke(&self.meta.sid);
            }
            let _ = std::fs::remove_file(self.recall_path());
            let _ = std::fs::remove_dir_all(self.skills_path());
        }
        // The key and token of the old provider are no longer scrubbed for: the next start sets those of the new one.
        self.secrets = Vec::new();
        self.meta.provider = new_ref.clone();
        self.save_meta();
        self.emit("provider", json!({"provider": new_ref.as_ref().map(|p| json!({"id": p.id, "name": p.name, "host": p.host}))}));
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
        self.bg_live = 0;
        self.ctl_drop();
        // A program told to resume a conversation it does not have ends before its init: the conversation is forgotten (also on disk), or
        // every later message would resume it and fail the same way for good. The person is told; the next message starts a new one.
        if let Some(lost) = self.spawned_resume.take().filter(|id| !self.init_seen && self.meta.claude_session_id.as_deref() == Some(id.as_str()) && crate::claude::process::resume_lost(&self.last_stderr)) {
            self.meta.claude_session_id = None;
            self.save_meta();
            self.emit("error", json!({"message": "Claude Code has no record of this conversation, so it could not be resumed. Your next message starts a new conversation without the earlier context.", "resume_lost": lost}));
        }
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

    fn in_worktree(cwd: &str, repo: &str) -> Meta {
        let mut meta = new_meta(PathBuf::from(cwd), None, None, false, None);
        meta.worktree = Some(crate::worktree::WorktreeRef { repo: PathBuf::from(repo), branch: "tenrec/x".into(), base_branch: "main".into(), base_commit: "c".into(), removed: false, root: None, config_hash: String::new() });
        meta
    }

    #[test]
    fn a_trust_stored_before_the_strong_hash_existed_still_reads() {
        let old: ProjectChoice = serde_json::from_str(r#"{"Trusted":{"hash":"abc"}}"#).unwrap();
        assert_eq!(old, ProjectChoice::Trusted { hash: "abc".into(), strong: None });
        let new = ProjectChoice::Trusted { hash: "abc".into(), strong: Some("def".into()) };
        assert_eq!(serde_json::from_str::<ProjectChoice>(&serde_json::to_string(&new).unwrap()).unwrap(), new);
        assert!(!serde_json::to_string(&old).unwrap().contains("strong"));
    }

    #[test]
    fn an_old_meta_without_archived_reads_false_and_false_is_not_written() {
        let old = r#"{"sid":"a1","cwd":"/tmp","model":null,"claude_session_id":null,"title":"t","created":1}"#;
        let meta: Meta = serde_json::from_str(old).unwrap();
        assert!(!meta.archived);
        assert!(!serde_json::to_string(&meta).unwrap().contains("archived"));
        let mut meta = meta;
        meta.archived = true;
        assert!(serde_json::to_string(&meta).unwrap().contains("\"archived\":true"));
    }

    #[test]
    fn the_project_of_a_worktree_chat_is_the_folder_it_was_made_from() {
        assert_eq!(in_worktree("/w/k/sid", "/p").project_key(), "/p");
        assert_eq!(in_worktree("/w/k/sid", "/p").project_dir(), PathBuf::from("/p"));
        let plain = new_meta(PathBuf::from("/p"), None, None, false, None);
        assert_eq!(plain.project_key(), "/p", "a chat without a worktree is the project of its own folder");
        assert_eq!(plain.project_key(), in_worktree("/w/k/sid", "/p").project_key(), "so both read and write the same notes");
    }

    #[test]
    fn on_windows_a_plain_chat_and_a_worktree_chat_of_one_project_have_one_key() {
        // `canonicalize` leaves `\\?\` on a plain chat's folder; the worktree's repo went through `platform::canon`, which does not.
        let plain = new_meta(PathBuf::from(r"\\?\C:\proj"), None, None, false, None);
        assert_eq!(plain.project_key(), in_worktree(r"C:\w\sid", r"C:\proj").project_key());
        assert_eq!(plain.cwd, PathBuf::from(r"\\?\C:\proj"), "the folder the chat is stored with is not touched");
    }

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
