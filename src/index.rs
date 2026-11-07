//! A SQLite FTS5 index over every session's log: one row per message block, searched by the sidebar and by the `search_history` tool.
//!
//! One writer thread owns the connection and takes jobs from a channel (so `emit` never waits on the disk); reads use a second
//! read-only connection. The index is only a cache of the `.jsonl` logs: it is brought up to date at start (`catch_up`), is
//! idempotent (a per-session `last_seq`), and is thrown away and rebuilt when it is old or damaged.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use rusqlite::types::Value as Sql;
use rusqlite::{params, params_from_iter, Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;

use crate::session::Envelope;

/// Bump when the schema or what is indexed changes: an index of another version is rebuilt from the logs.
pub const SCHEMA_VERSION: i64 = 1;
/// The tool whose own results are never indexed (they would be indexed again by every search).
pub const OWN_TOOL: &str = "mcp__swb_app__search_history";

const MAX_TEXT: usize = 32 * 1024;
const MAX_RESULT: usize = 2 * 1024;
const MAX_SUMMARY: usize = 300;
pub const MARK_OPEN: char = '\u{2}';
pub const MARK_CLOSE: char = '\u{3}';

// ---- what is indexed ----

/// One searchable piece of an event: (part number within the event, role, text).
pub type Piece = (i64, &'static str, String);

fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn summary(input: &Value) -> String {
    for key in ["command", "file_path", "path", "pattern", "url", "query", "description"] {
        if let Some(text) = input.get(key).and_then(Value::as_str) {
            return cut(text.lines().next().unwrap_or(""), MAX_SUMMARY);
        }
    }
    String::new()
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// The searchable text of one envelope. Images, thinking, sub-agent messages, permissions, stderr and the search tool's own
/// calls and results are left out; `own_calls` remembers the tool-use ids of the search tool across the events of a session.
pub fn extract(envelope: &Envelope, own_calls: &mut HashSet<String>) -> Vec<Piece> {
    let mut out = Vec::new();
    let data = &envelope.data;
    match envelope.kind.as_str() {
        "user" => {
            if let Some(text) = data["text"].as_str().filter(|t| !t.trim().is_empty()) {
                out.push((0, "user", cut(text, MAX_TEXT)));
            }
        }
        "claude" if data["parent_tool_use_id"].is_null() => {
            let blocks = data["message"]["content"].as_array();
            match (data["type"].as_str(), blocks) {
                (Some("assistant"), Some(blocks)) => {
                    for (index, block) in blocks.iter().enumerate() {
                        match block["type"].as_str() {
                            Some("text") => {
                                if let Some(text) = block["text"].as_str().filter(|t| !t.trim().is_empty()) {
                                    out.push((index as i64, "assistant", cut(text, MAX_TEXT)));
                                }
                            }
                            Some("tool_use") => {
                                let name = block["name"].as_str().unwrap_or_default();
                                if name == OWN_TOOL {
                                    if let Some(id) = block["id"].as_str() {
                                        own_calls.insert(id.to_owned());
                                    }
                                } else {
                                    out.push((index as i64, "tool", format!("{name}: {}", summary(&block["input"]))));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                (Some("user"), Some(blocks)) => {
                    for (index, block) in blocks.iter().enumerate() {
                        if block["type"] != "tool_result" || block["tool_use_id"].as_str().map(|id| own_calls.contains(id)).unwrap_or(false) {
                            continue;
                        }
                        let text = result_text(&block["content"]);
                        if !text.trim().is_empty() {
                            out.push((index as i64, "result", cut(&text, MAX_RESULT)));
                        }
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
    out
}

// ---- the query ----

/// A search as the sidebar or the tool states it. The text is plain words: no FTS syntax is accepted.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub q: String,
    pub cwd: Option<String>,
    pub sid: Option<String>,
    /// Only messages at or after this time (ms since the epoch).
    pub since: Option<u64>,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Hit {
    pub sid: String,
    pub seq: u64,
    pub part: i64,
    pub role: String,
    pub ts: u64,
    pub cwd: String,
    /// The matching text with MARK_OPEN and MARK_CLOSE around the match; the page escapes it before drawing.
    pub snippet: String,
}

#[derive(Debug, PartialEq)]
struct Plan {
    /// Terms of three or more characters, each quoted for FTS5 (trigram) and joined by AND.
    fts: Option<String>,
    /// Shorter terms (a two-letter Korean word): trigram cannot see them, so they are LIKE filters.
    likes: Vec<String>,
    /// Every term as typed (quotes removed), for marking the match in the text.
    terms: Vec<String>,
}

fn plan(q: &str) -> Option<Plan> {
    // Quote marks only separate words here: a quoted word is searched as the word.
    let cleaned: String = q.chars().map(|c| if c == '"' { ' ' } else { c }).filter(|c| !c.is_control()).take(200).collect();
    let words: Vec<&str> = cleaned.split_whitespace().take(8).collect();
    if words.is_empty() {
        return None;
    }
    let terms: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
    let (long, short): (Vec<&str>, Vec<&str>) = words.into_iter().partition(|w| w.chars().count() >= 3);
    let fts = (!long.is_empty()).then(|| long.iter().map(|w| format!("\"{}\"", w.replace('"', "\"\""))).collect::<Vec<_>>().join(" AND "));
    Some(Plan { fts, likes: short.into_iter().map(str::to_owned).collect(), terms })
}

fn like_pattern(term: &str) -> String {
    let escaped = term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

/// Where `needle` first occurs in `hay` (both already lower-cased, as characters).
fn find_at(hay: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

/// About 280 characters of the text around the first matching term, every occurrence of any term marked
/// (case-insensitive), cut on character boundaries. FTS5's own snippet() counts trigrams, which is a few characters.
fn around(text: &str, terms: &[String]) -> String {
    let chars: Vec<char> = text.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let needles: Vec<Vec<char>> = terms.iter().map(|t| t.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect()).collect();
    let first = needles.iter().filter_map(|n| find_at(&lower, n, 0)).min().unwrap_or(0);
    let from = first.saturating_sub(60);
    let to = (first + 220).min(chars.len());

    let mut opens: HashSet<usize> = HashSet::new();
    let mut closes: HashSet<usize> = HashSet::new();
    for needle in &needles {
        let mut at = from;
        while let Some(i) = find_at(&lower, needle, at) {
            if i + needle.len() > to {
                break;
            }
            opens.insert(i);
            closes.insert(i + needle.len());
            at = i + needle.len();
        }
    }
    let mut out = String::new();
    if from > 0 {
        out.push('…');
    }
    for (i, c) in chars.iter().enumerate().take(to).skip(from) {
        if closes.contains(&i) {
            out.push(MARK_CLOSE);
        }
        if opens.contains(&i) {
            out.push(MARK_OPEN);
        }
        out.push(*c);
    }
    if closes.contains(&to) {
        out.push(MARK_CLOSE);
    }
    if to < chars.len() {
        out.push('…');
    }
    out
}

// ---- the index ----

enum Job {
    Add { cwd: String, fallback_ts: u64, envelope: Box<Envelope> },
    CatchUp { sid: String, cwd: String, fallback_ts: u64, log: Vec<Envelope> },
    Keep(HashSet<String>),
    Flush(mpsc::Sender<()>),
}

pub struct Index {
    jobs: mpsc::Sender<Job>,
    reader: Mutex<Connection>,
    writer: Mutex<Option<JoinHandle<()>>>,
}

const SCHEMA: &str = "
CREATE TABLE msg(id INTEGER PRIMARY KEY, sid TEXT NOT NULL, seq INTEGER NOT NULL, part INTEGER NOT NULL, role TEXT NOT NULL, cwd TEXT NOT NULL, ts INTEGER NOT NULL, text TEXT NOT NULL, UNIQUE(sid, seq, part));
CREATE INDEX msg_cwd_ts ON msg(cwd, ts);
CREATE TABLE indexed(sid TEXT PRIMARY KEY, last_seq INTEGER NOT NULL);
CREATE VIRTUAL TABLE msg_fts USING fts5(text, content='msg', content_rowid='id', tokenize='trigram case_sensitive 0');
CREATE TRIGGER msg_ai AFTER INSERT ON msg BEGIN INSERT INTO msg_fts(rowid, text) VALUES (new.id, new.text); END;
CREATE TRIGGER msg_ad AFTER DELETE ON msg BEGIN INSERT INTO msg_fts(msg_fts, rowid, text) VALUES ('delete', old.id, old.text); END;
";

fn create_private_file(path: &Path) -> std::io::Result<()> {
    // The file exists with mode 0600 before SQLite opens it; the -wal and -shm files copy its permissions.
    crate::private::write(path, "")
}

fn remove_db(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(name));
    }
}

fn open_writer(path: &Path) -> Result<Connection> {
    let fresh = !path.exists() || std::fs::metadata(path).map(|m| m.len() == 0).unwrap_or(true);
    if !path.exists() {
        create_private_file(path)?;
    }
    let conn = Connection::open(path).context("could not open the index")?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    if fresh {
        conn.execute_batch(SCHEMA)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        return Ok(conn);
    }
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let sound: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0)).unwrap_or_default();
    if version != SCHEMA_VERSION || sound != "ok" {
        anyhow::bail!("the index is of another version or damaged");
    }
    Ok(conn)
}

impl Index {
    /// Opens (or rebuilds from nothing) the index at `path`; `catch_up` then fills it from the logs.
    pub fn open(path: &Path) -> Result<Arc<Self>> {
        let writer_conn = match open_writer(path) {
            Ok(conn) => conn,
            Err(_) => {
                remove_db(path);
                open_writer(path)?
            }
        };
        let reader = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).context("could not open the index for reading")?;
        let (jobs, rx) = mpsc::channel::<Job>();
        let handle = std::thread::Builder::new().name("swb-index".into()).spawn(move || writer_loop(writer_conn, rx))?;

        Ok(Arc::new(Self { jobs, reader: Mutex::new(reader), writer: Mutex::new(Some(handle)) }))
    }

    /// Queues one new event of a session (never waits on the disk).
    pub fn add(&self, cwd: &str, fallback_ts: u64, envelope: &Envelope) {
        let _ = self.jobs.send(Job::Add { cwd: cwd.to_owned(), fallback_ts, envelope: Box::new(envelope.clone()) });
    }

    /// Queues a session's whole log: only what the index has not seen is added; a log shorter than the index starts over.
    pub fn catch_up(&self, sid: &str, cwd: &str, fallback_ts: u64, log: Vec<Envelope>) {
        let _ = self.jobs.send(Job::CatchUp { sid: sid.to_owned(), cwd: cwd.to_owned(), fallback_ts, log });
    }

    /// Queues the removal of every session that is not in `sids`.
    pub fn keep_only(&self, sids: HashSet<String>) {
        let _ = self.jobs.send(Job::Keep(sids));
    }

    /// Waits until everything queued so far is written.
    pub fn flush(&self) {
        let (tx, rx) = mpsc::channel();
        if self.jobs.send(Job::Flush(tx)).is_ok() {
            let _ = rx.recv();
        }
    }

    pub fn search(&self, query: &SearchQuery) -> Result<Vec<Hit>> {
        let Some(plan) = plan(&query.q) else { return Ok(Vec::new()) };
        let limit = query.limit.clamp(1, 50) as i64;
        let conn = self.reader.lock().map_err(|_| anyhow::anyhow!("the index lock is poisoned"))?;

        let mut filters = String::new();
        let mut args: Vec<Sql> = Vec::new();
        let push_filters = |args: &mut Vec<Sql>, filters: &mut String| {
            if let Some(cwd) = &query.cwd {
                filters.push_str(" AND m.cwd = ?");
                args.push(Sql::Text(cwd.clone()));
            }
            if let Some(sid) = &query.sid {
                filters.push_str(" AND m.sid = ?");
                args.push(Sql::Text(sid.clone()));
            }
            if let Some(since) = query.since {
                filters.push_str(" AND m.ts >= ?");
                args.push(Sql::Integer(since as i64));
            }
            for term in &plan.likes {
                filters.push_str(" AND m.text LIKE ? ESCAPE '\\'");
                args.push(Sql::Text(like_pattern(term)));
            }
        };

        let hits = if let Some(fts) = &plan.fts {
            args.push(Sql::Text(fts.clone()));
            push_filters(&mut args, &mut filters);
            args.push(Sql::Integer(limit));
            let sql = format!(
                "SELECT m.sid, m.seq, m.part, m.role, m.ts, m.cwd, m.text FROM msg_fts JOIN msg m ON m.id = msg_fts.rowid WHERE msg_fts MATCH ?{filters} ORDER BY bm25(msg_fts) LIMIT ?"
            );
            let mut statement = conn.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(args.iter()), |r| {
                let text: String = r.get(6)?;
                Ok(Hit { sid: r.get(0)?, seq: r.get::<_, i64>(1)? as u64, part: r.get(2)?, role: r.get(3)?, ts: r.get::<_, i64>(4)? as u64, cwd: r.get(5)?, snippet: around(&text, &plan.terms) })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            push_filters(&mut args, &mut filters);
            args.push(Sql::Integer(limit));
            let sql = format!("SELECT m.sid, m.seq, m.part, m.role, m.ts, m.cwd, m.text FROM msg m WHERE 1=1{filters} ORDER BY m.ts DESC, m.seq DESC LIMIT ?");
            let mut statement = conn.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(args.iter()), |r| {
                let text: String = r.get(6)?;
                Ok(Hit { sid: r.get(0)?, seq: r.get::<_, i64>(1)? as u64, part: r.get(2)?, role: r.get(3)?, ts: r.get::<_, i64>(4)? as u64, cwd: r.get(5)?, snippet: around(&text, &plan.terms) })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        Ok(hits)
    }
}

impl Drop for Index {
    fn drop(&mut self) {
        // Close the channel, let the writer finish what is queued, and wait for it: a later open of the same file must not meet it.
        let (closed, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.jobs, closed));
        if let Ok(mut guard) = self.writer.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
    }
}

fn writer_loop(mut conn: Connection, rx: mpsc::Receiver<Job>) {
    let mut own_calls: HashSet<String> = HashSet::new();
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while batch.len() < 200 {
            match rx.try_recv() {
                Ok(job) => batch.push(job),
                Err(_) => break,
            }
        }
        let mut flushes = Vec::new();
        let result = (|| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            for job in &batch {
                match job {
                    Job::Add { cwd, fallback_ts, envelope } => insert(&tx, cwd, *fallback_ts, envelope, &mut own_calls)?,
                    Job::CatchUp { sid, cwd, fallback_ts, log } => catch_up(&tx, sid, cwd, *fallback_ts, log, &mut own_calls)?,
                    Job::Keep(keep) => {
                        let known: Vec<String> = tx.prepare("SELECT sid FROM indexed")?.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
                        for sid in known.into_iter().filter(|sid| !keep.contains(sid)) {
                            tx.execute("DELETE FROM msg WHERE sid = ?1", [&sid])?;
                            tx.execute("DELETE FROM indexed WHERE sid = ?1", [&sid])?;
                        }
                    }
                    Job::Flush(_) => {}
                }
            }
            tx.commit()
        })();
        if let Err(error) = result {
            tracing::warn!("index write failed: {error}");
        }
        for job in batch {
            if let Job::Flush(done) = job {
                flushes.push(done);
            }
        }
        for done in flushes {
            let _ = done.send(());
        }
    }
}

fn insert(tx: &rusqlite::Transaction, cwd: &str, fallback_ts: u64, envelope: &Envelope, own_calls: &mut HashSet<String>) -> rusqlite::Result<()> {
    let last: i64 = tx.query_row("SELECT COALESCE((SELECT last_seq FROM indexed WHERE sid = ?1), 0)", [&envelope.sid], |r| r.get(0))?;
    if envelope.seq as i64 <= last {
        return Ok(());
    }
    let ts = if envelope.ts > 0 { envelope.ts } else { fallback_ts };
    for (part, role, text) in extract(envelope, own_calls) {
        tx.execute("INSERT OR IGNORE INTO msg(sid, seq, part, role, cwd, ts, text) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![envelope.sid, envelope.seq as i64, part, role, cwd, ts as i64, text])?;
    }
    tx.execute("INSERT INTO indexed(sid, last_seq) VALUES (?1, ?2) ON CONFLICT(sid) DO UPDATE SET last_seq = excluded.last_seq", params![envelope.sid, envelope.seq as i64])?;
    Ok(())
}

fn catch_up(tx: &rusqlite::Transaction, sid: &str, cwd: &str, fallback_ts: u64, log: &[Envelope], own_calls: &mut HashSet<String>) -> rusqlite::Result<()> {
    let end = log.iter().map(|e| e.seq).max().unwrap_or(0) as i64;
    let last: i64 = tx.query_row("SELECT COALESCE((SELECT last_seq FROM indexed WHERE sid = ?1), 0)", [sid], |r| r.get(0))?;
    // The log was replaced by a shorter one: what the index holds for it is of another log.
    if last > end {
        tx.execute("DELETE FROM msg WHERE sid = ?1", [sid])?;
        tx.execute("DELETE FROM indexed WHERE sid = ?1", [sid])?;
    }
    // The own-tool ids are learnt from the whole log, so a result after a restart is still recognised.
    for envelope in log {
        extract_own(envelope, own_calls);
    }
    for envelope in log {
        insert(tx, cwd, fallback_ts, envelope, own_calls)?;
    }
    Ok(())
}

fn extract_own(envelope: &Envelope, own_calls: &mut HashSet<String>) {
    if envelope.kind == "claude" && envelope.data["type"] == "assistant" {
        for block in envelope.data["message"]["content"].as_array().into_iter().flatten() {
            if block["type"] == "tool_use" && block["name"] == OWN_TOOL {
                if let Some(id) = block["id"].as_str() {
                    own_calls.insert(id.to_owned());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env(sid: &str, seq: u64, kind: &str, data: Value) -> Envelope {
        Envelope { v: 1, sid: sid.into(), seq, ts: 1000 + seq, kind: kind.into(), data }
    }

    fn user(sid: &str, seq: u64, text: &str) -> Envelope {
        env(sid, seq, "user", json!({"text": text, "images": []}))
    }

    fn assistant(sid: &str, seq: u64, blocks: Value) -> Envelope {
        env(sid, seq, "claude", json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": "m", "content": blocks}}))
    }

    fn search(index: &Index, q: &str) -> Vec<Hit> {
        index.flush();
        index.search(&SearchQuery { q: q.into(), limit: 20, ..Default::default() }).unwrap()
    }

    fn fixture() -> (tempfile::TempDir, Arc<Index>) {
        let dir = tempfile::tempdir().unwrap();
        let index = Index::open(&dir.path().join("index.sqlite")).unwrap();
        (dir, index)
    }

    #[test]
    fn korean_english_and_short_words_are_found() {
        let (_dir, index) = fixture();
        index.add("/p", 0, &user("s1", 1, "마이그레이션 스크립트를 작성해 줘"));
        index.add("/p", 0, &assistant("s1", 2, json!([{"type": "text", "text": "The Build failed with a Linker error"}])));
        index.add("/p", 0, &user("s2", 1, "검색 기능을 넣자"));
        assert_eq!(search(&index, "마이그레이션").len(), 1);
        assert_eq!(search(&index, "베이스").len(), 0);
        assert_eq!(search(&index, "그레이").len(), 1, "a part of a word");
        assert_eq!(search(&index, "linker").len(), 1, "case-insensitive");
        assert_eq!(search(&index, "build failed").len(), 1, "every word must be there");
        let short = search(&index, "검색");
        assert_eq!(short.len(), 1, "a two-letter word goes through LIKE");
        assert_eq!(short[0].sid, "s2");
        assert!(short[0].snippet.contains(MARK_OPEN) && short[0].snippet.contains('검'));
        assert_eq!(search(&index, "검색 기능을").len(), 1, "short and long words together");
    }

    #[test]
    fn query_syntax_is_plain_words_and_never_an_error() {
        let (_dir, index) = fixture();
        index.add("/p", 0, &user("s1", 1, "a \"quoted\" word and star* NEAR text"));
        for q in ["\"", "\"\"\"", "NEAR(a b)", "text:foo", "col: x", "a AND", "*", "^x", "(", "a OR b", "% _", "\\", ""] {
            assert!(index.search(&SearchQuery { q: q.into(), limit: 5, ..Default::default() }).is_ok(), "query {q:?}");
        }
        assert_eq!(search(&index, "\"quoted\"").len(), 1);
        assert_eq!(search(&index, "star*").len(), 1);
    }

    #[test]
    fn only_text_is_indexed_and_the_search_tools_own_results_are_left_out() {
        let (_dir, index) = fixture();
        index.add("/p", 0, &user("s1", 1, "please run it"));
        index.add("/p", 0, &assistant("s1", 2, json!([{"type": "thinking", "thinking": "SECRETTHOUGHT"}, {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cargo build --release"}}])));
        index.add("/p", 0, &env("s1", 3, "claude", json!({"type": "user", "parent_tool_use_id": null, "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": "compiled everything fine"}]}})));
        index.add("/p", 0, &assistant("s1", 4, json!([{"type": "tool_use", "id": "own", "name": OWN_TOOL, "input": {"query": "needle"}}])));
        index.add("/p", 0, &env("s1", 5, "claude", json!({"type": "user", "parent_tool_use_id": null, "message": {"content": [{"type": "tool_result", "tool_use_id": "own", "content": "OWNRESULT excerpt text"}]}})));
        index.add("/p", 0, &env("s1", 6, "claude", json!({"type": "assistant", "parent_tool_use_id": "sub", "message": {"content": [{"type": "text", "text": "SUBAGENTTEXT"}]}})));
        index.add("/p", 0, &env("s1", 7, "user", json!({"text": "see image", "images": [{"media_type": "image/png", "data": "QUFBQUJBU0U2NERBVEE="}]})));
        index.add("/p", 0, &env("s1", 8, "permission", json!({"request_id": "r", "tool_name": "PERMTOOL", "input": {}})));
        index.add("/p", 0, &env("s1", 9, "stderr", json!({"text": "STDERRTEXT"})));
        assert_eq!(search(&index, "cargo build").len(), 1, "a tool call is found by its summary");
        assert_eq!(search(&index, "compiled everything").len(), 1, "a short result is kept");
        for hidden in ["SECRETTHOUGHT", "OWNRESULT", "SUBAGENTTEXT", "QUFBQUJBU0U2NERBVEE", "PERMTOOL", "STDERRTEXT"] {
            assert!(search(&index, hidden).is_empty(), "{hidden} must not be indexed");
        }
        assert_eq!(search(&index, "see image").len(), 1);
    }

    #[test]
    fn long_text_is_cut_and_adding_twice_changes_nothing() {
        let (_dir, index) = fixture();
        let long = format!("{}TAILMARKER", "x".repeat(40 * 1024));
        index.add("/p", 0, &user("s1", 1, &long));
        index.add("/p", 0, &user("s1", 1, &long));
        index.add("/p", 0, &user("s1", 1, "different text for the same seq"));
        index.flush();
        assert!(search(&index, "TAILMARKER").is_empty(), "past the cap");
        assert!(search(&index, "different text").is_empty(), "a seq already indexed is not replaced");
        assert_eq!(search(&index, "xxxxxxxxxx").len(), 1);
    }

    #[test]
    fn filters_narrow_by_project_session_and_time() {
        let (_dir, index) = fixture();
        index.add("/a", 0, &user("s1", 1, "shared words here"));
        index.add("/b", 0, &user("s2", 1, "shared words there"));
        let q = |cwd: Option<&str>, sid: Option<&str>, since: Option<u64>| {
            index.flush();
            index.search(&SearchQuery { q: "shared".into(), cwd: cwd.map(Into::into), sid: sid.map(Into::into), since, limit: 10 }).unwrap().len()
        };
        assert_eq!(q(None, None, None), 2);
        assert_eq!(q(Some("/a"), None, None), 1);
        assert_eq!(q(None, Some("s2"), None), 1);
        assert_eq!(q(None, None, Some(5000)), 0);
        assert_eq!(q(None, None, Some(1001)), 2);
    }

    #[test]
    fn catch_up_is_idempotent_restarts_a_replaced_log_and_prunes_gone_sessions() {
        let (_dir, index) = fixture();
        let log = vec![user("s1", 1, "first message"), user("s1", 2, "second message")];
        index.catch_up("s1", "/p", 7, log.clone());
        index.catch_up("s1", "/p", 7, log);
        index.add("/p", 0, &user("s9", 1, "gone session text"));
        assert_eq!(search(&index, "message").len(), 2);

        index.catch_up("s1", "/p", 7, vec![user("s1", 1, "replacement only")]);
        assert_eq!(search(&index, "message").len(), 0, "the old rows of a replaced log are dropped");
        assert_eq!(search(&index, "replacement").len(), 1);

        index.keep_only(["s1".to_owned()].into_iter().collect());
        assert!(search(&index, "gone session").is_empty());
        assert_eq!(search(&index, "replacement").len(), 1);
    }

    #[test]
    fn an_old_or_damaged_index_is_rebuilt_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        {
            let index = Index::open(&path).unwrap();
            index.add("/p", 0, &user("s1", 1, "kept text"));
            index.flush();
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            let wal = dir.path().join("index.sqlite-wal");
            if wal.exists() {
                assert_eq!(mode(&wal), 0o600, "the wal copies the main file's permissions");
            }
        }
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", 99).unwrap();
        }
        let index = Index::open(&path).unwrap();
        assert!(search(&index, "kept text").is_empty(), "another version starts over");
        index.add("/p", 0, &user("s1", 1, "new text"));
        assert_eq!(search(&index, "new text").len(), 1);
        drop(index);

        remove_db(&path);
        std::fs::write(&path, b"this is not a database at all, just bytes").unwrap();
        let index = Index::open(&path).unwrap();
        index.add("/p", 0, &user("s1", 1, "after damage"));
        assert_eq!(search(&index, "after damage").len(), 1);
    }

    #[test]
    fn the_excerpt_is_a_window_with_every_match_marked_whatever_the_case() {
        let text = format!("{} Needle in the middle, and a needle again. {}", "before ".repeat(40), "after ".repeat(80));
        let one = around(&text, &["needle".to_owned()]);
        assert!(one.starts_with('…') && one.ends_with('…'));
        assert_eq!(one.matches(MARK_OPEN).count(), 2, "both spellings are marked: {one}");
        assert_eq!(one.matches(MARK_CLOSE).count(), 2);
        assert!(one.chars().count() < 340, "about 280 characters, not the whole text");
        let korean = around("마이그레이션을 먼저 실행했습니다", &["그레이".to_owned(), "먼저".to_owned()]);
        assert_eq!(korean.matches(MARK_OPEN).count(), 2, "{korean}");
        assert_eq!(around("no match here", &["zzz".to_owned()]), "no match here");
        assert_eq!(around("", &["x".to_owned()]), "");
    }

    #[test]
    fn plan_splits_long_and_short_terms() {
        assert_eq!(plan("  "), None);
        let p = plan("검색 기능을 ab").unwrap();
        assert_eq!(p.fts.as_deref(), Some("\"기능을\""));
        assert_eq!(p.likes, vec!["검색".to_owned(), "ab".to_owned()]);
        let quoted = plan("say \"hi\" there").unwrap();
        assert_eq!(quoted.fts.as_deref(), Some("\"say\" AND \"there\""));
        assert_eq!(quoted.likes, vec!["hi".to_owned()]);
        let inner = plan("a\"bc").unwrap();
        assert_eq!(inner.fts, None);
        assert_eq!(inner.likes, vec!["a".to_owned(), "bc".to_owned()]);
        assert_eq!(plan("\"\"\""), None);
        assert!(plan(&"word ".repeat(40)).unwrap().fts.unwrap().matches(" AND ").count() < 8);
    }
}
