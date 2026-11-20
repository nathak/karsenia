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
use crate::watch::{Alert, Watch};

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

/// One line of an edit's diff as it is drawn: `kind` is ' ' (context), '+' (added), '-' (removed), '@' (a gap between two hunks) or
/// '…' (lines left out; `no` is how many). `no` is the line number (the new file's for context and added lines, the old file's for removed
/// ones), 0 when it is not known.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffLine {
    pub kind: char,
    pub no: u32,
    pub text: String,
}

/// Most diff lines drawn for one tool call.
const MAX_DIFF_ROWS: usize = 120;

/// One row of the transcript list.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    /// day, meta-user, user, meta-asst, para, heading, item, quote, code, rule, table, think, tool, steps (a long run of tools and
    /// thoughts folded into one row), note, note-err, note-sys, note-warn
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
    /// A table's cells by row (the first row is the header).
    pub grid: Vec<Vec<String>>,
    /// The diff of an opened edit card, drawn line by line (cheap to clone: the rows are rebuilt at every event).
    pub diff: Arc<[DiffLine]>,
    /// The first lines of a command's output, shown under the header while the card is closed.
    pub preview: String,
}

/// The row of a block as it is drawn when nothing folds (a code block is shown whole).
fn flat_row(block: &Block, key: String, depth: i32) -> Row {
    let row = |kind: &'static str| Row { kind, depth, key: key.clone(), ..Row::default() };
    match block {
        Block::Para(md) => Row { md: md.clone(), ..row("para") },
        Block::Heading(level, md) => Row { md: format!("**{md}**"), extra: level.to_string(), ..row("heading") },
        Block::Item { depth: d, marker, md } => Row { md: md.clone(), label: marker.clone(), indent: i32::from(*d), ..row("item") },
        Block::Quote(md) => Row { md: md.clone(), ..row("quote") },
        Block::Code { lang, text } => Row { text: text.clone(), label: if lang.is_empty() { "text".into() } else { lang.to_lowercase() }, ..row("code") },
        Block::Rule => row("rule"),
        Block::Table { text, grid } => Row { text: text.clone(), grid: grid.clone(), ..row("table") },
    }
}

/// The last `max` characters of a text, starting at a line (the first whole line after the cut), and how many characters were left out.
pub fn tail_chars(text: &str, max: usize) -> (&str, usize) {
    let total = text.chars().count();
    if total <= max {
        return (text, 0);
    }
    let cut = text.char_indices().nth(total - max).map_or(text.len(), |(at, _)| at);
    let start = text[cut..].find('\n').map_or(cut, |n| cut + n + 1);
    (&text[start..], text[..start].chars().count())
}

/// A leading front matter block (`---` … `---`, as markdown files of notes and sites have) and the text after it.
fn split_front_matter(src: &str) -> (Option<&str>, &str) {
    let mut lines = src.split_inclusive('\n');
    let first = lines.next().unwrap_or_default();
    if first.trim_end() != "---" {
        return (None, src);
    }
    let mut at = first.len();
    for (n, line) in lines.enumerate() {
        if n >= 100 {
            break;
        }
        if matches!(line.trim_end(), "---" | "...") {
            return (Some(src[first.len()..at].trim_end()), &src[at + line.len()..]);
        }
        at += line.len();
    }
    (None, src)
}

/// What the file viewer draws for a markdown file: the chat's own rows (so a table, a list or a quote look as they do in an
/// answer), a code block whole, a front matter block as a code block, no folding. Images and HTML are not drawn (as in a chat).
pub fn md_rows(src: &str, lang: Lang) -> Vec<Row> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let (front, body) = split_front_matter(src);
    let lines = |text: &str| fill(&tr(lang, "lines"), &[("n", text.lines().count().max(1).to_string())]);
    let mut rows = Vec::new();
    if let Some(front) = front {
        rows.push(Row { kind: "code", key: "m-front".into(), text: front.to_owned(), label: "yaml".into(), extra: lines(front), ..Row::default() });
    }
    for (b, block) in blocks(body).iter().enumerate() {
        let mut row = flat_row(block, format!("m{b}"), 0);
        if row.kind == "code" {
            row.extra = lines(&row.text);
        }
        rows.push(row);
    }
    if rows.is_empty() && !body.trim().is_empty() {
        rows.push(Row { kind: "para", key: "m0".into(), md: super::markdown::escape(body), ..Row::default() });
    }
    rows
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
    /// The diff from the tool's structured result (with context and line numbers), when Claude Code sent one.
    patch: Vec<DiffLine>,
    /// The person opened or closed the card: it is then no longer opened by itself when the result arrives.
    touched: bool,
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
    /// The share of the last result's calls that the prompt cache served (whole percent).
    pub cache_pct: Option<u32>,
    /// Whether the last `system/init` listed the SmartWorkbench plug-in's agents (None before any init).
    pub swb: Option<bool>,
    /// The last team-mode state the chat showed (see `view::team`).
    pub team: Option<TeamSeen>,
    /// How many `/swb` command turns have ended: a button waits for this to change.
    pub team_turns: u32,
    /// What the last `/swb status` answered (the plug-in's text summary of the contract) and when: shown when no snapshot exists.
    pub swb_status: Option<(String, u64)>,
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
    cmd_status: bool,
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
    /// The highest context level already said in the transcript since the chat was last below it (so each is said once).
    ctx_warned: Pressure,
    /// Signs that the chat has lost its way (the same call again and again, a wide spread of edits), from its own tool calls.
    watch: Watch,
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

/// A run of this many tools (with the thoughts between them) is folded into one row.
const MIN_STEPS: usize = 3;

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

/// The lines of an edit from Claude Code's structured result: its hunks (context, removed, added, with the line numbers they start at),
/// or for a file that was created its content as added lines. The counts are of everything kept; at most `MAX_DIFF_ROWS` lines are
/// drawn, the rest is one marker line. None when the result has neither (the diff then comes from the call's input).
fn patch_lines(result: &Value) -> Option<(Vec<DiffLine>, i32, i32)> {
    let mut lines: Vec<DiffLine> = Vec::new();
    let (mut add, mut del) = (0, 0);
    let hunks = result.get("structuredPatch").and_then(Value::as_array).filter(|h| !h.is_empty());
    if let Some(hunks) = hunks {
        for (n, hunk) in hunks.iter().enumerate() {
            if n > 0 {
                lines.push(DiffLine { kind: '@', no: 0, text: String::new() });
            }
            let start = |key: &str| hunk.get(key).and_then(Value::as_u64).unwrap_or(1).min(u32::MAX as u64) as u32;
            let (mut old_no, mut new_no) = (start("oldStart"), start("newStart"));
            for line in hunk.get("lines").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                let mut chars = line.chars();
                match chars.next() {
                    Some('+') => {
                        add += 1;
                        lines.push(DiffLine { kind: '+', no: new_no, text: chars.as_str().to_owned() });
                        new_no += 1;
                    }
                    Some('-') => {
                        del += 1;
                        lines.push(DiffLine { kind: '-', no: old_no, text: chars.as_str().to_owned() });
                        old_no += 1;
                    }
                    // "\ No newline at end of file" is a note about the line before it, not a line.
                    Some('\\') => {}
                    Some(_) => {
                        lines.push(DiffLine { kind: ' ', no: new_no, text: chars.as_str().to_owned() });
                        old_no += 1;
                        new_no += 1;
                    }
                    None => {
                        lines.push(DiffLine { kind: ' ', no: new_no, text: String::new() });
                        old_no += 1;
                        new_no += 1;
                    }
                }
            }
        }
    } else if s(result, "type") == "create" {
        for (i, line) in s(result, "content").lines().enumerate() {
            add += 1;
            lines.push(DiffLine { kind: '+', no: i as u32 + 1, text: line.to_owned() });
        }
        if lines.is_empty() {
            return None;
        }
    } else {
        return None;
    }
    if lines.len() > MAX_DIFF_ROWS {
        let hidden = lines.len() - MAX_DIFF_ROWS;
        lines.truncate(MAX_DIFF_ROWS);
        lines.push(DiffLine { kind: '…', no: hidden as u32, text: String::new() });
    }
    Some((lines, add, del))
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
    } else if head.contains("The user declined this") && head.contains("in SmartWorkbench") {
        // The guard asked, and in a chat with no terminal to ask in its question ends as a "decline": nobody here refused it.
        Some("swb_ask")
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

/// How full the context window is. The levels are where it is worth saying something: from `High` a manual `/compact` between two
/// phases of the work is better than the automatic one at an arbitrary point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Pressure {
    #[default]
    Ok,
    Warn,
    High,
    Crit,
}

