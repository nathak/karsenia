//! A chat's events turned into the rows the desktop app draws: the same reading the web page's `handleClaude` did, in Rust and
//! without a DOM. `apply` takes one event of the chat's log (or of its live stream); `rows` flattens what is known into a list,
//! nesting what a sub-agent did under its card. Nothing a model or a tool wrote is ever read as markup: text goes out as text,
//! and an answer's markdown goes through `markdown::blocks`, which escapes it.
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;

use super::i18n::{fill, tr, Lang};
use super::markdown::{blocks, Block};
use super::team::{self, Seen, TeamLine, TeamSeen};
use crate::schedule::{Clock, SystemClock};

/// The image types the desktop can draw as a thumbnail (the rest is told in the message text).
pub const THUMB_TYPES: [&str; 2] = ["image/png", "image/jpeg"];
static NEXT_SHOTS: AtomicU32 = AtomicU32::new(1);

/// The images a user message carried. Cheap to clone (one counter), equal when it is the same message's set, and its debug form
/// never prints the data. `id` is unique in the process, so a view can key its decoded copies by it.
#[derive(Clone)]
pub struct Shots {
    pub id: u32,
    pub list: Arc<[crate::claude::protocol::Image]>,
}
impl PartialEq for Shots {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl std::fmt::Debug for Shots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Shots#{}x{}", self.id, self.list.len())
    }
}

/// One row of the transcript list.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    /// day, meta-user, user, meta-asst, para, heading, item, quote, code, rule, table, think, tool, note, note-err, note-sys
    pub kind: &'static str,
    /// Nesting under sub-agent cards.
    pub depth: i32,
    /// What a click on the row refers to (a tool id, a thinking block).
    pub key: String,
    /// Plain text (a user message, code, a note, a tool's head line, a thinking line).
    pub text: String,
    /// Escaped inline markdown for a paragraph, heading, list item or quote.
    pub md: String,
    pub label: String,
    pub extra: String,
    pub status: String,
    /// running, done, err, guard (tools); live (text or thinking being streamed)
    pub state: &'static str,
    /// The opened part of a tool card or a thinking block.
    pub detail: String,
    pub open: bool,
    pub badge: String,
    pub add: i32,
    pub del: i32,
    /// A list item's indent level.
    pub indent: i32,
    /// The images of a user message.
    pub images: Option<Shots>,
}

#[derive(Debug, Clone)]
enum Item {
    Day(String),
    UserMeta(String),
    User { text: String, images: Option<Shots> },
    AsstMeta { who: String, time: String },
    Text { src: String, blocks: Vec<Block>, live: bool },
    Think { text: String, label: String, live: bool, open: bool },
    Tool(Tool),
    Note { text: String, kind: &'static str },
}

#[derive(Debug, Clone)]
struct Tool {
    id: String,
    name: String,
    head: String,
    extra: String,
    badge: String,
    input: String,
    diff: Vec<(char, String)>,
    /// A TodoWrite's list as lines ("[x] done", "[~] doing", "[ ] to do"); empty for any other tool.
    checklist: String,
    add: i32,
    del: i32,
    state: &'static str,
    status: String,
    result: String,
    open: bool,
    started: Option<Instant>,
}

#[derive(Debug, Clone)]
struct Node {
    item: Item,
    /// The tool card of the sub-agent that made it.
    parent: Option<String>,
}

/// What the header shows about the chat.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Head {
    pub state: String,
    pub model: Option<String>,
    /// The last result's cost and tokens.
    pub turn: Option<(Option<String>, Option<String>)>,
    /// The size of the conversation as of the last model call (tokens), and the model's context window when a result named it.
    pub ctx_used: Option<u64>,
    pub ctx_max: Option<u64>,
    /// Whether the last `system/init` listed the SmartWorkbench plug-in's agents (None before any init).
    pub swb: Option<bool>,
    /// The last team-mode state the chat showed (see `view::team`).
    pub team: Option<TeamSeen>,
    /// How many `/swb` command turns have ended: a button waits for this to change.
    pub team_turns: u32,
}

pub struct Transcript {
    lang: Lang,
    nodes: Vec<Node>,
    tools: HashMap<String, usize>,
    live_text: Option<usize>,
    live_think: Option<(usize, Instant, Option<u64>)>,
    think_ms: Option<u64>,
    asst_meta: bool,
    day: String,
    ts: u64,
    /// The turn in progress answers a `/swb` message, and the status reply it showed so far.
    team_cmd: bool,
    team_cand: Option<TeamLine>,
    pub head: Head,
    /// Permission requests waiting, oldest first (their whole data as the chat sent it).
    pub pending: Vec<Value>,
    /// True while the log is read back: no live durations, no dialog.
    pub replaying: bool,
    /// The chat's first message, for a title.
    pub first_text: Option<String>,
    /// Long code blocks opened in full (by row key).
    expanded: std::collections::HashSet<String>,
}

const MAX_RESULT: usize = 20_000;
const MAX_INPUT: usize = 4_000;

fn cut(text: &str, max: usize, lang: Lang) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n{}", &text[..end], fill(&tr(lang, "cut"), &[("n", max.to_string())]))
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 200 {
        line.chars().take(200).collect::<String>() + "…"
    } else {
        line.to_owned()
    }
}

fn s<'a>(input: &'a Value, key: &str) -> &'a str {
    input.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The short label of an agent or an MCP server: `smartworkbench:advisor` → advisor, `mcp__plugin_vhdb_vhdb__x` → vhdb.
fn badge_of(name: &str, input: &Value) -> String {
    if name == "Task" || name == "Agent" {
        let kind = s(input, "subagent_type");
        return kind.rsplit(':').next().unwrap_or("").chars().take(24).collect();
    }
    if let Some(rest) = name.strip_prefix("mcp__") {
        let server = rest.split("__").next().unwrap_or("");
        return match server {
            "plugin_vhdb_vhdb" => "vhdb".into(),
            "smartworkbench" => "swb".into(),
            other => other.trim_start_matches("plugin_").chars().take(16).collect(),
        };
    }
    String::new()
}

