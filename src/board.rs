//! The board: one line for every chat, made from what the host already knows (the chat's state, its log, and the worktree's facts), so a
//! person running several chats can see which one waits for them, which one is done and can be merged, and which one has lost its way.
//! It reads and never decides: nothing here approves, merges or removes (see `docs/design/board.md`).
//!
//! The parts that decide anything are plain functions of their inputs (`Digest::feed`, `merge_state`, `signals`, `row`, `due`): no clock,
//! no I/O. The clock is an argument, so every rule is tested by giving it the time.
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;

use crate::session::{Envelope, Info, State};
use crate::watch::{Alert, Watch};
use crate::worktree::Facts;

/// A chat that has been working this long since it last started to work is said to take long.
pub const LONG_MS: u64 = 10 * 60 * 1000;
/// The context window is said to be nearly full from this percentage of it.
pub const CTX_WARN: u8 = 85;
/// Different tool names kept for the turn's most used tool.
const TOOL_NAMES: usize = 64;

// -------------------------------------------------------------------------------------------------------------- what a chat did

/// What a chat's log adds up to, folded event by event. Created the first time the board asks (so a closed board costs nothing) and kept
/// current by the chat after that; folding the whole log gives the same as folding it event by event.
#[derive(Debug, Default)]
pub struct Digest {
    last_ts: u64,
    run_ts: u64,
    turns: u32,
    turn_tools: BTreeMap<String, u32>,
    last_tool: String,
    loops: u32,
    scope: u32,
    guarded: u32,
    ctx_used: Option<u64>,
    ctx_window: Option<u64>,
    last_ok: Option<bool>,
    watch: Watch,
}

/// The numbers of a `Digest`, small enough to hand out (the `Watch` inside keeps growing sets).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Counters {
    /// The newest event of the chat (ms since the epoch; 0 when no event has a time).
    pub last_ts: u64,
    /// When the chat last began to work (a `running` state), 0 if never.
    pub run_ts: u64,
    pub turns: u32,
    pub tools: u32,
    /// The tool the chat used most in its latest turn, and how often.
    pub top_tool: Option<(String, u32)>,
    pub last_tool: String,
    pub edited: u32,
    pub loops: u32,
    pub scope: u32,
    pub guarded: u32,
    /// What the last call of the chat took of the model's window, and the window.
    pub ctx: Option<(u64, u64)>,
    /// Whether the last turn went well (None while it goes on, or when none has ended).
    pub last_ok: Option<bool>,
}

impl Digest {
    pub fn from_log(log: &[Envelope]) -> Self {
        let mut digest = Self::default();
        for event in log {
            digest.feed(event);
        }
        digest
    }

    /// One event of the chat, in order. What was taken over from another chat only says that the chat was alive; its calls and turns are
    /// not this chat's own.
    pub fn feed(&mut self, e: &Envelope) {
        self.last_ts = self.last_ts.max(e.ts);
        if e.data["imported"].as_bool() == Some(true) {
            return;
        }
        match e.kind.as_str() {
            "user" => {
                self.turns += 1;
                self.turn_tools.clear();
                self.last_ok = None;
            }
            // The work began now: a prompt starts it, and so does a background job that finished and woke the chat (no prompt then).
            "state" if e.data["state"].as_str() == Some("running") => self.run_ts = e.ts,
            "claude" => match e.data["type"].as_str().unwrap_or_default() {
                "assistant" => self.assistant(&e.data),
                "user" => self.results(&e.data),
                "result" => self.result(&e.data),
                _ => {}
            },
            _ => {}
        }
    }

    fn assistant(&mut self, d: &Value) {
        let sub_agent = d["parent_tool_use_id"].as_str().is_some_and(|p| !p.is_empty());
        if !sub_agent {
            // The same sum the live view takes: the last call's prompt plus its answer.
            if let Some(usage) = d.pointer("/message/usage") {
                let n: u64 = ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens", "output_tokens"].iter().map(|k| usage[*k].as_u64().unwrap_or(0)).sum();
                if n > 0 {
                    self.ctx_used = Some(n);
                }
            }
        }
        for block in d.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
            if block["type"] != "tool_use" {
                continue;
            }
            let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) else { continue };
            let input = block.get("input").cloned().unwrap_or(Value::Null);
            let before = self.watch.calls();
            let alerts = self.watch.call(id, name, &input, sub_agent);
            // A message is written more than once while it streams: a call counts the first time its id is seen.
            if self.watch.calls() > before {
                self.last_tool = name.chars().take(80).collect();
                if self.turn_tools.contains_key(name) || self.turn_tools.len() < TOOL_NAMES {
                    *self.turn_tools.entry(name.chars().take(80).collect()).or_default() += 1;
                }
            }
            for alert in alerts {
                match alert {
                    Alert::Loop { .. } => self.loops += 1,
                    Alert::Scope { files } => self.scope = u32::try_from(files).unwrap_or(u32::MAX),
                }
            }
        }
    }

    /// The results of tools: the ones a guard held are counted (by the same judgement the live view makes), and their text is dropped.
    fn results(&mut self, d: &Value) {
        for block in d.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
            if block["type"] == "tool_result" && block["is_error"].as_bool() == Some(true) && crate::live::derive::blocked(&crate::live::derive::result_text(&block["content"])) {
                self.guarded += 1;
            }
        }
    }

    fn result(&mut self, d: &Value) {
        // A command the plug-in answered itself called no model: no turn ended.
        if d["num_turns"].as_u64() == Some(0) {
            return;
        }
        self.last_ok = Some(d["subtype"] == "success" && d["is_error"].as_bool() != Some(true));
        self.ctx_window = d["modelUsage"].as_object().and_then(|m| m.values().filter_map(|u| u["contextWindow"].as_u64()).max()).or(self.ctx_window);
    }

    pub fn counters(&self) -> Counters {
        let top_tool = self.turn_tools.iter().max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0))).map(|(name, n)| (name.clone(), *n));
        let small = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        Counters {
            last_ts: self.last_ts,
            run_ts: self.run_ts,
            turns: self.turns,
            tools: small(self.watch.calls()),
            top_tool,
            last_tool: self.last_tool.clone(),
            edited: small(self.watch.edited()),
            loops: self.loops,
            scope: self.scope,
            guarded: self.guarded,
            ctx: self.ctx_used.zip(self.ctx_window),
            last_ok: self.last_ok,
        }
    }
}