impl Pressure {
    /// 0..=3, for the header's colour.
    pub fn level(self) -> i32 {
        self as i32
    }
}

/// The share of the window the chat takes (whole percent, at most 999) and its level: 70% and more warns, 80% is high, 90% is critical.
/// Only a window a result named gives a share: a guessed window would make the number a guess too.
pub fn context_pressure(used: u64, max: u64) -> (u32, Pressure) {
    if max == 0 {
        return (0, Pressure::Ok);
    }
    let pct = (used.saturating_mul(100) / max).min(999) as u32;
    let level = match pct {
        90.. => Pressure::Crit,
        80.. => Pressure::High,
        70.. => Pressure::Warn,
        _ => Pressure::Ok,
    };
    (pct, level)
}

/// The share of what a result's calls were given that came from the prompt cache, in whole percent (None when nothing was given).
/// A high share is cheap tokens; a low one in a long chat means the start of the prompt keeps changing.
pub fn cache_hit(usage: &Value) -> Option<u32> {
    let n = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
    let (fresh, read, written) = (n("input_tokens"), n("cache_read_input_tokens"), n("cache_creation_input_tokens"));
    let all = fresh.saturating_add(read).saturating_add(written);
    (all > 0).then(|| (read.saturating_mul(100) / all) as u32)
}