/// A tool call's one-line head and a small extra (a line range, a path, a count).
pub fn tool_head(name: &str, input: &Value) -> (String, String) {
    match name {
        "Bash" => (first_line(s(input, "command")), first_line(s(input, "description"))),
        "Read" => {
            let range = match (input.get("offset").and_then(Value::as_u64), input.get("limit").and_then(Value::as_u64)) {
                (Some(o), Some(l)) => format!("L{o}-{}", o + l),
                (Some(o), None) => format!("L{o}-"),
                (None, Some(l)) => format!("L1-{l}"),
                _ => String::new(),
            };
            (s(input, "file_path").to_owned(), range)
        }
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => (s(input, "file_path").to_owned(), String::new()),
        "Grep" => (s(input, "pattern").to_owned(), [s(input, "path"), s(input, "glob")].iter().filter(|x| !x.is_empty()).copied().collect::<Vec<_>>().join(" ")),
        "Glob" => (s(input, "pattern").to_owned(), s(input, "path").to_owned()),
        "WebFetch" => (s(input, "url").to_owned(), String::new()),
        "WebSearch" => (s(input, "query").to_owned(), String::new()),
        "Task" | "Agent" => (first_line(s(input, "description")), String::new()),
        "TodoWrite" => {
            let todos = input.get("todos").and_then(Value::as_array);
            let n = todos.map_or(0, Vec::len);
            let done = todos.map_or(0, |t| t.iter().filter(|x| s(x, "status") == "completed").count());
            (format!("{done}/{n} todos"), String::new())
        }
        _ => {
            // The first text argument, whatever its name.
            let first = input.as_object().and_then(|m| m.values().find_map(Value::as_str)).map(first_line).unwrap_or_default();
            (first, String::new())
        }
    }
}