/// What a chat tells the board: its `Info`, what its log adds up to, and what it has cost (the chat's own figure, with its rules about old
/// logs and taken-over events, not a sum made again here).
#[derive(Debug, Clone)]
pub struct BoardSnap {
    pub info: Info,
    pub counters: Counters,
    pub cost: f64,
}

// ------------------------------------------------------------------------------------------------------------ the worktree's side

/// Where a chat's worktree stands for a merge: one thing, the most important there is to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Merge {
    /// Not looked at yet.
    Unknown,
    /// Removed, or gone from the disk.
    Gone,
    Conflict { n: usize },
    Uncommitted { n: usize },
    /// The branch the chat came from is not checked out in the project, or the project has changes or a merge going on.
    Blocked,
    /// Nothing to merge.
    Nothing,
    /// Can be merged, though the project has moved on by `n` commits.
    Behind { n: usize },
    Ready,
    /// Not looked at: the project's git settings changed since the chat was made (the person decides in the chat).
    UnsafeConfig,
    /// Not looked at: the worktree is not one this app made.
    UnsafeOwner,
    /// Looked at and git failed or took too long.
    Failed,
}

/// The one thing to say about a worktree's facts, most important first.
pub fn merge_state(f: &Facts) -> Merge {
    if f.missing {
        return Merge::Gone;
    }
    let has = |name: &str| f.blockers.contains(&name);
    if has("conflict") {
        return Merge::Conflict { n: f.conflicts.len() + f.conflicts_more };
    }
    if has("uncommitted") {
        return Merge::Uncommitted { n: f.uncommitted };
    }
    if has("wrong_branch") || has("main_dirty") || has("mid_merge") {
        return Merge::Blocked;
    }
    if has("nothing") {
        return Merge::Nothing;
    }
    if f.behind > 0 {
        return Merge::Behind { n: f.behind };
    }
    Merge::Ready
}

/// What the board shows of a chat's worktree.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WtRow {
    pub merge: Merge,
    pub ahead: usize,
    pub behind: usize,
    pub files: usize,
    pub insertions: usize,
    pub deletions: usize,
    pub uncommitted: usize,
    pub conflicts: usize,
    /// How long ago it was looked at (ms), and whether a new look is under way.
    pub age_ms: u64,
    pub refreshing: bool,
}

impl WtRow {
    pub fn of(merge: Merge) -> Self {
        Self { merge, ahead: 0, behind: 0, files: 0, insertions: 0, deletions: 0, uncommitted: 0, conflicts: 0, age_ms: 0, refreshing: false }
    }

    pub fn from_facts(f: &Facts) -> Self {
        Self { merge: merge_state(f), ahead: f.ahead, behind: f.behind, files: f.files, insertions: f.insertions, deletions: f.deletions, uncommitted: f.uncommitted, conflicts: f.conflicts.len() + f.conflicts_more, age_ms: 0, refreshing: false }
    }
}

// ------------------------------------------------------------------------------------------------------------------- signals

/// Something about a chat worth a look. `kind`: `ask` (n questions wait), `loop`, `scope` (n files edited), `guard` (n calls held),
/// `long` (working for n minutes), `failed` (the last turn failed), `ctx` (n percent of the window).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Signal {
    pub kind: &'static str,
    pub n: u32,
}

pub fn ctx_percent(ctx: Option<(u64, u64)>) -> Option<u8> {
    ctx.filter(|(_, window)| *window > 0).map(|(used, window)| u8::try_from((used.saturating_mul(100) / window).min(100)).unwrap_or(100))
}