/// "45k / 200k · 22%" when the window is known, else None (the header then shows the old per-turn count).
pub fn context_line(used: Option<u64>, max: Option<u64>) -> Option<String> {
    let (used, max) = (used?, max?);
    let (pct, _) = context_pressure(used, max);
    Some(format!("{} / {} · {pct}%", short_count(used), short_count(max)))
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
        Self { lang, nodes: Vec::new(), tools: HashMap::new(), live_text: None, live_think: None, think_ms: None, asst_meta: false, day: String::new(), ts: 0, team_cmd: false, cmd_status: false, team_cand: None, head: Head::default(), pending: Vec::new(), replaying: true, first_text: None, expanded: Default::default(), ctx_warned: Pressure::Ok, watch: Watch::new() }
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

    /// Says once, in the transcript, that the window is filling up (it is said again only after the chat has dropped below the level,
    /// for example after a compaction). The same results give the same notes on a replay as when following.
    fn context_notice(&mut self) {
        let (Some(used), Some(max)) = (self.head.ctx_used, self.head.ctx_max) else { return };
        let (_, level) = context_pressure(used, max);
        if level < Pressure::High {
            self.ctx_warned = level;
            return;
        }
        if level > self.ctx_warned {
            self.ctx_warned = level;
            let key = if level == Pressure::Crit { "ctx_crit" } else { "ctx_high" };
            let text = self.t(key);
            self.note(text, "note-warn");
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
                self.cmd_status = text.trim() == "/swb status";
                self.team_cand = None;
                if self.first_text.is_none() && !text.is_empty() && !crate::session::is_slash(&text) {
                    self.first_text = Some(text.chars().take(60).collect());
                }
                let list: Vec<crate::claude::protocol::Image> = data
                    .get("images")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|i| Some(crate::claude::protocol::Image { media_type: i["media_type"].as_str()?.to_owned(), data: i["data"].as_str().filter(|d| !d.is_empty())?.to_owned(), name: None })).collect())
                    .unwrap_or_default();
                // What cannot be drawn as a thumbnail is still told in the text.
                let plain = data.get("images").and_then(Value::as_array).map_or(0, Vec::len) - list.iter().filter(|i| THUMB_TYPES.contains(&i.media_type.as_str())).count();
                if plain > 0 {
                    text = format!("{}\n{}", fill(&self.t("images"), &[("n", plain.to_string())]), text).trim().to_owned();
                }
                // The other files of the message (name, size and kind only: their content is not in the log) are told above the words.
                let lines: Vec<String> = data
                    .get("files")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|f| {
                        let name: String = f["name"].as_str().unwrap_or("").chars().filter(|c| !c.is_control()).take(100).collect();
                        fill(&self.t("file_line"), &[("name", name), ("size", crate::view::attach::human_size(f["size"].as_u64().unwrap_or(0)))])
                    })
                    .collect();
                if !lines.is_empty() {
                    text = format!("{}\n{}", lines.join("\n"), text).trim().to_owned();
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
            // The chat's gateway was switched on (a provider that uses an account's login) or off, between turns.
            "provider" => {
                let text = match data["provider"]["name"].as_str() {
                    Some(name) => fill(&self.t("gw_on_note"), &[("name", name.chars().take(60).collect())]),
                    None => self.t("gw_off_note"),
                };
                self.note(text, "note");
            }
            // The chat's team settings changed (the helper subagents and efforts of the app's own team mode).
            "team" => {
                let summary: String = s(data, "summary").chars().filter(|c| !c.is_control()).take(160).collect();
                self.note(format!("{}: {summary}", self.t("team_changed_note")), "note");
            }
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
                // Claude Code's structured form of the result (the patch of an edit, the numbers of a read) rides on the event itself.
                let structured = d.get("tool_use_result").or_else(|| d.get("toolUseResult"));
                for block in d.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                    if s(block, "type") == "tool_result" {
                        self.tool_result(block, structured);
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
                    if zero && self.cmd_status {
                        let text: String = s(d, "result").chars().filter(|c| !c.is_control() || *c == '\n').take(4000).collect();
                        if !text.trim().is_empty() {
                            self.head.swb_status = Some((text, self.ts));
                        }
                    }
                    self.cmd_status = false;
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
                self.head.cache_pct = d.get("usage").and_then(cache_hit);
                self.context_notice();
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
            // The CLI is trying the endpoint again after an error: said in the transcript, as the person would otherwise see nothing happen.
            "system" if s(d, "subtype") == "api_retry" => {
                let num = |key: &str| d.get(key).and_then(Value::as_u64);
                let mut text = fill(&self.t("api_retry"), &[("n", num("attempt").map_or("?".into(), |n| n.to_string())), ("max", num("max_retries").map_or("?".into(), |n| n.to_string()))]);
                let status = num("error_status").map(|n| n.to_string());
                let error = d.get("error").and_then(Value::as_str).map(|e| e.chars().take(80).collect::<String>());
                let why: Vec<String> = status.into_iter().chain(error).collect();
                if !why.is_empty() {
                    text.push_str(&format!(" ({})", why.join(" · ")));
                }
                if let Some(ms) = num("retry_delay_ms") {
                    text.push_str(&fill(&self.t("api_retry_in"), &[("s", format!("{:.1}", ms as f64 / 1000.0))]));
                }
                self.note(text, "note-warn");
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
        let alerts = self.watch.call(&id, &name, &input, parent.is_some());
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
            patch: Vec::new(),
            touched: false,
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
                fresh.touched = old.touched;
                fresh.patch = old.patch.clone();
                fresh.started = old.started;
                self.nodes[i].item = Item::Tool(fresh);
            }
            return;
        }
        let i = self.push(Item::Tool(tool), parent);
        self.tools.insert(id, i);
        for alert in alerts {
            let text = match alert {
                Alert::Loop { tool, times } => fill(&self.t("watch_loop"), &[("n", times.to_string()), ("tool", tool)]),
                Alert::Scope { files } => fill(&self.t("watch_scope"), &[("n", files.to_string())]),
            };
            self.note(text, "note-warn");
        }
    }

    fn tool_result(&mut self, block: &Value, structured: Option<&Value>) {
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
            Some("swb_ask") => ("guard", tr(lang, "guard_swb_ask")),
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
            if let Some(res) = structured.filter(|_| !is_error) {
                // The real diff of an edit (context and line numbers), shown at once as Claude Code shows it, unless the person has
                // opened or closed the card already.
                if let Some((lines, add, del)) = patch_lines(res) {
                    t.patch = lines;
                    t.add = add;
                    t.del = del;
                    if !t.touched {
                        t.open = true;
                    }
                }
                // A read says how many lines it gave.
                if t.name == "Read" {
                    if let Some(n) = res.pointer("/file/numLines").and_then(Value::as_u64) {
                        let count = tr(lang, "lines").replace("{n}", &n.to_string());
                        t.extra = if t.extra.is_empty() { count } else { format!("{} · {count}", t.extra) };
                    }
                }
            }
        }
    }

    /// Opens or closes a tool card, a thinking block, or a long code block.
    pub fn toggle(&mut self, key: &str) {
        // A folded run of steps (`g:<first node>`): its own kind of key, which no tool id (`toolu_…`) or `n…` key can be.
        if let Some(n) = key.strip_prefix("g:") {
            if n.parse::<usize>().is_ok() && !self.expanded.remove(key) {
                self.expanded.insert(key.to_owned());
            }
            return;
        }
        if key.contains('-') && key.starts_with('n') {
            if !self.expanded.remove(key) {
                self.expanded.insert(key.to_owned());
            }
            return;
        }
        if let Some(&i) = self.tools.get(key) {
            if let Item::Tool(t) = &mut self.nodes[i].item {
                t.open = !t.open;
                t.touched = true;
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
        let mut k = 0;
        while k < top.len() {
            // A run of tools and finished thoughts with nothing else between them.
            let mut end = k;
            while end < top.len() && self.groupable(top[end]) {
                end += 1;
            }
            let run = &top[k..end];
            if run.iter().filter(|&&i| matches!(self.nodes[i].item, Item::Tool(_))).count() >= MIN_STEPS {
                self.emit_steps(run, &children, &mut out);
                k = end;
                continue;
            }
            // Too short to fold (or not a run at all): as they are.
            let upto = if run.is_empty() { k + 1 } else { end };
            for &i in &top[k..upto] {
                let depth = i32::from(self.nodes[i].parent.is_some());
                self.emit(i, depth, &children, &mut out);
            }
            k = upto;
        }
        out
    }

    /// The conversation as text to copy: the words as they were written (a message's markdown as it is, a code block whole), one line for
    /// each tool call, no thinking, no sub-agent's own calls (its card has a line), and the same whatever is open or folded on screen.
    pub fn plain_text(&self) -> String {
        self.plain_range(0..self.nodes.len(), true)
    }

    /// One message as text: the user's or the answer's that `key` (the key of its header row, `n<index>`) belongs to, up to the next one.
    pub fn message_text(&self, key: &str) -> Option<String> {
        let at: usize = key.strip_prefix('n')?.parse().ok()?;
        if !matches!(self.nodes.get(at)?.item, Item::UserMeta(_) | Item::AsstMeta { .. }) {
            return None;
        }
        let end = (at + 1..self.nodes.len()).find(|&i| self.nodes[i].parent.is_none() && matches!(self.nodes[i].item, Item::UserMeta(_) | Item::AsstMeta { .. } | Item::Day(_))).unwrap_or(self.nodes.len());
        let text = self.plain_range(at + 1..end, false);
        (!text.is_empty()).then_some(text)
    }

    fn plain_range(&self, range: std::ops::Range<usize>, headers: bool) -> String {
        let mut out = String::new();
        let mut prev_tool = false;
        for i in range {
            let node = &self.nodes[i];
            if node.parent.as_deref().is_some_and(|p| self.tools.contains_key(p)) {
                continue;
            }
            let (block, tool) = match &node.item {
                Item::Day(text) if headers => (format!("── {text} ──"), false),
                Item::UserMeta(time) if headers => (format!("── {} · {time} ──", self.t("you")), false),
                Item::AsstMeta { who, time } if headers => (format!("── {who} · {time} ──"), false),
                Item::User { text, images } => {
                    let n = images.as_ref().map_or(0, |s| s.list.len());
                    let note = if n > 0 { fill(&self.t("plain_images"), &[("n", n.to_string())]) } else { String::new() };
                    (format!("{}\n{}", text.trim_end(), note).trim().to_owned(), false)
                }
                Item::Text { src, .. } => (src.trim().to_owned(), false),
                Item::Tool(tool) => {
                    let short = tool.name.rsplit("__").next().unwrap_or(&tool.name);
                    (format!("[{short}] {}", tool.head.split_whitespace().collect::<Vec<_>>().join(" ")).trim_end().to_owned(), true)
                }
                Item::Note { text, .. } => (format!("[{}]", text.trim()), false),
                _ => continue,
            };
            if block.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push_str(if prev_tool && tool { "\n" } else { "\n\n" });
            }
            out.push_str(&block);
            prev_tool = tool;
        }
        out
    }

    /// A node that may be folded into a run of steps: a tool, or a thought that is finished (a thought being streamed stays on its own so
    /// its text can be read as it comes), at the top level (a sub-agent's work stays under its card).
    fn groupable(&self, i: usize) -> bool {
        // A file edit stays a card of its own: its diff is what the person came to see.
        self.nodes[i].parent.is_none()
            && match &self.nodes[i].item {
                Item::Tool(t) => !matches!(t.name.as_str(), "Edit" | "MultiEdit" | "Write" | "NotebookEdit"),
                Item::Think { live, .. } => !*live,
                _ => false,
            }
    }

    /// One row for a run of steps: how many, of what, and whether any is running, failed or held; the steps themselves follow it when it
    /// is opened (a thought without text carries nothing and is left out: it is only counted).
    fn emit_steps(&self, run: &[usize], children: &HashMap<&str, Vec<usize>>, out: &mut Vec<Row>) {
        let key = format!("g:{}", run[0]);
        let open = self.expanded.contains(&key);
        let mut names: Vec<(String, usize)> = Vec::new();
        let (mut tools, mut thinks, mut running, mut errs, mut guards) = (0, 0, 0, 0, 0);
        // The tool that is running (the last of them, when several run side by side).
        let mut latest: Option<(&Tool, String)> = None;
        for &i in run {
            match &self.nodes[i].item {
                Item::Tool(t) => {
                    tools += 1;
                    let short = t.name.rsplit("__").next().unwrap_or(&t.name).to_owned();
                    match names.iter_mut().find(|(n, _)| *n == short) {
                        Some((_, c)) => *c += 1,
                        None => names.push((short.clone(), 1)),
                    }
                    match t.state {
                        "running" => {
                            running += 1;
                            latest = Some((t, short));
                        }
                        "err" => errs += 1,
                        "guard" => guards += 1,
                        _ => {}
                    }
                }
                _ => thinks += 1,
            }
        }
        // The most used names first; equal counts keep the order they came in.
        names.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        let mut text = names.iter().take(3).map(|(n, c)| format!("{n} {c}")).collect::<Vec<_>>().join(" · ");
        if names.len() > 3 {
            text.push_str(" · …");
        }
        if thinks > 0 {
            text.push_str(&format!(" · {}", fill(&self.t("steps_think"), &[("n", thinks.to_string())])));
        }
        let mut status = Vec::new();
        if running > 0 {
            status.push(self.t("running_tool"));
        }
        if errs > 0 {
            status.push(fill(&self.t("steps_err"), &[("n", errs.to_string())]));
        }
        if guards > 0 {
            status.push(fill(&self.t("steps_guard"), &[("n", guards.to_string())]));
        }
        let state = if running > 0 { "running" } else if errs > 0 { "err" } else if guards > 0 { "guard" } else { "done" };
        let status = if status.is_empty() { self.t("done") } else { status.join(" · ") };
        // A run in progress shows its latest step, so that folding it does not hide what is going on.
        let detail = match latest {
            Some((t, short)) if !open => first_line(&format!("{short}: {}", t.head)),
            _ => String::new(),
        };
        out.push(Row { kind: "steps", key, label: fill(&self.t("steps_title"), &[("n", tools.to_string())]), text, status, state, detail, open, ..Row::default() });
        if open {
            for &i in run {
                if matches!(&self.nodes[i].item, Item::Think { text, .. } if text.is_empty()) {
                    continue;
                }
                self.emit(i, 1, children, out);
            }
        }
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
                        Block::Code { lang, text } => {
                            // A block of more than 30 lines shows its first 12 until it is opened.
                            let n = text.lines().count().max(1);
                            let foldable = n > 30;
                            let open = self.expanded.contains(&key);
                            let shown = if foldable && !open { text.lines().take(12).collect::<Vec<_>>().join("\n") } else { text.clone() };
                            Row { text: shown, label: if lang.is_empty() { "text".into() } else { lang.to_lowercase() }, extra: fill(&self.t("lines"), &[("n", n.to_string())]), detail: if foldable { fill(&self.t(if open { "fold" } else { "show_all" }), &[("n", n.to_string())]) } else { String::new() }, open, add: i32::from(foldable), key, ..row("code") }
                        }
                        other => flat_row(other, key, depth),
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
                let mut diff: Vec<DiffLine> = Vec::new();
                if t.open {
                    // An edit is drawn as its diff: the real one when Claude Code sent it, else the one made of the call's own input.
                    if !t.patch.is_empty() {
                        diff = t.patch.clone();
                    } else if !t.diff.is_empty() && t.checklist.is_empty() {
                        diff = t.diff.iter().map(|(m, l)| DiffLine { kind: *m, no: 0, text: l.clone() }).collect();
                    }
                    for line in diff.iter_mut().filter(|l| l.kind == '…') {
                        line.text = fill(&self.t("diff_more"), &[("n", line.no.to_string())]);
                    }
                    if !t.checklist.is_empty() {
                        detail.push_str(&t.checklist);
                    } else if diff.is_empty() {
                        detail.push_str(&format!("── {} ──\n{}", self.t("input"), t.input));
                    }
                    // What the tool answered: shown under a command or a read, and under an edit only when it failed or was held (its
                    // "updated successfully" says nothing the diff does not).
                    if !t.result.is_empty() && (diff.is_empty() || t.state == "err" || t.state == "guard") {
                        if detail.is_empty() {
                            detail.push_str(&t.result);
                        } else {
                            detail.push_str(&format!("\n\n── {} ──\n{}", self.t("result"), t.result));
                        }
                    }
                }
                // The first lines of a command's answer, under its header while the card is closed (as the CLI prints them).
                let preview = if !t.open && t.name == "Bash" && !t.result.trim().is_empty() {
                    let lines: Vec<&str> = t.result.lines().collect();
                    let mut text = lines.iter().take(3).map(|l| l.chars().take(200).collect::<String>()).collect::<Vec<_>>().join("\n");
                    if lines.len() > 3 {
                        text.push('\n');
                        text.push_str(&fill(&self.t("preview_more"), &[("n", (lines.len() - 3).to_string())]));
                    }
                    text
                } else {
                    String::new()
                };
                let extra = match kids {
                    Some(k) if !k.is_empty() => {
                        let n = k.iter().filter(|&&c| matches!(self.nodes[c].item, Item::Tool(_))).count();
                        if n > 0 { fill(&self.t("sub_tools"), &[("n", n.to_string())]) } else { t.extra.clone() }
                    }
                    _ => t.extra.clone(),
                };
                out.push(Row { key: t.id.clone(), label: t.name.clone(), text: t.head.clone(), extra, badge: t.badge.clone(), status: t.status.clone(), state: t.state, detail, open: t.open, add: t.add, del: t.del, diff: diff.into(), preview, ..row("tool") });
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
    fn the_answer_of_swb_status_is_kept_and_only_for_that_command() {
        let mut t = chat();
        t.apply("user", &json!({"text": "/swb status"}), 1000);
        t.apply("claude", &json!({"type": "result", "subtype": "success", "num_turns": 0, "is_error": false, "result": "smartworkbench: WB ○ No goal set\nGoal: — (unlocked, draft)"}), 2000);
        let (text, at) = t.head.swb_status.clone().expect("kept");
        assert!(text.contains("Goal: —") && text.contains('\n'));
        assert_eq!(at, 2000);
        // Another command, or a turn a model answered, does not replace it.
        t.apply("user", &json!({"text": "/swb lock"}), 3000);
        t.apply("claude", &json!({"type": "result", "subtype": "success", "num_turns": 0, "is_error": false, "result": "smartworkbench: locked"}), 4000);
        t.apply("user", &json!({"text": "/swb status"}), 5000);
        t.apply("claude", &json!({"type": "result", "subtype": "success", "num_turns": 2, "is_error": false, "result": "a sentence a model wrote"}), 6000);
        assert_eq!(t.head.swb_status.as_ref().map(|s| s.1), Some(2000));
    }

    #[test]
    fn a_guard_question_that_had_nobody_to_ask_is_told_from_a_refusal() {
        let msg = "<tool_use_error>The user declined this Bash call in SmartWorkbench (may print secrets).</tool_use_error>";
        assert_eq!(guard_kind(msg), Some("swb_ask"));
        assert_eq!(guard_kind("The user declined this call"), None);
        assert_eq!(guard_kind("SmartWorkbench team mode: not small"), Some("team"));
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
        assert_eq!(context_line(t.head.ctx_used, t.head.ctx_max).as_deref(), Some("2.1k / 200k · 1%"));
        t.head.model = Some("claude-haiku".into());
        t.apply("claude", &json!({"type": "result", "total_cost_usd": 0.1, "modelUsage": {"claude-haiku": {"contextWindow": 100000}, "claude-opus": {"contextWindow": 200000}}}), 0);
        assert_eq!(t.head.ctx_max, Some(100000), "the chat's own model wins");
    }

    #[test]
    fn the_context_share_has_levels_and_never_divides_by_zero() {
        assert_eq!(context_pressure(0, 200_000), (0, Pressure::Ok));
        assert_eq!(context_pressure(139_999, 200_000), (69, Pressure::Ok));
        assert_eq!(context_pressure(140_000, 200_000), (70, Pressure::Warn));
        assert_eq!(context_pressure(160_000, 200_000), (80, Pressure::High));
        assert_eq!(context_pressure(180_000, 200_000), (90, Pressure::Crit));
        assert_eq!(context_pressure(5_000_000, 200_000), (999, Pressure::Crit), "the number stays short");
        assert_eq!(context_pressure(10, 0), (0, Pressure::Ok));
        assert_eq!(Pressure::Crit.level(), 3);
    }

    #[test]
    fn the_cache_share_counts_what_the_prompt_cache_served() {
        assert_eq!(cache_hit(&json!({"input_tokens": 100, "cache_read_input_tokens": 800, "cache_creation_input_tokens": 100, "output_tokens": 50})), Some(80));
        assert_eq!(cache_hit(&json!({"input_tokens": 10})), Some(0));
        assert_eq!(cache_hit(&json!({"output_tokens": 50})), None, "nothing was given to the model");
        assert_eq!(cache_hit(&json!({})), None);
        let mut t = chat();
        t.apply("claude", &json!({"type": "result", "usage": {"input_tokens": 50, "cache_read_input_tokens": 50}}), 0);
        assert_eq!(t.head.cache_pct, Some(50));
        t.apply("claude", &json!({"type": "result"}), 0);
        assert_eq!(t.head.cache_pct, None, "a result without usage clears it");
    }

    fn warnings(t: &Transcript) -> usize {
        t.rows().iter().filter(|r| r.kind == "note-warn").count()
    }

    #[test]
    fn a_full_window_is_said_once_per_level_and_again_after_it_dropped() {
        let used = |t: &mut Transcript, n: u64| {
            t.apply("claude", &json!({"type": "assistant", "message": {"usage": {"input_tokens": n}, "content": []}}), 0);
            t.apply("claude", &json!({"type": "result", "modelUsage": {"m": {"contextWindow": 200000}}}), 0);
        };
        let mut t = chat();
        used(&mut t, 150_000);
        assert_eq!(warnings(&t), 0, "70% only colours the header");
        used(&mut t, 165_000);
        assert_eq!(warnings(&t), 1);
        used(&mut t, 170_000);
        assert_eq!(warnings(&t), 1, "the same level is not said twice");
        used(&mut t, 185_000);
        assert_eq!(warnings(&t), 2, "a higher level is");
        used(&mut t, 20_000);
        used(&mut t, 170_000);
        assert_eq!(warnings(&t), 3, "after a compaction it says it again");
        // Before any result named the window nothing can be said.
        let mut early = chat();
        early.apply("claude", &json!({"type": "assistant", "message": {"usage": {"input_tokens": 199_000}, "content": []}}), 0);
        assert_eq!(warnings(&early), 0);
    }

    #[test]
    fn the_same_call_again_and_again_is_said_once_in_the_transcript_and_a_card_sent_twice_counts_once() {
        let call = |t: &mut Transcript, id: &str| {
            t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": "cargo test"}}]}}), 0);
        };
        let mut t = chat();
        for n in 0..4 {
            call(&mut t, &format!("c{n}"));
            call(&mut t, &format!("c{n}"));
        }
        assert_eq!(warnings(&t), 0, "four different calls, each sent twice");
        call(&mut t, "c4");
        assert_eq!(warnings(&t), 1);
        let text = t.rows().iter().find(|r| r.kind == "note-warn").map(|r| r.text.clone()).unwrap_or_default();
        assert!(text.contains("5") && text.contains("Bash"), "{text}");
        call(&mut t, "c5");
        call(&mut t, "c6");
        assert_eq!(warnings(&t), 1, "not said again while it goes on");
    }

    #[test]
    fn an_api_retry_from_the_cli_is_said_as_a_warning_with_what_it_knows_of_the_error() {
        let mut t = chat();
        t.apply("claude", &json!({"type": "system", "subtype": "api_retry", "attempt": 2, "max_retries": 10, "retry_delay_ms": 2500, "error_status": 401, "error": "authentication_failed"}), 0);
        let rows = t.rows();
        let note = rows.iter().find(|r| r.kind == "note-warn").expect("a warning row");
        for part in ["2", "10", "401", "authentication_failed", "2.5"] {
            assert!(note.text.contains(part), "{part} in {}", note.text);
        }
        // A retry with nothing else known still says that it happened.
        let mut bare = chat();
        bare.apply("claude", &json!({"type": "system", "subtype": "api_retry"}), 0);
        assert_eq!(warnings(&bare), 1);
        // Other system lines are left alone.
        let mut other = chat();
        other.apply("claude", &json!({"type": "system", "subtype": "hook_started"}), 0);
        assert_eq!(warnings(&other), 0);
    }

    #[test]
    fn a_gateway_switch_is_noted_in_the_transcript_with_the_providers_name_or_as_off() {
        let mut t = chat();
        t.apply("provider", &json!({"provider": {"id": "p1", "name": "Air", "host": "gw.example.com"}}), 0);
        t.apply("provider", &json!({"provider": null}), 0);
        let notes: Vec<String> = t.rows().iter().filter(|r| r.kind == "note").map(|r| r.text.clone()).collect();
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("Air") && !notes[1].contains("Air"), "{notes:?}");
    }

    #[test]
    fn counts_are_written_short() {
        assert_eq!(short_count(999), "999");
        assert_eq!(short_count(1000), "1k");
        assert_eq!(short_count(45_200), "45k");
        assert_eq!(short_count(1_000_000), "1M");
        assert_eq!(context_line(Some(0), Some(0)).as_deref(), Some("0 / 0 · 0%"));
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

#[cfg(test)]
mod steps_tests {
    use super::*;
    use serde_json::json;

    fn chat() -> Transcript {
        let mut t = Transcript::new(Lang::Ko);
        t.replaying = false;
        t.apply("user", &json!({"text": "go"}), 0);
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "start"}]}}), 0);
        t
    }

    /// A hidden thought and one tool call, in one assistant message.
    fn step(t: &mut Transcript, id: &str, name: &str, cmd: &str) {
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "redacted_thinking", "data": "X"}, {"type": "tool_use", "id": id, "name": name, "input": {"command": cmd}}]}}), 0);
    }

    fn result(t: &mut Transcript, id: &str, text: &str, is_error: bool) {
        t.apply("claude", &json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": id, "is_error": is_error, "content": [{"type": "text", "text": text}]}]}}), 0);
    }

    fn end(t: &mut Transcript) {
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "finish"}]}}), 0);
    }

    fn kinds(t: &Transcript) -> Vec<&'static str> {
        t.rows().iter().map(|r| r.kind).collect()
    }

    fn five_bash_and_a_read() -> Transcript {
        let mut t = chat();
        for i in 0..5 {
            // Different commands: five identical ones in a row would be said as a loop.
            step(&mut t, &format!("b{i}"), "Bash", &format!("ls {i}"));
            result(&mut t, &format!("b{i}"), "ok", false);
        }
        step(&mut t, "r1", "Read", "x");
        result(&mut t, "r1", "ok", false);
        end(&mut t);
        t
    }

    #[test]
    fn a_long_run_of_tools_and_hidden_thinking_is_one_steps_row() {
        let t = five_bash_and_a_read();
        let rows = t.rows();
        let at = rows.iter().position(|r| r.kind == "steps").expect("a steps row");
        assert_eq!((rows[at - 1].kind, rows[at + 1].kind), ("para", "para"));
        assert_eq!(rows.len(), at + 2, "nothing of the run is left beside it");
        let row = &rows[at];
        assert_eq!((row.label.as_str(), row.text.as_str(), row.state), ("작업 6단계", "Bash 5 · Read 1 · 생각 6", "done"));
        assert!(row.key.starts_with("g:") && !row.open && row.detail.is_empty());
    }

    #[test]
    fn opening_the_steps_row_lists_its_tools_without_empty_thinking_and_closes_again() {
        let mut t = chat();
        for i in 0..5 {
            step(&mut t, &format!("b{i}"), "Bash", &format!("ls {i}"));
            result(&mut t, &format!("b{i}"), "ok", false);
        }
        // One thought with text stays in the list when it is opened.
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "thinking", "thinking": "a real thought"}]}}), 0);
        step(&mut t, "r1", "Read", "x");
        result(&mut t, "r1", "ok", false);
        end(&mut t);
        let key = t.rows().into_iter().find(|r| r.kind == "steps").unwrap().key;
        t.toggle(&key);
        let rows = t.rows();
        let at = rows.iter().position(|r| r.kind == "steps").unwrap();
        assert!(rows[at].open);
        let inside: Vec<&str> = rows[at + 1..].iter().take_while(|r| r.kind != "para").map(|r| r.kind).collect();
        assert_eq!(inside.iter().filter(|k| **k == "tool").count(), 6);
        assert_eq!(inside.iter().filter(|k| **k == "think").count(), 1, "only the thought that has text");
        assert!(rows[at + 1..].iter().take_while(|r| r.kind != "para").all(|r| r.depth == 1));
        t.toggle(&key);
        assert!(!t.rows().into_iter().find(|r| r.kind == "steps").unwrap().open);
    }

    #[test]
    fn two_tools_stay_as_they_are() {
        let mut t = chat();
        for i in 0..2 {
            step(&mut t, &format!("b{i}"), "Bash", "ls");
            result(&mut t, &format!("b{i}"), "ok", false);
        }
        end(&mut t);
        let k = kinds(&t);
        assert!(!k.contains(&"steps"), "{k:?}");
        assert_eq!(k.iter().filter(|k| **k == "tool").count(), 2);
    }

    #[test]
    fn a_failed_or_held_step_shows_on_the_collapsed_row() {
        let mut t = chat();
        for i in 0..4 {
            step(&mut t, &format!("b{i}"), "Bash", "ls");
        }
        result(&mut t, "b0", "ok", false);
        result(&mut t, "b1", "boom", true);
        result(&mut t, "b2", "SmartWorkbench team mode: not small", true);
        result(&mut t, "b3", "ok", false);
        end(&mut t);
        let row = t.rows().into_iter().find(|r| r.kind == "steps").unwrap();
        assert_eq!(row.state, "err");
        assert!(row.status.contains("오류 1") && row.status.contains("차단 1"), "{}", row.status);
    }

    #[test]
    fn a_run_in_progress_shows_its_latest_step() {
        let mut t = chat();
        for i in 0..3 {
            step(&mut t, &format!("b{i}"), "Bash", "ls");
            result(&mut t, &format!("b{i}"), "ok", false);
        }
        step(&mut t, "b3", "Bash", "make release");
        let row = t.rows().into_iter().find(|r| r.kind == "steps").unwrap();
        assert_eq!(row.state, "running");
        assert!(row.detail.starts_with("Bash: make release"), "{}", row.detail);
        assert!(row.status.contains("실행 중"));
    }

    #[test]
    fn a_sub_agents_work_stays_under_its_card_in_a_run() {
        let mut t = chat();
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "a1", "name": "Agent", "input": {"description": "look"}}]}}), 0);
        t.apply("claude", &json!({"type": "assistant", "parent_tool_use_id": "a1", "message": {"content": [{"type": "tool_use", "id": "c1", "name": "Read", "input": {"file_path": "x"}}]}}), 0);
        for i in 0..2 {
            step(&mut t, &format!("b{i}"), "Bash", "ls");
            result(&mut t, &format!("b{i}"), "ok", false);
        }
        end(&mut t);
        let key = t.rows().into_iter().find(|r| r.kind == "steps").expect("three top-level tools make a run").key;
        assert!(!t.rows().iter().any(|r| r.kind == "tool" && r.label == "Read"), "folded: the sub-agent's Read is not a row");
        t.toggle(&key);
        let read = t.rows().into_iter().find(|r| r.kind == "tool" && r.label == "Read").expect("opened: it is there");
        assert_eq!(read.depth, 2);
    }

    #[test]
    fn a_group_key_does_not_touch_code_folds_or_tools() {
        let mut t = five_bash_and_a_read();
        let before = t.expanded.clone();
        t.toggle("g:x");
        t.toggle("g:");
        assert_eq!(t.expanded, before);
        t.toggle("g:3");
        assert!(t.expanded.contains("g:3") && t.rows().iter().filter(|r| r.kind == "tool").all(|r| !r.open));
        t.toggle("g:3");
        assert_eq!(t.expanded, before);
    }
}