/// A TodoWrite's list as text, one line per item (at most 50 lines of 200 characters).
fn checklist(name: &str, input: &Value) -> String {
    if name != "TodoWrite" {
        return String::new();
    }
    let mark = |status: &str| -> &str { match status {
        "completed" => "[x]",
        "in_progress" => "[~]",
        _ => "[ ]",
    } };
    input
        .get("todos")
        .and_then(Value::as_array)
        .map(|todos| {
            todos
                .iter()
                .take(50)
                .map(|t| {
                    let text: String = t["content"].as_str().unwrap_or_default().chars().filter(|c| !c.is_control()).take(200).collect();
                    format!("{} {text}", mark(s(t, "status")))
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// The changed lines of an Edit, MultiEdit or Write, with the counts. At most 400 lines are kept.
fn edit_diff(name: &str, input: &Value) -> (Vec<(char, String)>, i32, i32) {
    let mut lines = Vec::new();
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    match name {
        "Edit" => pairs.push((s(input, "old_string"), s(input, "new_string"))),
        "MultiEdit" => {
            for e in input.get("edits").and_then(Value::as_array).into_iter().flatten() {
                pairs.push((s(e, "old_string"), s(e, "new_string")));
            }
        }
        "Write" => pairs.push(("", s(input, "content"))),
        _ => return (lines, 0, 0),
    }
    let (mut add, mut del) = (0, 0);
    for (old, new) in pairs {
        for l in old.lines() {
            del += 1;
            lines.push(('-', l.to_owned()));
        }
        for l in new.lines() {
            add += 1;
            lines.push(('+', l.to_owned()));
        }
    }
    lines.truncate(400);
    (lines, add, del)
}

/// The text of a tool result: a string, or the text blocks of a list.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(t) => t.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|p| match p.get("type").and_then(Value::as_str) {
                Some("text") => s(p, "text").to_owned(),
                Some("image") => "[image]".to_owned(),
                _ => String::new(),
            })
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// A result held by a plug-in rule, told by the start of its text only.
pub fn guard_kind(text: &str) -> Option<&'static str> {
    let head: String = text.chars().take(400).collect();
    if head.contains("SmartWorkbench team mode:") {
        Some("team")
    } else if head.to_ascii_lowercase().contains("vhdb guard") {
        Some("vhdb")
    } else {
        None
    }
}

/// `2.4s · $0.0123` from a result.
pub fn fmt_turn(result: &Value) -> String {
    let mut bits = Vec::new();
    if let Some(ms) = result.get("duration_ms").and_then(Value::as_f64) {
        bits.push(format!("{:.1}s", ms / 1000.0));
    }
    if let Some(cost) = result.get("total_cost_usd").and_then(Value::as_f64) {
        bits.push(format!("${cost:.4}"));
    }
    bits.join(" · ")
}

/// The header's cost and tokens of the last result.
pub fn turn_line(result: &Value) -> Option<(Option<String>, Option<String>)> {
    let cost = result.get("total_cost_usd").and_then(Value::as_f64).map(|c| format!("${c:.3}"));
    let tokens = result.get("usage").map(|u| ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens", "output_tokens"].iter().map(|k| u.get(*k).and_then(Value::as_u64).unwrap_or(0)).sum::<u64>()).filter(|n| *n > 0).map(|n| if n >= 1000 { format!("{:.1}k", n as f64 / 1000.0) } else { n.to_string() });
    (cost.is_some() || tokens.is_some()).then_some((cost, tokens))
}

/// Tokens of one model call: what it was given and what it wrote.
fn call_tokens(usage: &Value) -> u64 {
    ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens", "output_tokens"].iter().map(|k| usage.get(*k).and_then(Value::as_u64).unwrap_or(0)).sum()
}

/// The context window a result's `modelUsage` names: the entry of the chat's model, else the largest one listed.
fn context_window(result: &Value, model: Option<&str>) -> Option<u64> {
    let usage = result.get("modelUsage")?.as_object()?;
    let window = |v: &Value| v.get("contextWindow").and_then(Value::as_u64).filter(|n| *n > 0);
    model.and_then(|m| usage.get(m)).and_then(window).or_else(|| usage.values().filter_map(window).max())
}

fn short_count(n: u64) -> String {
    match n {
        1_000_000.. => format!("{:.1}M", n as f64 / 1_000_000.0).replace(".0M", "M"),
        10_000.. => format!("{}k", n / 1000),
        1000.. => format!("{:.1}k", n as f64 / 1000.0).replace(".0k", "k"),
        _ => n.to_string(),
    }
}

/// "45k / 200k" when the window is known, else None (the header then shows the old per-turn count).
pub fn context_line(used: Option<u64>, max: Option<u64>) -> Option<String> {
    let (used, max) = (used?, max?);
    Some(format!("{} / {}", short_count(used), short_count(max)))
}

/// `claude-sonnet-4-5-20250929` → `sonnet-4-5`.
pub fn model_label(id: &str) -> String {
    let id = id.strip_prefix("claude-").unwrap_or(id);
    let id = match id.rfind('-') {
        Some(i) if id[i + 1..].len() == 8 && id[i + 1..].chars().all(|c| c.is_ascii_digit()) => &id[..i],
        _ => id,
    };
    id.chars().take(40).collect()
}

fn local(ms: u64) -> Option<(String, String)> {
    if ms == 0 {
        return None;
    }
    let l = SystemClock.local(ms as i64);
    Some((format!("{:04}-{:02}-{:02}", l.year, l.month, l.day), format!("{:02}:{:02}", l.hour, l.minute)))
}

impl Transcript {
    pub fn new(lang: Lang) -> Self {
        Self { lang, nodes: Vec::new(), tools: HashMap::new(), live_text: None, live_think: None, think_ms: None, asst_meta: false, day: String::new(), ts: 0, team_cmd: false, team_cand: None, head: Head::default(), pending: Vec::new(), replaying: true, first_text: None, expanded: Default::default() }
    }

    fn t(&self, key: &str) -> String {
        tr(self.lang, key)
    }

    fn push(&mut self, item: Item, parent: Option<String>) -> usize {
        self.nodes.push(Node { item, parent });
        self.nodes.len() - 1
    }

    fn day_mark(&mut self) {
        let Some((day, time)) = local(self.ts) else { return };
        if day == self.day {
            return;
        }
        let today = local(crate::now_ms()).map(|d| d.0).unwrap_or_default();
        let label = if day == today { self.t("today") } else { day.clone() };
        self.day = day;
        self.push(Item::Day(format!("{label} · {time}")), None);
    }

    fn asst_meta(&mut self) {
        if self.asst_meta {
            return;
        }
        self.asst_meta = true;
        self.day_mark();
        let who = self.head.model.as_deref().map(model_label).filter(|m| !m.is_empty()).unwrap_or_else(|| "Claude".into());
        let time = local(self.ts).map(|d| d.1).unwrap_or_default();
        self.push(Item::AsstMeta { who, time }, None);
    }

    fn note(&mut self, text: String, kind: &'static str) {
        if !text.trim().is_empty() {
            self.push(Item::Note { text, kind }, None);
        }
    }

    fn end_live(&mut self) {
        // The live thinking block may come after the live text: the later one goes first, so the other's index stays right.
        let text = self.live_text;
        let think = self.live_think.map(|(i, _, _)| i);
        if think.is_some() && think > text {
            self.end_live_think();
        }
        if let Some(i) = self.live_text.take() {
            self.nodes.remove(i);
            self.reindex();
        }
        self.end_live_think();
    }

    fn end_live_think(&mut self) {
        if let Some((i, t0, _)) = self.live_think.take() {
            if self.think_ms.is_none() {
                self.think_ms = Some(t0.elapsed().as_millis() as u64);
            }
            self.nodes.remove(i);
            self.reindex();
        }
    }

    fn reindex(&mut self) {
        self.tools.clear();
        for (i, n) in self.nodes.iter().enumerate() {
            if let Item::Tool(t) = &n.item {
                self.tools.insert(t.id.clone(), i);
            }
        }
    }

    fn think_label(&self, chars: usize, ms: Option<u64>) -> String {
        let secs = ms.map(|m| (m / 1000).max(1).to_string());
        if chars == 0 {
            return match secs {
                Some(s) => format!("{} · {}", self.t("think_hidden"), fill(&self.t("think_secs"), &[("s", s)])),
                None => self.t("think_hidden"),
            };
        }
        match secs {
            Some(s) => fill(&self.t("think_sum"), &[("s", s), ("n", chars.to_string())]),
            None => fill(&self.t("think_sum_nt"), &[("n", chars.to_string())]),
        }
    }

    /// One event of the chat (`kind`, `data`, its time in ms, 0 when unknown).
    pub fn apply(&mut self, kind: &str, data: &Value, ts: u64) {
        self.ts = ts;
        match kind {
            "user" => {
                self.asst_meta = false;
                self.day_mark();
                let time = local(ts).map(|d| d.1).unwrap_or_default();
                self.push(Item::UserMeta(time), None);
                let mut text = s(data, "text").to_owned();
                self.team_cmd = team::is_swb_command(&text);
                self.team_cand = None;
                if self.first_text.is_none() && !text.is_empty() && !crate::session::is_slash(&text) {
                    self.first_text = Some(text.chars().take(60).collect());
                }
                let list: Vec<crate::claude::protocol::Image> = data
                    .get("images")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|i| Some(crate::claude::protocol::Image { media_type: i["media_type"].as_str()?.to_owned(), data: i["data"].as_str().filter(|d| !d.is_empty())?.to_owned() })).collect())
                    .unwrap_or_default();
                // What cannot be drawn as a thumbnail is still told in the text.
                let plain = data.get("images").and_then(Value::as_array).map_or(0, Vec::len) - list.iter().filter(|i| THUMB_TYPES.contains(&i.media_type.as_str())).count();
                if plain > 0 {
                    text = format!("{}\n{}", fill(&self.t("images"), &[("n", plain.to_string())]), text).trim().to_owned();
                }
                let images = (!list.is_empty()).then(|| Shots { id: NEXT_SHOTS.fetch_add(1, Ordering::Relaxed), list: list.into() });
                self.push(Item::User { text, images }, None);
            }
            "claude" => self.claude(data),
            // A conversation copied from a session of the `claude` command line program: said once, above what was copied.
            "imported" => {
                let mut text = fill(&self.t("imp_note"), &[("n", data["total"].as_u64().unwrap_or(0).saturating_sub(data["dropped"].as_u64().unwrap_or(0)).to_string())]);
                if let Some(m) = data["dropped"].as_u64().filter(|m| *m > 0) {
                    text.push_str(&fill(&self.t("imp_note_cut"), &[("m", m.to_string())]));
                }
                self.note(text, "note");
            }
            "permission" => {
                let id = s(data, "request_id");
                if !self.pending.iter().any(|p| s(p, "request_id") == id) {
                    self.pending.push(data.clone());
                }
            }
            "permission_resolved" => {
                let id = s(data, "request_id").to_owned();
                self.pending.retain(|p| s(p, "request_id") != id);
                if !self.replaying {
                    let outcome = s(data, "outcome");
                    let text = if ["allowed", "denied", "cancelled"].contains(&outcome) { self.t(outcome) } else { outcome.to_owned() };
                    self.note(text, "note");
                }
            }
            "state" => self.head.state = s(data, "state").to_owned(),
            "memory_used" => {
                let n = data.get("ids").and_then(Value::as_array).map_or(0, Vec::len);
                self.note(format!("{}: {n}", self.t("mem_used")), "note");
            }
            "skills_given" => {
                let names: Vec<&str> = data.get("names").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
                self.note(format!("{}: {}", self.t("sk_given"), names.join(", ")), "note");
            }
            "stderr" => {
                let text = s(data, "text").trim();
                let tail: String = text.chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect();
                self.note(tail, "note-err");
            }
            "error" => self.note(s(data, "message").to_owned(), "note-err"),
            _ => {}
        }
    }

    fn claude(&mut self, d: &Value) {
        let parent = d.get("parent_tool_use_id").and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned);
        match s(d, "type") {
            "stream_event" => {
                if self.replaying || parent.is_some() {
                    return;
                }
                let ev = d.get("event").cloned().unwrap_or(Value::Null);
                let delta = ev.get("delta").cloned().unwrap_or(Value::Null);
                match (s(&ev, "type"), s(&delta, "type")) {
                    ("content_block_start", _) if ev.pointer("/content_block/type").and_then(Value::as_str) == Some("thinking") => self.start_think(),
                    ("content_block_delta", "thinking_delta") => {
                        self.start_think();
                        let tokens = delta.get("estimated_tokens").and_then(Value::as_u64);
                        if let Some((i, _, est)) = self.live_think.as_mut() {
                            if tokens.is_some() {
                                *est = tokens;
                            }
                            if let Item::Think { text, .. } = &mut self.nodes[*i].item {
                                text.push_str(s(&delta, "thinking"));
                            }
                        }
                        self.relabel_live_think();
                    }
                    ("content_block_stop", _) => {
                        // Only the first stop of the live block is timed (a later block's stop does not move it).
                        if let Some((i, t0, _)) = self.live_think.filter(|(i, _, _)| matches!(self.nodes.get(*i).map(|n| &n.item), Some(Item::Think { live: true, .. }))) {
                            self.think_ms = Some(t0.elapsed().as_millis() as u64);
                            let chars = match &self.nodes[i].item {
                                Item::Think { text, .. } => text.chars().count(),
                                _ => 0,
                            };
                            let label = self.think_label(chars, self.think_ms);
                            if let Item::Think { label: l, live, open, .. } = &mut self.nodes[i].item {
                                *l = label;
                                *live = false;
                                *open = false;
                            }
                        }
                    }
                    ("content_block_delta", "text_delta") => {
                        let piece = s(&delta, "text").to_owned();
                        match self.live_text {
                            Some(i) => {
                                if let Item::Text { src, blocks: b, .. } = &mut self.nodes[i].item {
                                    src.push_str(&piece);
                                    *b = blocks(src);
                                }
                            }
                            None => {
                                self.asst_meta();
                                let i = self.push(Item::Text { blocks: blocks(&piece), src: piece, live: true }, None);
                                self.live_text = Some(i);
                            }
                        }
                    }
                    _ => {}
                }
            }
            "assistant" => {
                if parent.is_none() {
                    self.end_live();
                    // The last call's prompt plus its answer is how much of the window the chat takes now (a result's own
                    // usage adds up every call of the turn, so it says more than that).
                    if let Some(n) = d.pointer("/message/usage").map(call_tokens).filter(|n| *n > 0) {
                        self.head.ctx_used = Some(n);
                    }
                }
                let think_ms = if parent.is_none() { self.think_ms.take() } else { None };
                for block in d.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                    if parent.is_none() {
                        self.asst_meta();
                    }
                    match s(block, "type") {
                        "text" => {
                            let text = s(block, "text");
                            if parent.is_none() && self.team_cmd {
                                if let Some(line) = team::parse_team_line(text) {
                                    self.team_cand = Some(line);
                                }
                            }
                            if !text.is_empty() {
                                self.push(Item::Text { src: text.to_owned(), blocks: blocks(text), live: false }, parent.clone());
                            }
                        }
                        "thinking" | "redacted_thinking" => {
                            let text = if s(block, "type") == "thinking" { s(block, "thinking").to_owned() } else { String::new() };
                            let label = self.think_label(text.chars().count(), think_ms);
                            self.push(Item::Think { text, label, live: false, open: false }, parent.clone());
                        }
                        "tool_use" => self.tool_use(block, parent.clone()),
                        _ => {}
                    }
                }
            }
            "user" => {
                for block in d.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                    if s(block, "type") == "tool_result" {
                        self.tool_result(block);
                    }
                }
            }
            "result" => {
                self.end_live();
                self.think_ms = None;
                self.asst_meta = false;
                // A command the plug-in answered itself: no model was called, so there is no cost to show.
                let zero = s(d, "subtype") == "success" && d.get("num_turns").and_then(Value::as_u64) == Some(0) && d.get("is_error").and_then(Value::as_bool) != Some(true);
                if self.team_cmd {
                    // The state counts only for a `/swb` message answered without a model: a sentence a model wrote, or one a
                    // file carried into its answer, does not change it.
                    if zero {
                        let line = self.team_cand.take().or_else(|| team::parse_team_line(s(d, "result")));
                        if let Some(line) = line {
                            self.head.team = Some(TeamSeen { line, ts: self.ts, source: Seen::Line });
                        }
                    }
                    self.head.team_turns += 1;
                    self.team_cmd = false;
                    self.team_cand = None;
                }
                if zero {
                    return;
                }
                if let Some(turn) = turn_line(d) {
                    self.head.turn = Some(turn);
                }
                if let Some(max) = context_window(d, self.head.model.as_deref()) {
                    self.head.ctx_max = Some(max);
                }
                let line = fmt_turn(d);
                if s(d, "subtype") == "error_during_execution" {
                    let text = if line.is_empty() { self.t("interrupted") } else { format!("{} · {line}", self.t("interrupted")) };
                    self.note(text, "note");
                } else if d.get("is_error").and_then(Value::as_bool) == Some(true) {
                    let text = d.get("result").and_then(Value::as_str).unwrap_or(s(d, "subtype")).to_owned();
                    self.note(text, "note-err");
                } else {
                    self.note(line, "note");
                }
            }
            "system" if s(d, "subtype") == "init" => {
                if let Some(model) = d.get("model").and_then(Value::as_str) {
                    self.head.model = Some(model.to_owned());
                }
                self.head.swb = Some(d.get("agents").and_then(Value::as_array).is_some_and(|a| a.iter().any(|x| x.as_str().is_some_and(|n| n.starts_with("smartworkbench:")))));
            }
            _ => {}
        }
    }

    fn start_think(&mut self) {
        if self.live_think.is_some() {
            return;
        }
        self.asst_meta();
        let i = self.push(Item::Think { text: String::new(), label: self.t("thinking"), live: true, open: true }, None);
        self.live_think = Some((i, Instant::now(), None));
    }

    fn relabel_live_think(&mut self) {
        let Some((i, _, est)) = self.live_think else { return };
        let label = match est {
            Some(n) => format!("{} · {}", self.t("thinking"), fill(&self.t("think_tok"), &[("n", n.to_string())])),
            None => self.t("thinking"),
        };
        if let Item::Think { label: l, live: true, .. } = &mut self.nodes[i].item {
            *l = label;
        }
    }

    fn tool_use(&mut self, block: &Value, parent: Option<String>) {
        let id = s(block, "id").to_owned();
        let name = s(block, "name").to_owned();
        let input = block.get("input").cloned().unwrap_or(Value::Null);
        let (head, extra) = tool_head(&name, &input);
        let (diff, add, del) = edit_diff(&name, &input);
        let checklist = checklist(&name, &input);
        let pretty = serde_json::to_string_pretty(&input).unwrap_or_default();
        let tool = Tool {
            badge: badge_of(&name, &input),
            input: cut(&pretty, MAX_INPUT, self.lang),
            id: id.clone(),
            name,
            head,
            extra,
            diff,
            checklist,
            add,
            del,
            state: "running",
            status: self.t("running_tool"),
            result: String::new(),
            open: false,
            started: (!self.replaying).then(Instant::now),
        };
        // The same call sent again (a fuller version of it) replaces the card in place.
        if let Some(&i) = self.tools.get(&id) {
            if let Item::Tool(old) = &self.nodes[i].item {
                let mut fresh = tool;
                fresh.open = old.open;
                fresh.started = old.started;
                self.nodes[i].item = Item::Tool(fresh);
            }
            return;
        }
        let i = self.push(Item::Tool(tool), parent);
        self.tools.insert(id, i);
    }

    fn tool_result(&mut self, block: &Value) {
        let Some(&i) = self.tools.get(s(block, "tool_use_id")) else { return };
        let text = result_text(block.get("content").unwrap_or(&Value::Null));
        let is_error = block.get("is_error").and_then(Value::as_bool) == Some(true);
        let guard = if is_error { guard_kind(&text) } else { None };
        // Team mode held an edit here, so it was on at this time (the roles last seen are kept).
        if guard == Some("team") {
            let roles = self.head.team.as_ref().map(|t| t.line.roles.clone()).unwrap_or_default();
            self.head.team = Some(TeamSeen { line: TeamLine { on: true, roles, opus_reviews: None, raw: String::new() }, ts: self.ts, source: Seen::Guard });
        }
        let lang = self.lang;
        let (state, mut status) = match guard {
            Some("team") => ("guard", tr(lang, "guard_team")),
            Some(_) => ("guard", tr(lang, "guard_vhdb")),
            None if is_error => ("err", tr(lang, "error")),
            None => ("done", tr(lang, "done")),
        };
        if let Item::Tool(t) = &mut self.nodes[i].item {
            if let Some(t0) = t.started {
                let ms = t0.elapsed().as_millis();
                if ms >= 100 {
                    status = if ms >= 1000 { format!("{status} · {:.1}s", ms as f64 / 1000.0) } else { format!("{status} · {ms}ms") };
                }
            }
            t.state = state;
            t.status = status;
            t.result = cut(&text, MAX_RESULT, lang);
        }
    }

    /// Opens or closes a tool card, a thinking block, or a long code block.
    pub fn toggle(&mut self, key: &str) {
        if key.contains('-') && key.starts_with('n') {
            if !self.expanded.remove(key) {
                self.expanded.insert(key.to_owned());
            }
            return;
        }
        if let Some(&i) = self.tools.get(key) {
            if let Item::Tool(t) = &mut self.nodes[i].item {
                t.open = !t.open;
            }
            return;
        }
        if let Some(i) = key.strip_prefix('n').and_then(|n| n.parse::<usize>().ok()) {
            if let Some(Node { item: Item::Think { open, .. }, .. }) = self.nodes.get_mut(i) {
                *open = !*open;
            }
        }
    }

    /// The rows to draw, a sub-agent's work under its card.
    pub fn rows(&self) -> Vec<Row> {
        let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut top = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            match n.parent.as_deref() {
                Some(p) if self.tools.contains_key(p) => children.entry(p).or_default().push(i),
                Some(_) => top.push(i),
                None => top.push(i),
            }
        }
        let mut out = Vec::new();
        for i in top {
            let depth = i32::from(self.nodes[i].parent.is_some());
            self.emit(i, depth, &children, &mut out);
        }
        out
    }

    fn emit(&self, i: usize, depth: i32, children: &HashMap<&str, Vec<usize>>, out: &mut Vec<Row>) {
        let row = |kind: &'static str| Row { kind, depth, key: format!("n{i}"), ..Row::default() };
        match &self.nodes[i].item {
            Item::Day(text) => out.push(Row { text: text.clone(), ..row("day") }),
            Item::UserMeta(time) => out.push(Row { text: self.t("you"), extra: time.clone(), ..row("meta-user") }),
            Item::User { text, images } => out.push(Row { text: text.clone(), images: images.clone(), ..row("user") }),
            Item::AsstMeta { who, time } => out.push(Row { text: who.clone(), extra: time.clone(), ..row("meta-asst") }),
            Item::Text { blocks, live, src } => {
                let start = out.len();
                for (b, block) in blocks.iter().enumerate() {
                    let key = format!("n{i}-{b}");
                    out.push(match block {
                        Block::Para(md) => Row { md: md.clone(), key, ..row("para") },
                        Block::Heading(level, md) => Row { md: format!("**{md}**"), extra: level.to_string(), key, ..row("heading") },
                        Block::Item { depth: d, marker, md } => Row { md: md.clone(), label: marker.clone(), indent: i32::from(*d), key, ..row("item") },
                        Block::Quote(md) => Row { md: md.clone(), key, ..row("quote") },
                        Block::Code { lang, text } => {
                            // A block of more than 30 lines shows its first 12 until it is opened.
                            let n = text.lines().count().max(1);
                            let foldable = n > 30;
                            let open = self.expanded.contains(&key);
                            let shown = if foldable && !open { text.lines().take(12).collect::<Vec<_>>().join("\n") } else { text.clone() };
                            Row { text: shown, label: if lang.is_empty() { "text".into() } else { lang.to_lowercase() }, extra: fill(&self.t("lines"), &[("n", n.to_string())]), detail: if foldable { fill(&self.t(if open { "fold" } else { "show_all" }), &[("n", n.to_string())]) } else { String::new() }, open, add: i32::from(foldable), key, ..row("code") }
                        }
                        Block::Rule => Row { key, ..row("rule") },
                        Block::Table(text) => Row { text: text.clone(), key, ..row("table") },
                    });
                }
                if out.len() == start && !src.trim().is_empty() {
                    out.push(Row { md: super::markdown::escape(src), ..row("para") });
                }
                if *live {
                    if let Some(last) = out.last_mut() {
                        last.state = "live";
                    }
                }
            }
            Item::Think { text, label, live, open } => {
                out.push(Row { text: label.clone(), detail: if *open || *live { text.clone() } else { String::new() }, open: *open || *live, state: if *live { "live" } else { "" }, add: i32::from(!text.is_empty()), ..row("think") });
            }
            Item::Tool(t) => {
                let kids = children.get(t.id.as_str());
                let mut detail = String::new();
                if t.open {
                    if !t.checklist.is_empty() {
                        detail.push_str(&t.checklist);
                    } else if t.diff.is_empty() {
                        detail.push_str(&format!("── {} ──\n{}", self.t("input"), t.input));
                    } else {
                        detail.push_str(&t.diff.iter().map(|(m, l)| format!("{m} {l}")).collect::<Vec<_>>().join("\n"));
                    }
                    if !t.result.is_empty() {
                        detail.push_str(&format!("\n\n── {} ──\n{}", self.t("result"), t.result));
                    }
                }
                let extra = match kids {
                    Some(k) if !k.is_empty() => {
                        let n = k.iter().filter(|&&c| matches!(self.nodes[c].item, Item::Tool(_))).count();
                        if n > 0 { fill(&self.t("sub_tools"), &[("n", n.to_string())]) } else { t.extra.clone() }
                    }
                    _ => t.extra.clone(),
                };
                out.push(Row { key: t.id.clone(), label: t.name.clone(), text: t.head.clone(), extra, badge: t.badge.clone(), status: t.status.clone(), state: t.state, detail, open: t.open, add: t.add, del: t.del, ..row("tool") });
                for &c in kids.into_iter().flatten() {
                    self.emit(c, depth + 1, children, out);
                }
            }
            Item::Note { text, kind } => out.push(Row { text: text.clone(), ..row(kind) }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user_with(images: Value) -> Transcript {
        let mut t = chat();
        t.apply("user", &json!({"text": "hi", "images": images}), 1000);
        t
    }
    fn user_row(t: &Transcript) -> Row {
        t.rows().into_iter().find(|r| r.kind == "user").unwrap()
    }

    #[test]
    fn user_images_ride_on_the_row_without_a_text_note() {
        let row = user_row(&user_with(json!([{"media_type": "image/png", "data": "AAAA"}, {"media_type": "image/jpeg", "data": "BBBB"}])));
        assert_eq!(row.images.as_ref().unwrap().list.len(), 2);
        assert_eq!(row.text, "hi");
    }

    #[test]
    fn an_undecodable_image_keeps_the_text_note() {
        let row = user_row(&user_with(json!([{"media_type": "image/png", "data": "AAAA"}, {"media_type": "image/gif", "data": "BBBB"}])));
        assert!(row.text.contains('1') && row.text.ends_with("hi") && row.text != "hi", "{}", row.text);
        assert_eq!(row.images.as_ref().unwrap().list.len(), 2);
    }

    #[test]
    fn no_images_means_none() {
        assert!(user_row(&user_with(json!([]))).images.is_none());
        assert_eq!(user_row(&user_with(json!([]))).text, "hi");
    }

    #[test]
    fn user_row_stays_equal_while_a_turn_streams() {
        let mut t = user_with(json!([{"media_type": "image/png", "data": "AAAA"}]));
        let before = user_row(&t);
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "ok"}]}}), 2000);
        assert_eq!(before, user_row(&t));
    }

    #[test]
    fn two_messages_with_the_same_image_get_distinct_ids_and_debug_hides_data() {
        let a = user_row(&user_with(json!([{"media_type": "image/png", "data": "SECRETDATA"}])));
        let b = user_row(&user_with(json!([{"media_type": "image/png", "data": "SECRETDATA"}])));
        assert_ne!(a.images, b.images);
        assert!(!format!("{a:?}").contains("SECRETDATA"));
    }

    fn chat() -> Transcript {
        let mut t = Transcript::new(Lang::Ko);
        t.replaying = false;
        t
    }

    #[test]
    fn a_turn_becomes_rows_and_a_tool_gets_its_result() {
        let mut t = chat();
        t.apply("user", &json!({"text": "hello"}), 0);
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "**hi** <b>x</b>"}, {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls -la\nmore"}}]}}), 0);
        let rows = t.rows();
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, vec!["meta-user", "user", "meta-asst", "para", "tool"]);
        assert_eq!(rows[3].md, r"**hi** \<b\>x\<\/b\>");
        assert_eq!(rows[4].text, "ls -la");
        assert_eq!(rows[4].state, "running");
        t.apply("claude", &json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": "out"}]}]}}), 0);
        let tool = t.rows().into_iter().find(|r| r.kind == "tool").unwrap();
        assert_eq!(tool.state, "done");
        assert!(tool.detail.is_empty(), "closed card shows no detail");
        t.toggle("t1");
        let tool = t.rows().into_iter().find(|r| r.kind == "tool").unwrap();
        assert!(tool.open && tool.detail.contains("out") && tool.detail.contains("ls -la"));
    }

    #[test]
    fn a_guard_is_told_from_an_error_and_a_sub_agent_nests() {
        let mut t = chat();
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "a", "name": "Agent", "input": {"description": "design", "subagent_type": "smartworkbench:advisor"}}]}}), 0);
        t.apply("claude", &json!({"type": "assistant", "parent_tool_use_id": "a", "message": {"content": [{"type": "tool_use", "id": "r", "name": "Read", "input": {"file_path": "/x", "offset": 1, "limit": 9}}]}}), 0);
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "e", "name": "Edit", "input": {"file_path": "/p", "old_string": "a", "new_string": "b"}}]}}), 0);
        t.apply("claude", &json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "e", "is_error": true, "content": "SmartWorkbench team mode: this edit is not small"}]}}), 0);
        let rows = t.rows();
        let tools: Vec<(&str, i32, &str)> = rows.iter().filter(|r| r.kind == "tool").map(|r| (r.label.as_str(), r.depth, r.state)).collect();
        assert_eq!(tools, vec![("Agent", 0, "running"), ("Read", 1, "running"), ("Edit", 0, "guard")]);
        let agent = rows.iter().find(|r| r.label == "Agent").unwrap();
        assert_eq!(agent.badge, "advisor");
        assert_eq!(agent.extra, "도구 1개");
        let read = rows.iter().find(|r| r.label == "Read").unwrap();
        assert_eq!(read.extra, "L1-10");
        let edit = rows.iter().find(|r| r.label == "Edit").unwrap();
        assert_eq!((edit.add, edit.del), (1, 1));
    }

    fn say(t: &mut Transcript, text: &str, ts: u64) {
        t.apply("user", &json!({"text": text}), ts);
    }
    fn reply(t: &mut Transcript, text: &str, turns: u64, ts: u64) {
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}), ts);
        t.apply("claude", &json!({"type": "result", "subtype": "success", "is_error": false, "num_turns": turns, "total_cost_usd": 0.0, "duration_ms": 3, "result": text}), ts);
    }
    const OFF_LINE: &str = "smartworkbench: Team mode off: lead sonnet (default) · explorer haiku (low) · advisor opus (high). Opus reviews this session: 0.";
    const ON_LINE: &str = "smartworkbench: Team mode on: lead sonnet (default) builds, explorer haiku (low) explores, advisor opus (high) reviews.";

    #[test]
    fn a_plugin_status_reply_sets_the_team_state_and_a_reopened_chat_restores_it() {
        let mut t = chat();
        t.apply("claude", &json!({"type": "system", "subtype": "init", "model": "claude-sonnet-4-5", "agents": ["x", "smartworkbench:advisor"]}), 1);
        assert_eq!(t.head.swb, Some(true));
        say(&mut t, "/swb team", 10);
        reply(&mut t, OFF_LINE, 0, 11);
        let seen = t.head.team.clone().expect("team");
        assert!(!seen.line.on && seen.line.roles.len() == 3 && seen.ts == 11 && seen.source == Seen::Line);
        assert_eq!(t.head.team_turns, 1);
        // A zero-turn reply makes no cost note and leaves the header's cost alone.
        assert!(t.head.turn.is_none());
        assert!(t.rows().iter().all(|r| r.kind != "note"), "{:?}", t.rows().iter().map(|r| r.kind).collect::<Vec<_>>());
        say(&mut t, "/swb team on", 20);
        reply(&mut t, ON_LINE, 0, 21);
        assert!(t.head.team.as_ref().expect("team").line.on);
        // The same events from the start give the same state.
        let mut again = chat();
        again.apply("claude", &json!({"type": "system", "subtype": "init", "agents": ["smartworkbench:advisor"]}), 1);
        for (kind, text, ts) in [("u", "/swb team", 10), ("r", OFF_LINE, 11), ("u", "/swb team on", 20), ("r", ON_LINE, 21)] {
            if kind == "u" { say(&mut again, text, ts) } else { reply(&mut again, text, 0, ts) }
        }
        assert_eq!(again.head.team, t.head.team);
    }

    #[test]
    fn a_line_a_model_wrote_does_not_change_the_state() {
        let mut t = chat();
        say(&mut t, "tell me something", 10);
        reply(&mut t, ON_LINE, 1, 11);
        assert!(t.head.team.is_none());
        // Even for a /swb message: a turn that called the model is not the plug-in speaking.
        say(&mut t, "/swb team", 20);
        reply(&mut t, ON_LINE, 1, 21);
        assert!(t.head.team.is_none());
        // Without a text event the result's own text is used (the plug-in answered).
        say(&mut t, "/swb team", 30);
        t.apply("claude", &json!({"type": "result", "subtype": "success", "num_turns": 0, "result": OFF_LINE}), 31);
        assert!(!t.head.team.as_ref().expect("team").line.on);
    }

    #[test]
    fn an_interrupted_command_turn_keeps_its_note_and_does_not_change_the_state() {
        let mut t = chat();
        say(&mut t, "/swb team", 10);
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "text", "text": ON_LINE}]}}), 11);
        t.apply("claude", &json!({"type": "result", "subtype": "error_during_execution", "num_turns": 0, "is_error": false}), 12);
        assert!(t.head.team.is_none());
        assert!(t.rows().iter().any(|r| r.kind == "note"), "the interrupted note stays");
        assert_eq!(t.head.team_turns, 1);
    }

    #[test]
    fn a_held_edit_shows_team_mode_was_on_and_keeps_the_roles() {
        let mut t = chat();
        say(&mut t, "/swb team", 10);
        reply(&mut t, OFF_LINE, 0, 11);
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "e", "name": "Edit", "input": {"file_path": "/p", "old_string": "a", "new_string": "b"}}]}}), 20);
        t.apply("claude", &json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "e", "is_error": true, "content": "SmartWorkbench team mode: this edit is not small"}]}}), 21);
        let seen = t.head.team.clone().expect("team");
        assert!(seen.line.on && seen.source == Seen::Guard && seen.ts == 21 && seen.line.roles.len() == 3);
    }

    #[test]
    fn a_command_does_not_name_the_chat_and_no_plugin_means_unknown() {
        let mut t = chat();
        t.apply("claude", &json!({"type": "system", "subtype": "init", "agents": ["general-purpose"]}), 1);
        assert_eq!(t.head.swb, Some(false));
        say(&mut t, "/swb team", 10);
        assert!(t.first_text.is_none());
        say(&mut t, "hello", 20);
        assert_eq!(t.first_text.as_deref(), Some("hello"));
    }

    #[test]
    fn streaming_text_is_replaced_by_the_final_answer_and_hidden_thinking_shows_no_data() {
        let mut t = chat();
        t.apply("claude", &json!({"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "Hel"}}}), 0);
        t.apply("claude", &json!({"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "lo"}}}), 0);
        let rows = t.rows();
        assert_eq!(rows.last().unwrap().md, "Hello");
        assert_eq!(rows.last().unwrap().state, "live");
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "redacted_thinking", "data": "SECRETDATA"}, {"type": "text", "text": "Hello!"}]}}), 0);
        let rows = t.rows();
        assert!(rows.iter().all(|r| !format!("{r:?}").contains("SECRETDATA")));
        assert_eq!(rows.iter().filter(|r| r.kind == "para").count(), 1, "the live text is gone");
        assert_eq!(rows.iter().find(|r| r.kind == "think").unwrap().text, "생각 (가려짐)");
    }

    #[test]
    fn live_thinking_after_live_text_is_removed_without_touching_other_rows() {
        let mut t = chat();
        t.apply("user", &json!({"text": "q"}), 0);
        t.apply("claude", &json!({"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "partial"}}}), 0);
        t.apply("claude", &json!({"type": "stream_event", "event": {"type": "content_block_start", "content_block": {"type": "thinking"}}}), 0);
        t.apply("claude", &json!({"type": "stream_event", "event": {"type": "content_block_stop"}}), 0);
        t.apply("claude", &json!({"type": "stream_event", "event": {"type": "content_block_stop"}}), 0);
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "final"}]}}), 0);
        let kinds: Vec<&str> = t.rows().iter().map(|r| r.kind).collect();
        assert_eq!(kinds, vec!["meta-user", "user", "meta-asst", "para"], "the user's message stays and only the live rows go");
        assert_eq!(t.rows().last().unwrap().md, "final");
    }

    #[test]
    fn permissions_wait_until_resolved_and_a_result_sets_the_header() {
        let mut t = chat();
        t.apply("permission", &json!({"request_id": "p1", "tool_name": "Bash", "input": {}}), 0);
        t.apply("permission", &json!({"request_id": "p1", "tool_name": "Bash", "input": {}}), 0);
        assert_eq!(t.pending.len(), 1);
        t.apply("permission_resolved", &json!({"request_id": "p1", "outcome": "denied"}), 0);
        assert!(t.pending.is_empty());
        t.apply("claude", &json!({"type": "result", "duration_ms": 2400, "total_cost_usd": 0.0123, "usage": {"input_tokens": 1000, "output_tokens": 500}}), 0);
        assert_eq!(t.head.turn, Some((Some("$0.012".into()), Some("1.5k".into()))));
        assert!(t.rows().iter().any(|r| r.kind == "note" && r.text == "2.4s · $0.0123"));
        assert_eq!(model_label("claude-sonnet-4-5-20250929"), "sonnet-4-5");
    }

    #[test]
    fn the_context_size_is_the_last_calls_and_the_window_comes_from_the_result() {
        let mut t = chat();
        t.apply("claude", &json!({"type": "assistant", "message": {"usage": {"input_tokens": 10, "cache_read_input_tokens": 1000, "output_tokens": 5}, "content": []}}), 0);
        t.apply("claude", &json!({"type": "assistant", "message": {"usage": {"input_tokens": 20, "cache_read_input_tokens": 2000, "cache_creation_input_tokens": 100, "output_tokens": 30}, "content": []}}), 0);
        // A sub-agent's call is not the chat's own context; a call without usage leaves the number alone.
        t.apply("claude", &json!({"type": "assistant", "parent_tool_use_id": "a", "message": {"usage": {"input_tokens": 99999}, "content": []}}), 0);
        t.apply("claude", &json!({"type": "assistant", "message": {"content": []}}), 0);
        assert_eq!(t.head.ctx_used, Some(2150));
        assert_eq!(context_line(t.head.ctx_used, t.head.ctx_max), None, "no window yet: the old count stays");
        t.apply("claude", &json!({"type": "result", "total_cost_usd": 0.1, "usage": {"input_tokens": 5}, "modelUsage": {"claude-haiku": {"contextWindow": 100000}, "claude-opus": {"contextWindow": 200000}}}), 0);
        assert_eq!(t.head.ctx_max, Some(200000), "no model known: the largest window");
        assert_eq!(context_line(t.head.ctx_used, t.head.ctx_max).as_deref(), Some("2.1k / 200k"));
        t.head.model = Some("claude-haiku".into());
        t.apply("claude", &json!({"type": "result", "total_cost_usd": 0.1, "modelUsage": {"claude-haiku": {"contextWindow": 100000}, "claude-opus": {"contextWindow": 200000}}}), 0);
        assert_eq!(t.head.ctx_max, Some(100000), "the chat's own model wins");
    }

    #[test]
    fn counts_are_written_short() {
        assert_eq!(short_count(999), "999");
        assert_eq!(short_count(1000), "1k");
        assert_eq!(short_count(45_200), "45k");
        assert_eq!(short_count(1_000_000), "1M");
        assert_eq!(context_line(Some(0), Some(0)).as_deref(), Some("0 / 0"));
        assert_eq!(context_line(None, Some(5)), None);
    }

    #[test]
    fn a_todo_list_shows_as_a_checklist_with_its_progress() {
        let mut t = chat();
        let todos = json!([{"content": "design", "status": "completed"}, {"content": "write\u{1b}[31m code", "status": "in_progress"}, {"content": "test", "status": "pending"}]);
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "td", "name": "TodoWrite", "input": {"todos": todos}}]}}), 0);
        t.toggle("td");
        let rows = t.rows();
        let row = rows.iter().find(|r| r.label == "TodoWrite").expect("todo card");
        assert_eq!(row.text, "1/3 todos");
        assert_eq!(row.detail, "[x] design\n[~] write[31m code\n[ ] test");
    }
}