/// The signals of a chat at `now`, the strongest first.
pub fn signals(c: &Counters, state: &State, pending: usize, now: u64) -> Vec<Signal> {
    let mut out = Vec::new();
    if pending > 0 {
        out.push(Signal { kind: "ask", n: u32::try_from(pending).unwrap_or(u32::MAX) });
    }
    if c.loops > 0 {
        out.push(Signal { kind: "loop", n: c.loops });
    }
    if c.scope > 0 {
        out.push(Signal { kind: "scope", n: c.scope });
    }
    if c.guarded > 0 {
        out.push(Signal { kind: "guard", n: c.guarded });
    }
    // A clock set back, or a chat that never recorded when it began to work, says nothing.
    if matches!(state, State::Running) && c.run_ts > 0 && now >= c.run_ts && now - c.run_ts >= LONG_MS {
        out.push(Signal { kind: "long", n: u32::try_from((now - c.run_ts) / 60_000).unwrap_or(u32::MAX) });
    }
    if matches!(state, State::Idle) && c.last_ok == Some(false) {
        out.push(Signal { kind: "failed", n: 1 });
    }
    if let Some(pct) = ctx_percent(c.ctx).filter(|p| *p >= CTX_WARN) {
        out.push(Signal { kind: "ctx", n: u32::from(pct) });
    }
    out
}

// ----------------------------------------------------------------------------------------------------------------------- rows

/// One line of the board.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub sid: String,
    pub title: String,
    /// The project's folder name.
    pub project: String,
    pub account: Option<String>,
    pub account_name: Option<String>,
    pub created: u64,
    pub worktree: bool,
    pub provider: bool,
    pub container: bool,
    /// `running`, `idle` or `exited`.
    pub state: &'static str,
    pub pending: usize,
    pub archived: bool,
    pub counters: Counters,
    pub cost: f64,
    pub ctx_pct: Option<u8>,
    pub wt: Option<WtRow>,
    pub signals: Vec<Signal>,
}

/// What is known of a chat's worktree: nothing yet, or the last look.
pub type WtView = Option<WtRow>;

/// The row of a chat. `wt` is only looked at for a chat with a worktree.
pub fn row(snap: &BoardSnap, wt: WtView, now: u64) -> Row {
    let meta = &snap.info.meta;
    let worktree = meta.worktree.as_ref();
    let wt = worktree.map(|w| if w.removed { WtRow::of(Merge::Gone) } else { wt.unwrap_or_else(|| WtRow::of(Merge::Unknown)) });
    let project = meta.project_dir().file_name().map(|n| n.to_string_lossy().chars().take(60).collect::<String>()).unwrap_or_default();
    Row {
        sid: meta.sid.clone(),
        title: crate::live::mask::mask_text(&meta.title, 80),
        project,
        account: meta.account.as_ref().map(|a| a.id.clone()),
        account_name: meta.account.as_ref().map(|a| a.name.chars().take(40).collect()),
        created: meta.created,
        worktree: worktree.is_some(),
        provider: meta.provider.is_some(),
        container: meta.container,
        state: match snap.info.state {
            State::Running => "running",
            State::Idle => "idle",
            State::Exited { .. } => "exited",
        },
        pending: snap.info.pending,
        archived: meta.archived,
        counters: snap.counters.clone(),
        cost: snap.cost,
        ctx_pct: ctx_percent(snap.counters.ctx),
        wt,
        signals: signals(&snap.counters, &snap.info.state, snap.info.pending, now),
    }
}

// ------------------------------------------------------------------------------------------------------- looking at worktrees

/// Looks at one worktree at a time are expensive (a dozen git runs after the checks); these are how often, and how many at once.
const RUNNING_EVERY: u64 = 30_000;
const AFTER_EVENT: u64 = 2_000;
const IDLE_TTL: u64 = 180_000;
const OLD_AFTER: u64 = 24 * 60 * 60 * 1000;
const OLD_TTL: u64 = 30 * 60 * 1000;
const UNSAFE_TTL: u64 = 60_000;
const FAIL_BASE: u64 = 60_000;
const FAIL_MAX: u64 = 10 * 60_000;
const FORCE_GAP: u64 = 5_000;
/// A refresh may take this long once it has its turn.
pub const REFRESH_LIMIT: std::time::Duration = std::time::Duration::from_secs(20);
/// At most this many chats are looked at at once.
pub const CONCURRENT: usize = 3;
/// A look that waited its turn is dropped when nobody has asked for the board for this long.
pub const ABANDONED_MS: u64 = 10_000;
/// A chat's answer to the board is waited for this long; later, its last one is shown.
pub const SNAP_WAIT: std::time::Duration = std::time::Duration::from_millis(300);

/// What changes the answer about a chat's worktree besides time: its settings hash (changed when the person accepts the new settings in
/// the chat), whether it was removed, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub config_hash: String,
    pub removed: bool,
    pub cwd: PathBuf,
}

/// How the last look ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Last {
    Facts,
    Unsafe,
    Failed,
}

#[derive(Debug, Clone)]
struct Entry {
    wt: WtRow,
    last: Last,
    at: u64,
    seen_last_ts: u64,
    seen_running: bool,
    key: Key,
    fails: u32,
    in_flight: bool,
    /// Not before this time (a look that found the project busy tries again soon, not at once).
    skip_until: u64,
}

/// What `due` needs to know about the chat now.
#[derive(Debug, Clone)]
pub struct Now<'a> {
    pub at: u64,
    pub running: bool,
    pub last_ts: u64,
    pub key: &'a Key,
}