#[cfg(test)]
mod md_rows_tests {
    use super::*;

    fn kinds(rows: &[Row]) -> Vec<&'static str> {
        rows.iter().map(|r| r.kind).collect()
    }

    #[test]
    fn a_markdown_file_is_drawn_with_the_rows_of_an_answer() {
        let src = "# Title\n\nSome *text* and a [link](https://x.y).\n\n- one\n- two\n\n> quoted\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n---\n";
        let rows = md_rows(src, Lang::En);
        assert_eq!(kinds(&rows), ["heading", "para", "item", "item", "quote", "table", "rule"]);
        assert_eq!(rows[0].extra, "1");
        assert_eq!(rows[5].grid, vec![vec!["a".to_owned(), "b".to_owned()], vec!["1".to_owned(), "2".to_owned()]]);
        assert!(rows.iter().all(|r| r.depth == 0 && !r.key.is_empty()));
        let keys: std::collections::HashSet<_> = rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys.len(), rows.len(), "every row has its own key");
    }

    #[test]
    fn a_front_matter_block_is_one_code_row_and_a_long_code_block_is_not_folded() {
        let long: String = (0..40).map(|n| format!("line {n}\n")).collect();
        let src = format!("---\ntitle: T\ntags: [a]\n---\n# After\n\n```rust\n{long}```\n");
        let rows = md_rows(&src, Lang::En);
        assert_eq!(kinds(&rows), ["code", "heading", "code"]);
        assert_eq!((rows[0].label.as_str(), rows[0].text.as_str()), ("yaml", "title: T\ntags: [a]"));
        assert_eq!(rows[2].label, "rust");
        assert_eq!(rows[2].text.lines().count(), 40);
        assert_eq!(rows[2].add, 0, "nothing folds in a file");
        assert!(rows[2].extra.contains("40"), "{}", rows[2].extra);
    }

    #[test]
    fn a_line_of_dashes_that_is_not_a_front_matter_stays_a_rule_and_a_byte_order_mark_is_dropped() {
        let rows = md_rows("---\nnot closed\n\ntext\n", Lang::En);
        assert!(!rows.iter().any(|r| r.label == "yaml"), "{:?}", kinds(&rows));
        let rows = md_rows("\u{feff}# T\n", Lang::En);
        assert_eq!(kinds(&rows), ["heading"]);
        let far = format!("---\n{}---\n# T\n", "a: b\n".repeat(150));
        assert!(!md_rows(&far, Lang::En).iter().any(|r| r.label == "yaml"), "a front matter that does not end in 100 lines is not one");
        assert!(md_rows("", Lang::En).is_empty());
        assert!(md_rows("  \n\n", Lang::En).is_empty());
    }
}

#[cfg(test)]
mod plain_text_tests {
    use super::*;
    use serde_json::json;

    fn chat() -> Transcript {
        Transcript::new(Lang::En)
    }

    fn say(t: &mut Transcript, text: &str, ts: u64) {
        t.apply("user", &json!({"text": text}), ts);
    }

    fn reply(t: &mut Transcript, blocks: Value, ts: u64) {
        t.apply("claude", &json!({"type": "assistant", "message": {"content": blocks}}), ts);
    }

    fn tool(id: &str, name: &str, input: Value) -> Value {
        json!({"type": "tool_use", "id": id, "name": name, "input": input})
    }

    #[test]
    fn a_conversation_becomes_text_with_the_words_as_written_and_a_line_for_each_tool() {
        let mut t = chat();
        say(&mut t, "list the files", 1_700_000_000_000);
        let long: String = (0..40).map(|n| format!("line {n}\n")).collect();
        reply(&mut t, json!([{"type": "thinking", "thinking": "hmm"}, {"type": "text", "text": format!("Here **is** the list:\n\n```sh\n{long}```")}, tool("a", "Bash", json!({"command": "ls   -la"})), tool("b", "Read", json!({"file_path": "a.rs"}))]), 1_700_000_001_000);
        let text = t.plain_text();
        assert!(text.contains("list the files") && text.contains("Here **is** the list:"), "{text}");
        assert!(text.contains("line 0\n") && text.contains("line 39\n```"), "a code block is shown whole and fenced: {text}");
        assert!(!text.contains("hmm"), "thinking is left out");
        assert!(text.contains("[Bash] ls -la\n[Read] a.rs"), "one line for each tool, no blank line between: {text}");
        assert!(text.starts_with("──"), "{text}");
    }