/// Whether the worktree of a chat is to be looked at again.
fn due(entry: Option<&Entry>, n: &Now) -> bool {
    let Some(e) = entry else { return true };
    if e.in_flight || n.at < e.skip_until {
        return false;
    }
    if e.key != *n.key {
        return true;
    }
    let age = n.at.saturating_sub(e.at);
    let moved = n.last_ts > e.seen_last_ts;
    match e.last {
        Last::Failed => return age >= FAIL_BASE.saturating_mul(1 << e.fails.saturating_sub(1).min(10)).min(FAIL_MAX),
        Last::Unsafe => return age >= UNSAFE_TTL,
        Last::Facts => {}
    }
    if e.seen_running && !n.running {
        return true;
    }
    if n.running {
        return moved && age >= RUNNING_EVERY;
    }
    if moved {
        return age >= AFTER_EVENT;
    }
    let old = n.last_ts > 0 && n.at.saturating_sub(n.last_ts) >= OLD_AFTER;
    age >= if old { OLD_TTL } else { IDLE_TTL }
}

/// How a look ended.
pub enum Outcome {
    Done(WtRow, Last),
    /// The project was busy (a merge or a removal holds it): nothing was looked at.
    Busy,
}

pub struct FactsCache {
    entries: Mutex<HashMap<String, Entry>>,
    /// The last answer of each chat: shown when a chat is too slow to answer this time.
    snaps: Mutex<HashMap<String, BoardSnap>>,
    /// One line of looks for each project: chats of one project are looked at one after another (waiting here, not quarrelling over the
    /// project's lock), and a look that has its turn takes that lock only if no merge or removal holds it.
    gates: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    /// Left to the caller: it owns the semaphore of the looks.
    pub permits: tokio::sync::Semaphore,
    asked: AtomicU64,
}

impl Default for FactsCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Held while a look is under way; whatever ends it (finished, cancelled, panicked) marks it as not under way.
pub struct Flight {
    cache: Arc<FactsCache>,
    sid: String,
}

impl Drop for Flight {
    fn drop(&mut self) {
        if let Some(e) = self.cache.entries.lock().unwrap_or_else(|p| p.into_inner()).get_mut(&self.sid) {
            e.in_flight = false;
        }
    }
}

impl FactsCache {
    pub fn new() -> Self {
        Self { entries: Mutex::new(HashMap::new()), snaps: Mutex::new(HashMap::new()), gates: Mutex::new(HashMap::new()), permits: tokio::sync::Semaphore::new(CONCURRENT), asked: AtomicU64::new(0) }
    }

    /// The line of looks of a project.
    pub fn gate(&self, repo: &std::path::Path) -> Arc<tokio::sync::Mutex<()>> {
        self.gates.lock().unwrap_or_else(|p| p.into_inner()).entry(repo.to_path_buf()).or_default().clone()
    }

    /// Notes that the board was asked for at `now`.
    pub fn asked(&self, now: u64) {
        self.asked.store(now, Ordering::Relaxed);
    }

    /// True when nobody has asked for the board for a while (the page was closed).
    pub fn abandoned(&self, now: u64) -> bool {
        now.saturating_sub(self.asked.load(Ordering::Relaxed)) >= ABANDONED_MS
    }

    /// What is known of a chat's worktree, as the row shows it.
    pub fn view(&self, sid: &str, now: u64) -> WtView {
        let entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.get(sid).map(|e| WtRow { age_ms: now.saturating_sub(e.at), refreshing: e.in_flight, ..e.wt.clone() })
    }

    /// Whether a look is to be started for the chat; when it is, it is marked as under way and the guard that ends it is returned.
    pub fn begin(self: &Arc<Self>, sid: &str, n: &Now, force: bool) -> Option<Flight> {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let entry = entries.get_mut(sid);
        // "Look again" makes everything old, but not oftener than every few seconds for one chat.
        let wanted = match &entry {
            Some(e) if force && !e.in_flight => n.at.saturating_sub(e.at) >= FORCE_GAP && n.at >= e.skip_until,
            other => due(other.as_deref(), n),
        };
        if !wanted {
            return None;
        }
        match entry {
            Some(e) => e.in_flight = true,
            None => {
                let mut wt = WtRow::of(Merge::Unknown);
                wt.refreshing = true;
                entries.insert(sid.to_owned(), Entry { wt, last: Last::Failed, at: 0, seen_last_ts: 0, seen_running: false, key: n.key.clone(), fails: 0, in_flight: true, skip_until: 0 });
            }
        }
        Some(Flight { cache: self.clone(), sid: sid.to_owned() })
    }

    /// The end of a look. `n` is the chat as it was when the look began; a look of a chat that has changed meanwhile (`key`) is dropped.
    pub fn finish(&self, sid: &str, n: &Now, outcome: Outcome) {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let Some(e) = entries.get_mut(sid) else { return };
        match outcome {
            Outcome::Busy => e.skip_until = n.at + FORCE_GAP,
            Outcome::Done(wt, last) => {
                e.fails = if last == Last::Failed { e.fails.saturating_add(1) } else { 0 };
                e.wt = wt;
                e.last = last;
                e.at = n.at;
                e.seen_last_ts = n.last_ts;
                e.seen_running = n.running;
                e.key = n.key.clone();
                e.skip_until = 0;
            }
        }
    }