    #[test]
    fn folded_steps_a_sub_agents_calls_and_images_give_the_same_text_whatever_is_open() {
        let mut t = chat();
        say(&mut t, "go", 1);
        reply(&mut t, json!([tool("p", "Agent", json!({"description": "look"})), tool("a", "Bash", json!({"command": "one"})), tool("b", "Bash", json!({"command": "two"})), tool("c", "Bash", json!({"command": "three"}))]), 2);
        t.apply("claude", &json!({"type": "assistant", "parent_tool_use_id": "p", "message": {"content": [tool("k", "Read", json!({"file_path": "inner.rs"}))]}}), 3);
        let closed = t.plain_text();
        assert!(!closed.contains("inner.rs"), "a sub-agent's own calls are not listed: {closed}");
        assert!(closed.contains("[Agent]") && closed.contains("[Bash] one") && closed.contains("[Bash] three"), "{closed}");
        let rows = t.rows();
        let key = rows.iter().find(|r| r.kind == "steps").map(|r| r.key.clone()).expect("three tools in a row are folded into a steps row");
        t.toggle(&key);
        assert_eq!(t.plain_text(), closed, "open or folded on screen, the text is the same");

        let mut with_image = chat();
        with_image.apply("user", &json!({"text": "see", "images": [{"media_type": "image/png", "data": "AAAA"}, {"media_type": "image/png", "data": "BBBB"}]}), 1);
        assert!(with_image.plain_text().contains("[2 images]"), "{}", with_image.plain_text());
    }

    #[test]
    fn one_message_is_copied_up_to_the_next_one_and_a_wrong_key_gives_nothing() {
        let mut t = chat();
        say(&mut t, "first question", 1);
        reply(&mut t, json!([{"type": "text", "text": "first answer"}, tool("a", "Bash", json!({"command": "ls"}))]), 2);
        say(&mut t, "second question", 3);
        reply(&mut t, json!([{"type": "text", "text": "second answer"}]), 4);
        let rows = t.rows();
        let heads: Vec<&Row> = rows.iter().filter(|r| r.kind == "meta-user" || r.kind == "meta-asst").collect();
        assert_eq!(heads.len(), 4, "{:?}", rows.iter().map(|r| r.kind).collect::<Vec<_>>());
        assert_eq!(t.message_text(&heads[0].key).as_deref(), Some("first question"));
        assert_eq!(t.message_text(&heads[1].key).as_deref(), Some("first answer\n\n[Bash] ls"));
        assert_eq!(t.message_text(&heads[3].key).as_deref(), Some("second answer"));
        assert!(t.message_text("n9999").is_none() && t.message_text("x").is_none() && t.message_text("n").is_none());
        let some_text_row = rows.iter().find(|r| r.kind == "para").unwrap();
        assert!(t.message_text(&some_text_row.key).is_none(), "only a message's header row names a message");
    }

    #[test]
    fn a_team_change_is_a_one_line_note() {
        let mut t = Transcript::new(Lang::Ko);
        t.apply("team", &json!({"summary": "on · lead high · explorer haiku · advisor opus"}), 1);
        let text = format!("{:?}", t.rows().iter().map(|r| r.text.clone()).collect::<Vec<_>>());
        assert!(text.contains("explorer haiku") && text.contains("advisor opus"), "{text}");
    }

    #[test]
    fn the_files_of_a_message_are_named_above_its_words_without_their_content() {
        let mut t = Transcript::new(Lang::Ko);
        t.apply("user", &json!({"text": "봐 주세요", "images": [], "files": [{"name": "a.pdf", "size": 2048, "kind": "pdf"}, {"name": "z.bin", "size": 7, "kind": "file"}]}), 1);
        let rows = t.rows();
        let user = rows.iter().find(|r| r.kind == "user").expect("a user row");
        assert!(user.text.contains("a.pdf") && user.text.contains("2.0 KB") && user.text.contains("z.bin") && user.text.ends_with("봐 주세요"), "{}", user.text);
    }