    /// Keeps a chat's answer, and gives back the one kept (for a chat that did not answer in time).
    pub fn remember(&self, snap: &BoardSnap) {
        self.snaps.lock().unwrap_or_else(|p| p.into_inner()).insert(snap.info.meta.sid.clone(), snap.clone());
    }

    pub fn recall(&self, sid: &str) -> Option<BoardSnap> {
        self.snaps.lock().unwrap_or_else(|p| p.into_inner()).get(sid).cloned()
    }

    /// Forgets the chats that are not in `sids` any more.
    pub fn keep_only(&self, sids: &std::collections::HashSet<String>) {
        self.entries.lock().unwrap_or_else(|p| p.into_inner()).retain(|sid, _| sids.contains(sid));
        self.snaps.lock().unwrap_or_else(|p| p.into_inner()).retain(|sid, _| sids.contains(sid));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

// ------------------------------------------------------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn env(seq: u64, ts: u64, kind: &str, data: Value) -> Envelope {
        Envelope { v: 1, sid: "s".into(), seq, ts, kind: kind.into(), data }
    }

    fn assistant(blocks: Value, parent: Option<&str>, used: u64) -> Value {
        json!({"type": "assistant", "parent_tool_use_id": parent, "message": {"content": blocks, "usage": {"input_tokens": used, "output_tokens": 5}}})
    }

    fn call(id: &str, name: &str, input: Value) -> Value {
        json!([{"type": "tool_use", "id": id, "name": name, "input": input}])
    }

    fn result_ok(window: u64) -> Value {
        json!({"type": "result", "subtype": "success", "num_turns": 1, "modelUsage": {"m": {"contextWindow": window}}})
    }

    fn tool_error(id: &str, text: &str) -> Value {
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text, "is_error": true}]}})
    }

    fn story() -> Vec<Envelope> {
        vec![
            env(1, 1000, "state", json!({"state": "running"})),
            env(2, 1000, "user", json!({"text": "go"})),
            env(3, 1100, "claude", assistant(call("a", "Edit", json!({"file_path": "x.rs"})), None, 100)),
            // The same message written again while it streams: counted once.
            env(4, 1110, "claude", assistant(call("a", "Edit", json!({"file_path": "x.rs"})), None, 100)),
            env(5, 1200, "claude", assistant(call("b", "Edit", json!({"file_path": "y.rs"})), None, 120)),
            env(6, 1300, "claude", assistant(call("c", "Bash", json!({"command": "ls"})), None, 150)),
            env(7, 1400, "claude", tool_error("c", "SmartWorkbench guard: held")),
            env(8, 1500, "claude", result_ok(1000)),
            env(9, 1500, "state", json!({"state": "idle"})),
        ]
    }

    #[test]
    fn a_log_folded_at_once_is_the_log_folded_event_by_event() {
        let log = story();
        let whole = Digest::from_log(&log).counters();
        let mut each = Digest::default();
        for e in &log {
            each.feed(e);
        }
        assert_eq!(whole, each.counters());
        // A digest begun in the middle (the chat wrote events while it was restoring) still counts every event once.
        let mut late = Digest::from_log(&log[..4]);
        for e in &log[4..] {
            late.feed(e);
        }
        assert_eq!(whole, late.counters());
    }

    #[test]
    fn the_story_adds_up() {
        let c = Digest::from_log(&story()).counters();
        assert_eq!((c.turns, c.tools, c.edited, c.guarded), (1, 3, 2, 1), "a call repeated while it streams counts once");
        assert_eq!(c.top_tool, Some(("Edit".to_owned(), 2)));
        assert_eq!((c.last_tool.as_str(), c.last_ts, c.run_ts), ("Bash", 1500, 1000));
        assert_eq!((c.ctx, c.last_ok), (Some((155, 1000)), Some(true)));
    }

    #[test]
    fn a_new_prompt_starts_the_turns_tool_count_again_and_forgets_the_last_outcome() {
        let mut d = Digest::from_log(&story());
        d.feed(&env(10, 2000, "user", json!({"text": "more"})));
        let c = d.counters();
        assert_eq!((c.turns, c.top_tool, c.last_ok), (2, None, None));
        assert_eq!(c.tools, 3, "the chat's total goes on");
    }

    #[test]
    fn what_was_taken_over_from_another_chat_only_says_the_chat_was_alive() {
        let mut d = Digest::default();
        d.feed(&env(1, 500, "user", json!({"text": "old", "imported": true})));
        d.feed(&env(2, 600, "claude", json!({"type": "assistant", "imported": true, "message": {"content": call("z", "Bash", json!({"command": "ls"}))}})));
        d.feed(&env(3, 700, "claude", json!({"type": "result", "subtype": "error", "imported": true, "num_turns": 3})));
        let c = d.counters();
        assert_eq!((c.turns, c.tools, c.last_ok, c.last_ts), (0, 0, None, 700));
    }

    #[test]
    fn work_that_a_background_job_wakes_begins_when_the_state_says_so() {
        let mut d = Digest::from_log(&story());
        // No prompt: the chat was woken at 9000.
        d.feed(&env(10, 9000, "state", json!({"state": "running"})));
        assert_eq!(d.counters().run_ts, 9000);
        let c = d.counters();
        let long = |now: u64| signals(&c, &State::Running, 0, now).iter().any(|s| s.kind == "long");
        assert!(!long(9000 + LONG_MS - 1), "it has only just been woken");
        assert!(long(9000 + LONG_MS));
    }

    #[test]
    fn the_guard_judgement_is_the_live_views() {
        let mut d = Digest::default();
        d.feed(&env(1, 1, "claude", tool_error("a", "SmartWorkbench team mode: not now")));
        d.feed(&env(2, 2, "claude", tool_error("b", "permission denied: no such file")));
        d.feed(&env(3, 3, "claude", json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "c", "content": "SmartWorkbench guard: ok text but not an error", "is_error": false}]}})));
        assert_eq!(d.counters().guarded, 1);
    }

    #[test]
    fn a_loop_and_a_wide_spread_are_the_watchs_and_a_command_that_called_no_model_ends_no_turn() {
        let mut d = Digest::default();
        for i in 0..5 {
            d.feed(&env(i, 10 + i, "claude", assistant(call(&format!("t{i}"), "Bash", json!({"command": "cargo test"})), None, 10)));
        }
        d.feed(&env(9, 99, "claude", json!({"type": "result", "subtype": "success", "num_turns": 0})));
        let c = d.counters();
        assert_eq!((c.loops, c.last_ok), (1, None));
        for i in 0..20 {
            d.feed(&env(20 + i, 100 + i, "claude", assistant(call(&format!("e{i}"), "Edit", json!({"file_path": format!("f{i}.rs")})), None, 10)));
        }
        assert_eq!(d.counters().scope, 20);
    }

    #[test]
    fn a_sub_agents_prompt_is_not_the_chats_context_and_events_without_a_time_do_not_move_the_clock_back() {
        let mut d = Digest::default();
        d.feed(&env(1, 50, "claude", assistant(json!([]), None, 400)));
        d.feed(&env(2, 0, "claude", assistant(json!([]), Some("agent1"), 900)));
        d.feed(&env(3, 0, "claude", result_ok(1000)));
        let c = d.counters();
        assert_eq!((c.ctx, c.last_ts), (Some((405, 1000)), 50));
    }

    fn facts() -> Facts {
        Facts { main_branch: Some("main".into()), base_branch: "main".into(), ahead: 2, behind: 0, files: 3, ..Facts::default() }
    }

    fn state_of(f: &mut Facts) -> Merge {
        f.blockers = crate::worktree::blockers(f);
        merge_state(f)
    }

    #[test]
    fn the_merge_state_is_the_most_important_thing_there_is_to_say() {
        let mut f = facts();
        assert_eq!(state_of(&mut f), Merge::Ready);
        f.behind = 4;
        assert_eq!(state_of(&mut f), Merge::Behind { n: 4 });
        f.main_dirty = true;
        assert_eq!(state_of(&mut f), Merge::Blocked, "a blocker is more than being behind");
        f.uncommitted = 1;
        assert_eq!(state_of(&mut f), Merge::Uncommitted { n: 1 });
        f.conflicts = vec!["a".into(), "b".into()];
        f.conflicts_more = 3;
        assert_eq!(state_of(&mut f), Merge::Conflict { n: 5 }, "a conflict is more than uncommitted work");
        let mut none = Facts { ahead: 0, ..facts() };
        assert_eq!(state_of(&mut none), Merge::Nothing);
        let mut other_branch = Facts { main_branch: Some("dev".into()), ..facts() };
        assert_eq!(state_of(&mut other_branch), Merge::Blocked);
        let mut mid = Facts { mid_merge: true, ..facts() };
        assert_eq!(state_of(&mut mid), Merge::Blocked);
        let mut gone = Facts { missing: true, ..facts() };
        assert_eq!(state_of(&mut gone), Merge::Gone);
    }

    fn counters() -> Counters {
        Counters::default()
    }

    #[test]
    fn signals_have_their_edges() {
        assert!(signals(&counters(), &State::Idle, 0, 1).is_empty());
        let c = Counters { loops: 1, scope: 20, guarded: 2, ..counters() };
        let kinds: Vec<_> = signals(&c, &State::Idle, 2, 1).iter().map(|s| s.kind).collect();
        assert_eq!(kinds, ["ask", "loop", "scope", "guard"], "a waiting question first");
        // Long: only while working, only from the edge, never from a clock that went back or a start nobody wrote down.
        let c = Counters { run_ts: 1000, ..counters() };
        assert!(signals(&c, &State::Running, 0, 1000 + LONG_MS - 1).is_empty());
        assert_eq!(signals(&c, &State::Running, 0, 1000 + LONG_MS + 60_000)[0], Signal { kind: "long", n: 11 });
        assert!(signals(&c, &State::Idle, 0, 1000 + LONG_MS).is_empty());
        assert!(signals(&c, &State::Running, 0, 10).is_empty());
        assert!(signals(&counters(), &State::Running, 0, LONG_MS * 9).is_empty());
        // Failed: only for a chat that is idle; context: from 85 percent of the window.
        let failed = Counters { last_ok: Some(false), ..counters() };
        assert_eq!(signals(&failed, &State::Idle, 0, 1)[0].kind, "failed");
        assert!(signals(&failed, &State::Running, 0, 1).is_empty());
        assert!(signals(&Counters { ctx: Some((849, 1000)), ..counters() }, &State::Idle, 0, 1).is_empty());
        assert_eq!(signals(&Counters { ctx: Some((850, 1000)), ..counters() }, &State::Idle, 0, 1), [Signal { kind: "ctx", n: 85 }]);
        assert_eq!(ctx_percent(Some((5, 0))), None);
        assert_eq!(ctx_percent(Some((2000, 1000))), Some(100));
    }

    fn snap(worktree: bool, removed: bool, state: State, pending: usize, title: &str) -> BoardSnap {
        let mut meta = crate::session::new_meta(PathBuf::from("/work/proj"), None, None, false, None);
        meta.title = title.to_owned();
        if worktree {
            meta.worktree = Some(crate::worktree::WorktreeRef { repo: PathBuf::from("/work/proj"), branch: "tenrec/x".into(), base_branch: "main".into(), base_commit: String::new(), removed, root: None, config_hash: String::new() });
        }
        BoardSnap { info: Info { meta, state, pending }, counters: counters(), cost: 0.42 }
    }

    #[test]
    fn a_chat_without_a_worktree_has_no_merge_column_data_and_one_with_nothing_looked_at_is_unknown() {
        let plain = row(&snap(false, false, State::Idle, 0, "a"), None, 1);
        assert!(plain.wt.is_none() && !plain.worktree);
        let fresh = row(&snap(true, false, State::Idle, 0, "a"), None, 1);
        assert_eq!(fresh.wt.map(|w| w.merge), Some(Merge::Unknown));
        let seen = row(&snap(true, false, State::Idle, 0, "a"), Some(WtRow::of(Merge::Ready)), 1);
        assert_eq!(seen.wt.map(|w| w.merge), Some(Merge::Ready));
        // A removed worktree is gone whatever an old look said.
        let gone = row(&snap(true, true, State::Idle, 0, "a"), Some(WtRow::of(Merge::Ready)), 1);
        assert_eq!(gone.wt.map(|w| w.merge), Some(Merge::Gone));
    }

    #[test]
    fn a_row_says_what_the_chat_is_and_masks_the_title() {
        let r = row(&snap(false, false, State::Running, 2, "key sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789 here"), None, 1);
        assert_eq!((r.state, r.pending, r.project.as_str(), r.cost), ("running", 2, "proj", 0.42));
        assert!(!r.title.contains("abcdefghijklmnop"), "{}", r.title);
        assert_eq!(row(&snap(false, false, State::Exited { code: Some(1) }, 0, "x"), None, 1).state, "exited");
    }

    fn key(hash: &str) -> Key {
        Key { config_hash: hash.into(), removed: false, cwd: PathBuf::from("/w") }
    }

    fn at<'a>(now: u64, running: bool, last_ts: u64, k: &'a Key) -> Now<'a> {
        Now { at: now, running, last_ts, key: k }
    }

    fn done(cache: &FactsCache, n: &Now, last: Last) {
        cache.finish("s", n, Outcome::Done(WtRow::of(Merge::Ready), last));
    }

    #[test]
    fn the_first_look_is_at_once_and_a_look_under_way_is_not_started_twice() {
        let cache = Arc::new(FactsCache::new());
        let k = key("h");
        let first = cache.begin("s", &at(1000, false, 10, &k), false);
        assert!(first.is_some());
        assert!(cache.begin("s", &at(1001, false, 10, &k), false).is_none(), "under way");
        assert!(cache.view("s", 1002).is_some_and(|w| w.refreshing && w.merge == Merge::Unknown));
        drop(first);
        assert!(cache.view("s", 1002).is_some_and(|w| !w.refreshing), "ended by whatever ended the look");
    }

    #[test]
    fn a_chat_that_works_is_looked_at_only_when_it_has_done_something_and_not_oftener_than_every_half_minute() {
        let cache = Arc::new(FactsCache::new());
        let k = key("h");
        drop(cache.begin("s", &at(0, true, 10, &k), false));
        done(&cache, &at(0, true, 10, &k), Last::Facts);
        assert!(cache.begin("s", &at(60_000, true, 10, &k), false).is_none(), "nothing happened since");
        assert!(cache.begin("s", &at(RUNNING_EVERY - 1, true, 20, &k), false).is_none(), "too soon");
        assert!(cache.begin("s", &at(RUNNING_EVERY, true, 20, &k), false).is_some());
    }

    #[test]
    fn a_chat_that_has_just_stopped_is_looked_at_once_more_at_once() {
        let cache = Arc::new(FactsCache::new());
        let k = key("h");
        drop(cache.begin("s", &at(0, true, 10, &k), false));
        done(&cache, &at(0, true, 10, &k), Last::Facts);
        assert!(cache.begin("s", &at(1, false, 10, &k), false).is_some(), "running, then idle: its last changes");
    }

    #[test]
    fn an_idle_chat_waits_for_its_ttl_unless_it_has_news_and_an_old_one_waits_longer() {
        let cache = Arc::new(FactsCache::new());
        let k = key("h");
        let now = 10 * OLD_AFTER;
        drop(cache.begin("s", &at(now, false, now - 5, &k), false));
        done(&cache, &at(now, false, now - 5, &k), Last::Facts);
        assert!(cache.begin("s", &at(now + IDLE_TTL - 1, false, now - 5, &k), false).is_none());
        assert!(cache.begin("s", &at(now + AFTER_EVENT, false, now + 1, &k), false).is_some(), "an event since");
        drop(cache.begin("s", &at(now + AFTER_EVENT, false, now + 1, &k), false));
        done(&cache, &at(now + 5000, false, now + 1, &k), Last::Facts);
        assert!(cache.begin("s", &at(now + 5000 + IDLE_TTL, false, now + 1, &k), false).is_some());
        // A chat nobody has touched for a day.
        let cold = at(now + 5000, false, 1000, &k);
        drop(cache.begin("t", &cold, false));
        cache.finish("t", &cold, Outcome::Done(WtRow::of(Merge::Ready), Last::Facts));
        assert!(cache.begin("t", &at(now + 5000 + IDLE_TTL, false, 1000, &k), false).is_none());
        assert!(cache.begin("t", &at(now + 5000 + OLD_TTL, false, 1000, &k), false).is_some());
    }

    #[test]
    fn a_changed_hash_or_removal_is_looked_at_at_once_so_an_accepted_setting_does_not_stay_unsafe_for_minutes() {
        let cache = Arc::new(FactsCache::new());
        let before = key("old");
        drop(cache.begin("s", &at(0, false, 10, &before), false));
        done(&cache, &at(0, false, 10, &before), Last::Unsafe);
        assert!(cache.begin("s", &at(1, false, 10, &before), false).is_none());
        let after = key("new");
        assert!(cache.begin("s", &at(1, false, 10, &after), false).is_some(), "the person accepted the new settings in the chat");
        drop(cache.begin("s", &at(1, false, 10, &after), false));
        done(&cache, &at(1, false, 10, &after), Last::Facts);
        let removed = Key { removed: true, ..after.clone() };
        assert!(cache.begin("s", &at(2, false, 10, &removed), false).is_some());
    }

    #[test]
    fn a_failure_is_tried_again_later_and_later_and_an_unsafe_one_is_checked_every_minute() {
        let cache = Arc::new(FactsCache::new());
        let k = key("h");
        let mut now = 0;
        let mut gaps = Vec::new();
        for _ in 0..6 {
            drop(cache.begin("s", &at(now, false, 10, &k), false));
            done(&cache, &at(now, false, 10, &k), Last::Failed);
            let mut gap = 0;
            while cache.begin("s", &at(now + gap, false, 10, &k), false).is_none() {
                gap += 1000;
            }
            gaps.push(gap);
            now += gap;
        }
        assert_eq!(gaps, [60_000, 120_000, 240_000, 480_000, FAIL_MAX, FAIL_MAX]);
        let cache = Arc::new(FactsCache::new());
        drop(cache.begin("s", &at(0, false, 10, &k), false));
        done(&cache, &at(0, false, 10, &k), Last::Unsafe);
        assert!(cache.begin("s", &at(UNSAFE_TTL - 1, false, 10, &k), false).is_none());
        assert!(cache.begin("s", &at(UNSAFE_TTL, false, 10, &k), false).is_some());
    }

    #[test]
    fn looking_again_is_wanted_by_the_person_but_not_oftener_than_every_few_seconds_for_one_chat() {
        let cache = Arc::new(FactsCache::new());
        let k = key("h");
        drop(cache.begin("s", &at(0, false, 10, &k), false));
        done(&cache, &at(0, false, 10, &k), Last::Facts);
        assert!(cache.begin("s", &at(FORCE_GAP - 1, false, 10, &k), true).is_none());
        assert!(cache.begin("s", &at(FORCE_GAP, false, 10, &k), true).is_some());
    }

    #[test]
    fn a_busy_project_is_tried_again_in_a_few_seconds_and_keeps_what_was_known() {
        let cache = Arc::new(FactsCache::new());
        let k = key("h");
        drop(cache.begin("s", &at(0, false, 10, &k), false));
        done(&cache, &at(0, false, 10, &k), Last::Facts);
        drop(cache.begin("s", &at(IDLE_TTL, false, 10, &k), false));
        cache.finish("s", &at(IDLE_TTL, false, 10, &k), Outcome::Busy);
        assert!(cache.begin("s", &at(IDLE_TTL + FORCE_GAP - 1, false, 10, &k), false).is_none());
        assert!(cache.begin("s", &at(IDLE_TTL + FORCE_GAP, false, 10, &k), false).is_some());
        assert_eq!(cache.view("s", IDLE_TTL).map(|w| w.merge), Some(Merge::Ready), "the old look is still shown");
    }

    #[test]
    fn the_board_is_closed_when_nobody_asks_and_chats_that_are_gone_are_forgotten() {
        let cache = Arc::new(FactsCache::new());
        cache.asked(1000);
        assert!(!cache.abandoned(1000 + ABANDONED_MS - 1));
        assert!(cache.abandoned(1000 + ABANDONED_MS));
        let k = key("h");
        drop(cache.begin("a", &at(0, false, 1, &k), false));
        drop(cache.begin("b", &at(0, false, 1, &k), false));
        cache.keep_only(&["a".to_owned()].into_iter().collect());
        assert_eq!(cache.len(), 1);
    }
}