    fn edit_event(t: &mut Transcript, id: &str, name: &str, input: Value) {
        t.apply("claude", &json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}}), 1);
    }
    fn result_event(t: &mut Transcript, id: &str, text: &str, structured: Value) {
        t.apply("claude", &json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text}]}, "tool_use_result": structured}), 2);
    }
    fn tool_row(t: &Transcript, label: &str) -> Row {
        t.rows().into_iter().find(|r| r.kind == "tool" && r.label == label).expect("a tool row")
    }

    #[test]
    fn an_edit_is_drawn_from_its_structured_patch_with_context_and_line_numbers_and_opens_by_itself() {
        let mut t = Transcript::new(Lang::Ko);
        edit_event(&mut t, "e1", "Edit", json!({"file_path": "/p/a.rs", "old_string": "b", "new_string": "c"}));
        assert!(!tool_row(&t, "Edit").open, "closed while it runs");
        let patch = json!({"filePath": "/p/a.rs", "structuredPatch": [
            {"oldStart": 10, "oldLines": 3, "newStart": 10, "newLines": 3, "lines": [" keep", "-old", "+new", " tail"]},
            {"oldStart": 40, "oldLines": 1, "newStart": 40, "newLines": 2, "lines": ["-x", "+y", "+z", "\\ No newline at end of file"]}]});
        result_event(&mut t, "e1", "The file /p/a.rs has been updated successfully.", patch);
        let row = tool_row(&t, "Edit");
        assert!(row.open);
        let shown: Vec<(char, u32, &str)> = row.diff.iter().map(|l| (l.kind, l.no, l.text.as_str())).collect();
        assert_eq!(shown, [(' ', 10, "keep"), ('-', 11, "old"), ('+', 11, "new"), (' ', 12, "tail"), ('@', 0, ""), ('-', 40, "x"), ('+', 40, "y"), ('+', 41, "z")]);
        assert_eq!((row.add, row.del), (3, 2));
        assert!(!row.detail.contains("successfully"), "the 'updated' sentence adds nothing to a diff: {}", row.detail);
    }

    #[test]
    fn a_card_the_person_closed_stays_closed_when_its_result_comes_and_a_failure_shows_its_text() {
        let mut t = Transcript::new(Lang::Ko);
        edit_event(&mut t, "e1", "Edit", json!({"file_path": "/p/a.rs", "old_string": "b", "new_string": "c"}));
        t.toggle("e1");
        t.toggle("e1");
        result_event(&mut t, "e1", "ok", json!({"structuredPatch": [{"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1, "lines": ["-a", "+b"]}]}));
        assert!(!tool_row(&t, "Edit").open, "the person had touched it");
        // An edit that failed has no diff of the result's; its text is what is shown when it is opened.
        edit_event(&mut t, "e2", "Edit", json!({"file_path": "/p/b.rs", "old_string": "b", "new_string": "c"}));
        t.apply("claude", &json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "e2", "is_error": true, "content": "String to replace not found"}]}}), 3);
        t.toggle("e2");
        let row = t.rows().into_iter().find(|r| r.key == "e2").expect("the failed edit");
        assert_eq!(row.state, "err");
        assert!(row.detail.contains("String to replace not found"), "{}", row.detail);
    }

    #[test]
    fn a_created_file_shows_its_content_as_added_lines_and_a_long_diff_is_cut_with_a_count() {
        let mut t = Transcript::new(Lang::Ko);
        edit_event(&mut t, "w1", "Write", json!({"file_path": "/p/n.txt", "content": "ignored"}));
        let content = (1..=150).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        result_event(&mut t, "w1", "created", json!({"type": "create", "filePath": "/p/n.txt", "content": content, "structuredPatch": []}));
        let row = tool_row(&t, "Write");
        assert_eq!((row.add, row.del), (150, 0));
        assert_eq!(row.diff.len(), MAX_DIFF_ROWS + 1);
        assert_eq!((row.diff[0].kind, row.diff[0].no, row.diff[0].text.as_str()), ('+', 1, "line 1"));
        let last = row.diff.last().unwrap();
        assert_eq!(last.kind, '…');
        assert!(last.text.contains("30"), "{}", last.text);
    }

    #[test]
    fn edits_are_never_folded_into_steps_and_a_command_shows_its_first_lines_while_closed() {
        let mut t = Transcript::new(Lang::Ko);
        for i in 0..4 {
            edit_event(&mut t, &format!("b{i}"), "Bash", json!({"command": format!("ls {i}")}));
            result_event(&mut t, &format!("b{i}"), "one\ntwo\nthree\nfour\nfive", json!({"stdout": "x"}));
        }
        edit_event(&mut t, "e1", "Edit", json!({"file_path": "/p/a.rs", "old_string": "b", "new_string": "c"}));
        result_event(&mut t, "e1", "ok", json!({"structuredPatch": [{"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1, "lines": ["-a", "+b"]}]}));
        let rows = t.rows();
        assert!(rows.iter().any(|r| r.kind == "steps"), "the commands are folded");
        let edit = rows.iter().find(|r| r.kind == "tool" && r.label == "Edit").expect("the edit is its own card");
        assert!(edit.open && edit.diff.len() == 2);
        // Opened, the folded commands show a preview of their output.
        let mut t = Transcript::new(Lang::Ko);
        edit_event(&mut t, "b", "Bash", json!({"command": "ls"}));
        result_event(&mut t, "b", "one\ntwo\nthree\nfour\nfive", Value::Null);
        let preview = tool_row(&t, "Bash").preview;
        assert!(preview.starts_with("one\ntwo\nthree\n") && preview.contains("+2"), "{preview}");
        t.toggle("b");
        assert_eq!(tool_row(&t, "Bash").preview, "", "no preview once it is open");
    }

    #[test]
    fn a_read_says_how_many_lines_it_gave() {
        let mut t = Transcript::new(Lang::Ko);
        edit_event(&mut t, "r1", "Read", json!({"file_path": "/p/a.rs", "offset": 10, "limit": 50}));
        result_event(&mut t, "r1", "…", json!({"type": "text", "file": {"filePath": "/p/a.rs", "numLines": 50, "startLine": 10, "totalLines": 99}}));
        let row = tool_row(&t, "Read");
        assert!(row.extra.contains("L10-60") && row.extra.contains("50"), "{}", row.extra);
    }

    #[test]
    fn the_tail_of_a_text_starts_at_a_line_keeps_whole_characters_and_counts_what_it_left_out() {
        assert_eq!(tail_chars("short", 100), ("short", 0));
        let text = "aaa\nbbb\nccc\nddd";
        let (tail, left) = tail_chars(text, 9);
        assert_eq!((tail, left), ("ccc\nddd", 8), "the cut falls inside 'bbb', so the tail starts at the next line");
        let korean = "가나다\n라마바\n사아자";
        let (tail, left) = tail_chars(korean, 7);
        assert_eq!((tail, left), ("사아자", 8), "cut on character boundaries: {tail}");
        let (tail, left) = tail_chars("one long line with no break", 10);
        assert_eq!((tail.chars().count(), left), (10, 17), "no line break after the cut: the cut stays");
    }
}
