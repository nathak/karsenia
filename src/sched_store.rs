//! Scheduled tasks and what their runs left behind, in `<data>/schedule.sqlite` (0600).
//!
//! This file is primary data (a task is something the person typed), so a damaged one is set aside as `.broken` and the person is
//! told. Tasks are created and changed only by the person through the UI: nothing in this module is reachable from the MCP tools.
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::schedule::Schedule;

const SCHEMA_VERSION: i64 = 2;
pub const MAX_TASKS: usize = 30;
pub const MAX_NAME: usize = 80;
pub const MAX_PROMPT: usize = 4000;
/// Runs kept per task; older ones are dropped.
pub const KEEP_RUNS: i64 = 50;
pub const MAX_RESULT: usize = 64 * 1024;
/// Sites a web task may open.
pub const MAX_DOMAINS: usize = 20;
/// Why a web task made before the site list existed is paused; the page translates it.
pub const NEEDS_DOMAINS: &str = "needs the sites it may open: edit the task and list them";
const MAX_DENIED: usize = 50;
const MAX_DENIED_CHARS: usize = 300;

const SCHEMA: &str = "
-- AUTOINCREMENT: a deleted task's number is never given to a new one, so nothing queued or running can reach a replacement by its old number.
CREATE TABLE tasks(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  prompt TEXT NOT NULL,
  cwd TEXT NOT NULL,
  profile TEXT NOT NULL,
  model TEXT NOT NULL,
  kind TEXT NOT NULL,
  minute_of_day INTEGER NOT NULL DEFAULT 0,
  weekdays INTEGER NOT NULL DEFAULT 127,
  every_min INTEGER NOT NULL DEFAULT 0,
  max_turns INTEGER NOT NULL,
  budget REAL NOT NULL,
  timeout_s INTEGER NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  fails INTEGER NOT NULL DEFAULT 0,
  paused_reason TEXT NOT NULL DEFAULT '',
  domains TEXT NOT NULL DEFAULT '[]',
  last_due INTEGER NOT NULL DEFAULT 0,
  next_due INTEGER NOT NULL DEFAULT 0,
  created INTEGER NOT NULL,
  updated INTEGER NOT NULL
);
CREATE TABLE runs(
  id INTEGER PRIMARY KEY,
  task_id INTEGER NOT NULL,
  task_name TEXT NOT NULL,
  due INTEGER NOT NULL,
  started INTEGER NOT NULL DEFAULT 0,
  finished INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL,
  cost REAL NOT NULL DEFAULT 0,
  turns INTEGER NOT NULL DEFAULT 0,
  result TEXT NOT NULL DEFAULT '',
  denied TEXT NOT NULL DEFAULT '[]',
  note TEXT NOT NULL DEFAULT '',
  def TEXT NOT NULL DEFAULT '',
  seen INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX runs_task ON runs(task_id, id);
CREATE TABLE counters(day INTEGER PRIMARY KEY, runs INTEGER NOT NULL);
";

/// What a task's run may use. The two never mix: a run that can read files and also fetch from the web could carry a file's content out in a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Read, Grep and Glob inside one project folder.
    Folder,
    /// WebFetch and WebSearch only, in an empty folder of the app's own.
    Web,
}

impl Profile {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "folder" => Some(Self::Folder),
            "web" => Some(Self::Web),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Web => "web",
        }
    }

    /// The tools that exist in the run.
    pub fn tools(self) -> &'static str {
        match self {
            Self::Folder => "Read,Grep,Glob",
            Self::Web => "WebFetch,WebSearch",
        }
    }

    /// Belt and braces beside `--tools`: names that must never be usable, whatever a setting says.
    pub fn disallowed(self) -> &'static str {
        match self {
            Self::Folder => "Bash,Write,Edit,NotebookEdit,WebFetch,WebSearch,Task,Agent",
            Self::Web => "Bash,Write,Edit,NotebookEdit,Read,Grep,Glob,Task,Agent,WebFetch(domain:localhost),WebFetch(domain:127.0.0.1),WebFetch(domain:[::1]),WebFetch(domain:0.0.0.0),WebFetch(domain:169.254.169.254),WebFetch(domain:metadata.google.internal)",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskDef {
    pub name: String,
    pub prompt: String,
    /// The project folder (canonical) for a folder task; empty for a web task.
    pub cwd: String,
    pub profile: Profile,
    pub model: String,
    pub schedule: Schedule,
    pub max_turns: u32,
    pub budget_usd: f64,
    pub timeout_s: u32,
    /// The sites a web task may open (normalised, see `normalize_domain`); empty for a folder task.
    pub domains: Vec<String>,
}

/// Last labels that name something on a private network or are reserved: never a site to fetch from. Names that merely resolve to a
/// private address (nip.io and the like) cannot be listed here and are not stopped; the README says so.
const SPECIAL_TLDS: [&str; 11] = ["localhost", "local", "internal", "localdomain", "arpa", "lan", "home", "corp", "intranet", "test", "invalid"];

/// A site name as it is kept: trimmed, lower case, one trailing dot dropped. Only a host name is accepted: no address, port, path,
/// wildcard or user. An address is refused by the last label (a name's last label is letters, an address's is a number).
pub fn normalize_domain(raw: &str) -> Result<String, String> {
    let name = raw.trim().to_ascii_lowercase();
    let name = name.strip_suffix('.').unwrap_or(&name);
    let bad = |why: &str| Err(format!("{}: {why}", clip(raw.trim(), 60)));
    if name.is_empty() {
        return Err("a site name is empty".into());
    }
    if !name.is_ascii() {
        return bad("write an international name in its xn-- form");
    }
    if name.len() > 253 {
        return bad("too long");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') {
        return bad("only a host name, such as example.com (no address, port, path or *)");
    }
    let labels: Vec<&str> = name.split('.').collect();
    if labels.len() < 2 {
        return bad("a name needs a dot, such as example.com");
    }
    if labels.iter().any(|l| l.is_empty() || l.len() > 63 || l.starts_with('-') || l.ends_with('-')) {
        return bad("not a valid host name");
    }
    let tld = labels[labels.len() - 1];
    let is_name = (tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_lowercase())) || (tld.starts_with("xn--") && tld.len() > 4);
    if !is_name {
        return bad("an address is not a site name");
    }
    if SPECIAL_TLDS.contains(&tld) {
        return bad("a name on a private network is not allowed");
    }
    Ok(name.to_owned())
}

/// Normalises a list the person typed: each name, duplicates dropped, order kept.
pub fn normalize_domains(raw: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for item in raw {
        let name = normalize_domain(item)?;
        if !out.contains(&name) {
            out.push(name);
        }
    }
    Ok(out)
}

impl TaskDef {
    /// What is allowed outright in a run (no question, nobody is there to answer). The file tools are not: an allow rule for `Read`
    /// covers every path, while without one Claude Code reads inside the project folder only and refuses a path (or a link) that leads
    /// outside it. A web run may open only the listed sites; there is never a bare `WebFetch` rule. Search runs on the provider's side
    /// and reaches nothing local.
    pub fn allowed_tools(&self) -> Option<String> {
        match self.profile {
            Profile::Folder => None,
            Profile::Web => {
                let mut rules = vec!["WebSearch".to_owned()];
                rules.extend(self.domains.iter().map(|d| format!("WebFetch(domain:{d})")));
                Some(rules.join(","))
            }
        }
    }

    /// The checks that need no file system. The folder is checked by the caller (`is_safe_cwd`, not inside the data folder).
    pub fn check(&self) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME {
            return Err(format!("a name is 1 to {MAX_NAME} characters"));
        }
        let prompt = self.prompt.trim();
        if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT {
            return Err(format!("the instruction is 1 to {MAX_PROMPT} characters"));
        }
        if self.model.is_empty() || self.model.starts_with('-') || self.model.len() > 100 || !self.model.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/[]".contains(c)) {
            return Err("the model name is not valid".into());
        }
        if !(1..=50).contains(&self.max_turns) {
            return Err("turns are 1 to 50".into());
        }
        if !(0.01..=5.0).contains(&self.budget_usd) {
            return Err("the budget is between $0.01 and $5".into());
        }
        if !(30..=1800).contains(&self.timeout_s) {
            return Err("the time limit is 30 seconds to 30 minutes".into());
        }
        match self.profile {
            Profile::Folder if self.cwd.is_empty() => return Err("a folder task needs a project folder".into()),
            Profile::Web if !self.cwd.is_empty() => return Err("a web task has no project folder".into()),
            _ => {}
        }
        self.check_sites()?;
        self.schedule.check()
    }

    /// The site list against the profile. Also checked again where a run starts or a task is switched on, so a file edited by hand
    /// cannot give a web run a list that was never accepted (or none).
    pub fn check_sites(&self) -> Result<(), String> {
        match self.profile {
            Profile::Web if self.domains.is_empty() => return Err("a web task needs the sites it may open".into()),
            Profile::Folder if !self.domains.is_empty() => return Err("a folder task opens no sites".into()),
            _ => {}
        }
        if self.domains.len() > MAX_DOMAINS {
            return Err(format!("at most {MAX_DOMAINS} sites"));
        }
        for (i, domain) in self.domains.iter().enumerate() {
            if normalize_domain(domain)? != *domain || self.domains[..i].contains(domain) {
                return Err(format!("{domain}: the site list is not in its normal form"));
            }
        }
        Ok(())
    }

    /// What a run is shown to have used, kept with each run so a changed definition cannot hide.
    pub fn snapshot(&self) -> String {
        json!({"name": self.name, "prompt": self.prompt, "cwd": self.cwd, "profile": self.profile.name(), "tools": self.profile.tools(), "domains": self.domains, "model": self.model, "max_turns": self.max_turns, "budget_usd": self.budget_usd, "timeout_s": self.timeout_s}).to_string()
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: i64,
    pub def: TaskDef,
    pub enabled: bool,
    pub fails: u32,
    pub paused_reason: String,
    pub last_due: i64,
    pub next_due: i64,
    pub created: u64,
    pub updated: u64,
}

impl Task {
    pub fn to_json(&self) -> Value {
        let (kind, minute_of_day, weekdays, every_min) = match self.def.schedule {
            Schedule::Daily { minute_of_day, weekdays } => ("daily", minute_of_day, weekdays, 0),
            Schedule::Every { minutes } => ("every", 0, 127, minutes),
        };
        json!({
            "id": self.id, "name": self.def.name, "prompt": self.def.prompt, "cwd": self.def.cwd, "profile": self.def.profile.name(),
            "model": self.def.model, "kind": kind, "minute_of_day": minute_of_day, "weekdays": weekdays, "every_min": every_min,
            "max_turns": self.def.max_turns, "budget_usd": self.def.budget_usd, "timeout_s": self.def.timeout_s,
            "enabled": self.enabled, "fails": self.fails, "paused_reason": self.paused_reason,
            "last_due": self.last_due, "next_due": self.next_due, "tools": self.def.profile.tools(), "domains": self.def.domains,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Run {
    pub id: i64,
    pub task_id: i64,
    pub task_name: String,
    pub due: i64,
    pub started: i64,
    pub finished: i64,
    pub status: String,
    pub cost: f64,
    pub turns: u32,
    pub result: String,
    pub denied: Vec<String>,
    pub note: String,
    pub def: String,
    pub seen: bool,
}

impl Run {
    pub fn to_json(&self, with_result: bool) -> Value {
        json!({
            "id": self.id, "task_id": self.task_id, "task_name": self.task_name, "due": self.due, "started": self.started, "finished": self.finished,
            "status": self.status, "cost": self.cost, "turns": self.turns, "denied": self.denied, "note": self.note, "seen": self.seen,
            "result": if with_result { Value::String(self.result.clone()) } else { Value::Null },
            "def": if with_result { serde_json::from_str(&self.def).unwrap_or(Value::Null) } else { Value::Null },
        })
    }
}

/// What a finished run reports.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub status: String,
    pub cost: f64,
    pub turns: u32,
    pub result: String,
    pub denied: Vec<String>,
    pub note: String,
}

pub fn clip(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((at, _)) => text[..at].to_owned(),
        None => text.to_owned(),
    }
}

pub struct SchedStore {
    conn: Mutex<Connection>,
    pub path: PathBuf,
}

const TASK_COLS: &str = "id, name, prompt, cwd, profile, model, kind, minute_of_day, weekdays, every_min, max_turns, budget, timeout_s, enabled, fails, paused_reason, last_due, next_due, created, updated, domains";
const RUN_COLS: &str = "id, task_id, task_name, due, started, finished, status, cost, turns, result, denied, note, def, seen";

fn task_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let kind: String = r.get(6)?;
    let schedule = if kind == "every" { Schedule::Every { minutes: r.get::<_, i64>(9)? as u32 } } else { Schedule::Daily { minute_of_day: r.get::<_, i64>(7)? as u16, weekdays: r.get::<_, i64>(8)? as u8 } };
    let profile: String = r.get(4)?;
    Ok(Task {
        id: r.get(0)?,
        def: TaskDef {
            name: r.get(1)?,
            prompt: r.get(2)?,
            cwd: r.get(3)?,
            // A profile this version does not know is the narrower one: a task never runs with more than it was made with.
            profile: Profile::parse(&profile).unwrap_or(Profile::Web),
            model: r.get(5)?,
            schedule,
            max_turns: r.get::<_, i64>(10)? as u32,
            budget_usd: r.get(11)?,
            timeout_s: r.get::<_, i64>(12)? as u32,
            domains: serde_json::from_str(&r.get::<_, String>(20)?).unwrap_or_default(),
        },
        enabled: r.get::<_, i64>(13)? != 0,
        fails: r.get::<_, i64>(14)? as u32,
        paused_reason: r.get(15)?,
        last_due: r.get(16)?,
        next_due: r.get(17)?,
        created: r.get::<_, i64>(18)? as u64,
        updated: r.get::<_, i64>(19)? as u64,
    })
}

fn run_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    let denied: String = r.get(10)?;
    Ok(Run {
        id: r.get(0)?,
        task_id: r.get(1)?,
        task_name: r.get(2)?,
        due: r.get(3)?,
        started: r.get(4)?,
        finished: r.get(5)?,
        status: r.get(6)?,
        cost: r.get(7)?,
        turns: r.get::<_, i64>(8)? as u32,
        result: r.get(9)?,
        denied: serde_json::from_str(&denied).unwrap_or_default(),
        note: r.get(11)?,
        def: r.get(12)?,
        seen: r.get::<_, i64>(13)? != 0,
    })
}

fn domains_json(def: &TaskDef) -> String {
    serde_json::to_string(&def.domains).unwrap_or_else(|_| "[]".into())
}

fn schedule_cols(s: &Schedule) -> (&'static str, i64, i64, i64) {
    match *s {
        Schedule::Daily { minute_of_day, weekdays } => ("daily", i64::from(minute_of_day), i64::from(weekdays), 0),
        Schedule::Every { minutes } => ("every", 0, 127, i64::from(minutes)),
    }
}

/// Version 1 had no site list: a web task then could open any site. Such a task is switched off, with a reason, until the person lists
/// the sites; nothing is ever widened. A copy of the old file is kept first (an older app cannot read the new one).
fn migrate_v1(conn: &Connection, path: &Path) -> Result<()> {
    let result = migrate_v1_steps(conn, path);
    if result.is_err() {
        // An open transaction is dropped with the connection anyway; this only makes it explicit.
        let _ = conn.execute_batch("ROLLBACK");
    }
    result.map_err(|error| anyhow::Error::new(UpgradeFailed(format!("{error:#}"))))
}

/// Why a file could not be upgraded. Unlike a damaged file it is not set aside: it is left exactly as it was, and scheduled tasks stay off.
#[derive(Debug)]
struct UpgradeFailed(String);

impl std::fmt::Display for UpgradeFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the schedule file could not be upgraded and was left as it was ({})", self.0)
    }
}

impl std::error::Error for UpgradeFailed {}

fn migrate_v1_steps(conn: &Connection, path: &Path) -> Result<()> {
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let backup = path.with_extension("sqlite.v1");
    if !backup.exists() {
        std::fs::copy(path, &backup).context("could not keep a copy of the schedule file before upgrading it")?;
    }
    conn.execute_batch("BEGIN; ALTER TABLE tasks ADD COLUMN domains TEXT NOT NULL DEFAULT '[]';")?;
    conn.execute("UPDATE tasks SET enabled = 0, paused_reason = ?1 WHERE profile <> 'folder'", params![NEEDS_DOMAINS])?;
    conn.execute_batch("PRAGMA user_version = 2; COMMIT;")?;
    Ok(())
}

impl SchedStore {
    /// Opens the store; a damaged file is kept as `.broken`, a new one started, and the caller told.
    pub fn open(path: &Path) -> Result<(Self, Option<String>)> {
        let mut notice = None;
        let conn = match Self::try_open(path) {
            Ok(conn) => conn,
            Err(error) if error.downcast_ref::<UpgradeFailed>().is_some() => return Err(error),
            Err(error) => {
                let broken = path.with_extension("sqlite.broken");
                let _ = std::fs::rename(path, &broken);
                for suffix in ["-wal", "-shm"] {
                    let mut name = path.as_os_str().to_owned();
                    name.push(suffix);
                    let _ = std::fs::remove_file(PathBuf::from(name));
                }
                notice = Some(format!("the schedule file could not be read ({error:#}); it was kept as {}", broken.display()));
                Self::try_open(path)?
            }
        };
        let store = Self { conn: Mutex::new(conn), path: path.to_owned() };
        // A run that was going on when the host ended did not finish.
        store.lock().execute("UPDATE runs SET status = 'error', finished = ?1, note = 'the app ended while this was running' WHERE status = 'running'", params![crate::now_ms() as i64]).ok();
        Ok((store, notice))
    }

    fn try_open(path: &Path) -> Result<Connection> {
        let fresh = !path.exists() || std::fs::metadata(path).map(|m| m.len() == 0).unwrap_or(true);
        if !path.exists() {
            crate::private::write(path, "")?;
        }
        let conn = Connection::open(path).context("could not open the schedule file")?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        if fresh {
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            return Ok(conn);
        }
        let mut version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 1 {
            migrate_v1(&conn, path)?;
            version = SCHEMA_VERSION;
        }
        if version != SCHEMA_VERSION {
            anyhow::bail!("schedule file version {version} is not understood");
        }
        let _: i64 = conn.query_row("SELECT count(*) FROM tasks", [], |r| r.get(0))?;
        let _: i64 = conn.query_row("SELECT count(*) FROM runs", [], |r| r.get(0))?;
        Ok(conn)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn create_task(&self, def: &TaskDef, next_due: i64, now: u64) -> Result<i64, String> {
        def.check()?;
        let conn = self.lock();
        let count: i64 = conn.query_row("SELECT count(*) FROM tasks", [], |r| r.get(0)).unwrap_or(0);
        if count as usize >= MAX_TASKS {
            return Err(format!("at most {MAX_TASKS} tasks"));
        }
        let (kind, minute, weekdays, every) = schedule_cols(&def.schedule);
        conn.execute(
            "INSERT INTO tasks(name, prompt, cwd, profile, model, kind, minute_of_day, weekdays, every_min, max_turns, budget, timeout_s, enabled, next_due, created, updated, domains) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,1,?13,?14,?14,?15)",
            params![def.name.trim(), def.prompt.trim(), def.cwd, def.profile.name(), def.model, kind, minute, weekdays, every, def.max_turns, def.budget_usd, def.timeout_s, next_due, now as i64, domains_json(def)],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }

    /// Changes a task's definition. A changed task starts fresh: its failure count is cleared and it is switched on only if it was.
    pub fn update_task(&self, id: i64, def: &TaskDef, next_due: i64, now: u64) -> Result<(), String> {
        def.check()?;
        let (kind, minute, weekdays, every) = schedule_cols(&def.schedule);
        let n = self
            .lock()
            .execute(
                "UPDATE tasks SET name=?2, prompt=?3, cwd=?4, profile=?5, model=?6, kind=?7, minute_of_day=?8, weekdays=?9, every_min=?10, max_turns=?11, budget=?12, timeout_s=?13, next_due=?14, updated=?15, domains=?16, fails=0 WHERE id=?1",
                params![id, def.name.trim(), def.prompt.trim(), def.cwd, def.profile.name(), def.model, kind, minute, weekdays, every, def.max_turns, def.budget_usd, def.timeout_s, next_due, now as i64, domains_json(def)],
            )
            .map_err(|e| e.to_string())?;
        if n == 0 { Err("no such task".into()) } else { Ok(()) }
    }

    pub fn delete_task(&self, id: i64) -> bool {
        let conn = self.lock();
        conn.execute("DELETE FROM runs WHERE task_id = ?1", params![id]).ok();
        conn.execute("DELETE FROM tasks WHERE id = ?1", params![id]).map(|n| n > 0).unwrap_or(false)
    }

    /// Switches a task on or off. Switching on clears the failure count and the pause reason and sets the next time.
    pub fn set_enabled(&self, id: i64, enabled: bool, next_due: i64) -> bool {
        let conn = self.lock();
        let n = if enabled {
            conn.execute("UPDATE tasks SET enabled = 1, fails = 0, paused_reason = '', next_due = ?2 WHERE id = ?1", params![id, next_due])
        } else {
            conn.execute("UPDATE tasks SET enabled = 0 WHERE id = ?1", params![id])
        };
        n.map(|n| n > 0).unwrap_or(false)
    }

    pub fn task(&self, id: i64) -> Option<Task> {
        self.lock().query_row(&format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1"), params![id], task_from).optional().unwrap_or(None)
    }

    pub fn tasks(&self) -> Vec<Task> {
        let conn = self.lock();
        let Ok(mut statement) = conn.prepare(&format!("SELECT {TASK_COLS} FROM tasks ORDER BY id")) else { return Vec::new() };
        statement.query_map([], task_from).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    /// Sets the next time of every task that is on (used when the scheduler is switched on, so nothing that came due meanwhile runs).
    pub fn set_next_due(&self, id: i64, last_due: i64, next_due: i64) {
        self.lock().execute("UPDATE tasks SET last_due = ?2, next_due = ?3 WHERE id = ?1", params![id, last_due, next_due]).ok();
    }

    pub fn pause(&self, id: i64, reason: &str) {
        self.lock().execute("UPDATE tasks SET enabled = 0, paused_reason = ?2 WHERE id = ?1", params![id, reason]).ok();
    }

    /// A run that has no process behind it: skipped, missed, or stopped by a limit.
    pub fn add_note_run(&self, task: &Task, due: i64, status: &str, note: &str, now: i64) -> i64 {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO runs(task_id, task_name, due, started, finished, status, note, def) VALUES (?1,?2,?3,?4,?4,?5,?6,?7)",
            params![task.id, task.def.name, due, now, status, note, task.def.snapshot()],
        )
        .ok();
        let id = conn.last_insert_rowid();
        Self::trim(&conn, task.id);
        id
    }

    pub fn start_run(&self, task: &Task, due: i64, now: i64) -> i64 {
        let conn = self.lock();
        conn.execute("INSERT INTO runs(task_id, task_name, due, started, status, def) VALUES (?1,?2,?3,?4,'running',?5)", params![task.id, task.def.name, due, now, task.def.snapshot()]).ok();
        let id = conn.last_insert_rowid();
        Self::trim(&conn, task.id);
        id
    }

    fn trim(conn: &Connection, task_id: i64) {
        conn.execute("DELETE FROM runs WHERE task_id = ?1 AND id NOT IN (SELECT id FROM runs WHERE task_id = ?1 ORDER BY id DESC LIMIT ?2)", params![task_id, KEEP_RUNS]).ok();
    }

    /// Records how a run ended and updates the task's failure count. Returns the number of failures in a row now.
    pub fn finish_run(&self, run_id: i64, task_id: i64, outcome: &Outcome, now: i64) -> u32 {
        let conn = self.lock();
        let denied: Vec<String> = outcome.denied.iter().take(MAX_DENIED).map(|d| clip(d, MAX_DENIED_CHARS)).collect();
        conn.execute(
            "UPDATE runs SET status=?2, finished=?3, cost=?4, turns=?5, result=?6, denied=?7, note=?8 WHERE id=?1",
            params![run_id, outcome.status, now, outcome.cost, outcome.turns, clip(&outcome.result, MAX_RESULT), serde_json::to_string(&denied).unwrap_or_else(|_| "[]".into()), clip(&outcome.note, 2000)],
        )
        .ok();
        // A run the person stopped is neither a success nor a failure.
        match outcome.status.as_str() {
            "ok" => conn.execute("UPDATE tasks SET fails = 0 WHERE id = ?1", params![task_id]).ok(),
            "stopped" => None,
            _ => conn.execute("UPDATE tasks SET fails = fails + 1 WHERE id = ?1", params![task_id]).ok(),
        };
        conn.query_row("SELECT fails FROM tasks WHERE id = ?1", params![task_id], |r| r.get::<_, i64>(0)).map(|n| n as u32).unwrap_or(0)
    }

    pub fn run(&self, id: i64) -> Option<Run> {
        self.lock().query_row(&format!("SELECT {RUN_COLS} FROM runs WHERE id = ?1"), params![id], run_from).optional().unwrap_or(None)
    }

    /// The newest runs of every task (or one), newest first.
    pub fn runs(&self, task_id: Option<i64>, limit: usize) -> Vec<Run> {
        let conn = self.lock();
        let sql = format!("SELECT {RUN_COLS} FROM runs WHERE (?1 IS NULL OR task_id = ?1) ORDER BY id DESC LIMIT ?2");
        let Ok(mut statement) = conn.prepare(&sql) else { return Vec::new() };
        statement.query_map(params![task_id, limit as i64], run_from).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    pub fn running_tasks(&self) -> Vec<i64> {
        let conn = self.lock();
        let Ok(mut statement) = conn.prepare("SELECT DISTINCT task_id FROM runs WHERE status = 'running'") else { return Vec::new() };
        statement.query_map([], |r| r.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    pub fn mark_seen(&self, run_id: Option<i64>) {
        let conn = self.lock();
        match run_id {
            Some(id) => conn.execute("UPDATE runs SET seen = 1 WHERE id = ?1", params![id]),
            None => conn.execute("UPDATE runs SET seen = 1", []),
        }
        .ok();
    }

    /// Runs the person has not looked at that ended in a way worth looking at.
    pub fn unseen(&self) -> i64 {
        self.lock().query_row("SELECT count(*) FROM runs WHERE seen = 0 AND status NOT IN ('running')", [], |r| r.get(0)).unwrap_or(0)
    }

    pub fn runs_on(&self, day: u64) -> u32 {
        self.lock().query_row("SELECT runs FROM counters WHERE day = ?1", params![day as i64], |r| r.get::<_, i64>(0)).optional().ok().flatten().unwrap_or(0) as u32
    }

    pub fn count_run(&self, day: u64) {
        let conn = self.lock();
        conn.execute("INSERT INTO counters(day, runs) VALUES (?1, 1) ON CONFLICT(day) DO UPDATE SET runs = runs + 1", params![day as i64]).ok();
        conn.execute("DELETE FROM counters WHERE day < ?1", params![day as i64 - 30]).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn def(name: &str) -> TaskDef {
        TaskDef { name: name.into(), prompt: "summarise the README".into(), cwd: "/tmp".into(), profile: Profile::Folder, model: "sonnet".into(), schedule: Schedule::Daily { minute_of_day: 540, weekdays: 127 }, max_turns: 20, budget_usd: 0.5, timeout_s: 600, domains: Vec::new() }
    }

    #[test]
    fn the_checks_refuse_what_a_task_must_not_be() {
        assert!(def("a").check().is_ok());
        assert!(TaskDef { name: " ".into(), ..def("a") }.check().is_err());
        assert!(TaskDef { prompt: String::new(), ..def("a") }.check().is_err());
        assert!(TaskDef { model: "x y".into(), ..def("a") }.check().is_err());
        assert!(TaskDef { model: "--dangerously-skip-permissions".into(), ..def("a") }.check().is_err(), "a model name is never a flag");
        assert!(TaskDef { max_turns: 0, ..def("a") }.check().is_err());
        assert!(TaskDef { budget_usd: 50.0, ..def("a") }.check().is_err());
        assert!(TaskDef { timeout_s: 5, ..def("a") }.check().is_err());
        assert!(TaskDef { cwd: String::new(), ..def("a") }.check().is_err(), "a folder task needs a folder");
        assert!(TaskDef { profile: Profile::Web, ..def("a") }.check().is_err(), "a web task has no folder");
        assert!(TaskDef { profile: Profile::Web, cwd: String::new(), domains: vec!["example.com".into()], ..def("a") }.check().is_ok());
        assert!(TaskDef { profile: Profile::Web, cwd: String::new(), ..def("a") }.check().is_err(), "a web task needs sites");
        assert!(TaskDef { schedule: Schedule::Every { minutes: 5 }, ..def("a") }.check().is_err());
    }

    #[test]
    fn a_task_and_its_runs_survive_a_restart_and_old_runs_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schedule.sqlite");
        let (store, notice) = SchedStore::open(&path).unwrap();
        assert!(notice.is_none());
        let id = store.create_task(&def("daily"), 1000, 5).unwrap();
        let task = store.task(id).unwrap();
        assert_eq!((task.def.name.as_str(), task.next_due, task.enabled, task.fails), ("daily", 1000, true, 0));
        for i in 0..(KEEP_RUNS + 5) {
            store.add_note_run(&task, i, "missed", "", i);
        }
        assert_eq!(store.runs(Some(id), 500).len() as i64, KEEP_RUNS);
        drop(store);
        let (store, _) = SchedStore::open(&path).unwrap();
        assert_eq!(store.tasks().len(), 1);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn failures_count_in_a_row_and_a_success_clears_them() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = SchedStore::open(&dir.path().join("s.sqlite")).unwrap();
        let id = store.create_task(&def("t"), 0, 0).unwrap();
        let task = store.task(id).unwrap();
        let bad = Outcome { status: "error".into(), ..Default::default() };
        let good = Outcome { status: "ok".into(), result: "fine".into(), ..Default::default() };
        let r = store.start_run(&task, 0, 1);
        assert_eq!(store.running_tasks(), vec![id]);
        assert_eq!(store.finish_run(r, id, &bad, 2), 1);
        let r = store.start_run(&task, 0, 3);
        assert_eq!(store.finish_run(r, id, &bad, 4), 2);
        let r = store.start_run(&task, 0, 5);
        assert_eq!(store.finish_run(r, id, &good, 6), 0, "a success clears the count");
        assert!(store.running_tasks().is_empty());
        assert_eq!(store.run(r).unwrap().result, "fine");
        // Skipped and missed runs are not failures.
        store.add_note_run(&task, 0, "skipped", "", 7);
        assert_eq!(store.task(id).unwrap().fails, 0);
        // Switching a paused task on clears the count and the reason.
        store.pause(id, "three failures");
        assert!(!store.task(id).unwrap().enabled);
        assert!(store.set_enabled(id, true, 99));
        let task = store.task(id).unwrap();
        assert_eq!((task.enabled, task.fails, task.paused_reason.as_str(), task.next_due), (true, 0, "", 99));
    }

    #[test]
    fn a_run_left_running_by_a_crash_is_closed_and_the_day_counter_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sqlite");
        let (store, _) = SchedStore::open(&path).unwrap();
        let id = store.create_task(&def("t"), 0, 0).unwrap();
        let r = store.start_run(&store.task(id).unwrap(), 0, 1);
        store.count_run(20_000);
        store.count_run(20_000);
        drop(store);
        let (store, _) = SchedStore::open(&path).unwrap();
        let run = store.run(r).unwrap();
        assert_eq!(run.status, "error");
        assert!(run.note.contains("ended while"));
        assert_eq!(store.runs_on(20_000), 2);
        assert_eq!(store.runs_on(20_001), 0);
    }

    #[test]
    fn a_deleted_tasks_number_is_not_given_to_the_next_task() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = SchedStore::open(&dir.path().join("s.sqlite")).unwrap();
        let first = store.create_task(&def("a"), 0, 0).unwrap();
        assert!(store.delete_task(first));
        let second = store.create_task(&def("b"), 0, 0).unwrap();
        assert!(second > first, "{second} must not reuse {first}");
    }

    #[test]
    fn a_damaged_file_is_set_aside_and_the_task_cap_holds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sqlite");
        std::fs::write(&path, "this is not a database at all, not even close, padding padding padding").unwrap();
        let (store, notice) = SchedStore::open(&path).unwrap();
        assert!(notice.unwrap().contains(".broken"));
        assert!(dir.path().join("s.sqlite.broken").exists());
        for i in 0..MAX_TASKS {
            store.create_task(&def(&format!("t{i}")), 0, 0).unwrap();
        }
        assert!(store.create_task(&def("one more"), 0, 0).is_err());
    }

    #[test]
    fn long_results_are_clipped_and_an_unknown_profile_is_the_narrow_one() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = SchedStore::open(&dir.path().join("s.sqlite")).unwrap();
        let id = store.create_task(&def("t"), 0, 0).unwrap();
        let task = store.task(id).unwrap();
        let r = store.start_run(&task, 0, 1);
        let big = Outcome { status: "ok".into(), result: "가".repeat(MAX_RESULT + 10), denied: (0..80).map(|i| format!("Bash {i}")).collect(), ..Default::default() };
        store.finish_run(r, id, &big, 2);
        let run = store.run(r).unwrap();
        assert_eq!(run.result.chars().count(), MAX_RESULT);
        assert_eq!(run.denied.len(), MAX_DENIED);
        store.lock().execute("UPDATE tasks SET profile = 'shell'", []).unwrap();
        assert_eq!(store.task(id).unwrap().def.profile, Profile::Web);
        assert!(store.task(id).unwrap().def.check().is_err(), "and with no sites listed it cannot run");
    }

    #[test]
    fn a_site_name_is_a_plain_host_name_and_nothing_else() {
        for (given, kept) in [("example.com", "example.com"), ("Example.COM.", "example.com"), (" sub.example.co.uk ", "sub.example.co.uk"), ("xn--bcher-kva.example", "xn--bcher-kva.example")] {
            assert_eq!(normalize_domain(given).as_deref(), Ok(kept), "{given}");
        }
        let long_label = format!("{}.com", "a".repeat(64));
        let too_long = format!("{}.com", ["abcdefghi"; 30].join("."));
        for bad in [
            "", " ", "localhost", "a.localhost", "printer.local", "metadata.google.internal", "x.home.arpa", "1.0.0.127.in-addr.arpa", "intranet", "nas.lan", "x.test", "x.invalid",
            "1.2.3.4", "127.1", "0x7f.1", "0x7f000001", "2130706433", "[::1]", "::1", "example.com:8080", "https://example.com", "example.com/p", "*.example.com", "u@example.com",
            "bücher.de", "a..b.com", "-a.com", "a-.com", "a.-com", "a b.com", &long_label, &too_long, "a.c", "a.1com", "example.com..",
        ] {
            assert!(normalize_domain(bad).is_err(), "{bad:?} must be refused");
        }
        let list = vec!["B.example.com".to_owned(), "a.example.com".to_owned(), "b.example.com.".to_owned()];
        assert_eq!(normalize_domains(&list).unwrap(), vec!["b.example.com", "a.example.com"], "normalised, duplicates dropped, order kept");
    }

    #[test]
    fn the_check_holds_the_site_list_to_the_profile() {
        let web = |domains: &[&str]| TaskDef { profile: Profile::Web, cwd: String::new(), domains: domains.iter().map(|d| (*d).to_owned()).collect(), ..def("w") };
        assert!(web(&["example.com"]).check().is_ok());
        assert!(web(&[]).check().is_err(), "a web task needs sites");
        assert!(web(&["Example.com"]).check().is_err(), "a list that is not normalised is refused, not fixed");
        assert!(web(&["example.com", "example.com"]).check().is_err());
        assert!(web(&["127.0.0.1"]).check().is_err());
        let many: Vec<String> = (0..=MAX_DOMAINS).map(|i| format!("s{i}.example.com")).collect();
        assert!(TaskDef { domains: many, ..web(&["example.com"]) }.check().is_err());
        assert!(TaskDef { domains: vec!["example.com".into()], ..def("f") }.check().is_err(), "a folder task opens no sites");
    }

    #[test]
    fn what_is_allowed_outright_is_the_search_and_the_listed_sites_never_a_bare_fetch() {
        let web = TaskDef { profile: Profile::Web, cwd: String::new(), domains: vec!["a.example.com".into(), "b.example.org".into()], ..def("w") };
        assert_eq!(web.allowed_tools().as_deref(), Some("WebSearch,WebFetch(domain:a.example.com),WebFetch(domain:b.example.org)"));
        assert_eq!(def("f").allowed_tools(), None);
    }

    #[test]
    fn sites_are_kept_and_shown_with_the_task_and_with_each_run() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = SchedStore::open(&dir.path().join("s.sqlite")).unwrap();
        let web = TaskDef { profile: Profile::Web, cwd: String::new(), domains: vec!["a.example.com".into()], ..def("w") };
        let id = store.create_task(&web, 0, 0).unwrap();
        assert_eq!(store.task(id).unwrap().def.domains, vec!["a.example.com"]);
        assert_eq!(store.task(id).unwrap().to_json()["domains"], json!(["a.example.com"]));
        store.update_task(id, &TaskDef { domains: vec!["b.example.com".into(), "c.example.com".into()], ..web.clone() }, 0, 1).unwrap();
        assert_eq!(store.task(id).unwrap().def.domains, vec!["b.example.com", "c.example.com"]);
        let run = store.start_run(&store.task(id).unwrap(), 0, 2);
        let shown: Value = serde_json::from_str(&store.run(run).unwrap().def).unwrap();
        assert_eq!(shown["domains"], json!(["b.example.com", "c.example.com"]), "the run keeps the sites it was allowed");
    }

    /// A schedule file of the first version: tasks had no site list, and a web task could open any site.
    fn make_v1(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch("ALTER TABLE tasks DROP COLUMN domains;").unwrap();
        for (name, profile, cwd) in [("files", "folder", "/tmp"), ("news", "web", ""), ("odd", "shell", "")] {
            conn.execute(
                "INSERT INTO tasks(name, prompt, cwd, profile, model, kind, every_min, max_turns, budget, timeout_s, enabled, created, updated) VALUES (?1,'p',?2,?3,'sonnet','every',30,20,0.5,600,1,1,1)",
                params![name, cwd, profile],
            )
            .unwrap();
        }
        conn.pragma_update(None, "user_version", 1).unwrap();
    }

    #[test]
    fn a_first_version_file_is_upgraded_a_web_task_is_switched_off_until_it_lists_sites_and_a_copy_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sqlite");
        make_v1(&path);
        let (store, notice) = SchedStore::open(&path).unwrap();
        assert!(notice.is_none(), "{notice:?}");
        let by_name = |name: &str| store.tasks().into_iter().find(|t| t.def.name == name).unwrap();
        let files = by_name("files");
        assert!(files.enabled && files.paused_reason.is_empty(), "a folder task is left as it was");
        for name in ["news", "odd"] {
            let task = by_name(name);
            assert!(!task.enabled, "{name}");
            assert_eq!(task.paused_reason, NEEDS_DOMAINS);
            assert!(task.def.domains.is_empty() && task.def.check().is_err(), "{name} cannot run without sites");
        }
        assert!(dir.path().join("s.sqlite.v1").exists(), "the file as it was is kept");
        let version: i64 = store.lock().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        // The way back on is to list sites.
        let news = by_name("news");
        store.update_task(news.id, &TaskDef { domains: vec!["example.com".into()], profile: Profile::Web, cwd: String::new(), ..news.def.clone() }, 0, 2).unwrap();
        assert_eq!(by_name("news").def.domains, vec!["example.com"]);
        drop(store);
        let (again, notice) = SchedStore::open(&path).unwrap();
        assert!(notice.is_none() && again.tasks().len() == 3, "upgrading is done once");
    }

    #[test]
    fn an_upgrade_that_fails_leaves_the_file_where_it_was_and_loses_no_task() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sqlite");
        make_v1(&path);
        // The column is already there, so the upgrade's first step fails.
        Connection::open(&path).unwrap().execute_batch("ALTER TABLE tasks ADD COLUMN domains TEXT NOT NULL DEFAULT '[]';").unwrap();
        let error = SchedStore::open(&path).err().expect("open refuses").to_string();
        assert!(error.contains("left as it was"), "{error}");
        assert!(path.exists() && !dir.path().join("s.sqlite.broken").exists(), "the file is not set aside");
        let conn = Connection::open(&path).unwrap();
        let (tasks, version): (i64, i64) = (conn.query_row("SELECT count(*) FROM tasks", [], |r| r.get(0)).unwrap(), conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap());
        assert_eq!((tasks, version), (3, 1), "every task is still there, in the old version");
        let enabled: i64 = conn.query_row("SELECT count(*) FROM tasks WHERE enabled = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(enabled, 3, "nothing was half-changed");
    }

    #[test]
    fn a_file_of_a_newer_version_is_still_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sqlite");
        drop(SchedStore::open(&path).unwrap());
        Connection::open(&path).unwrap().pragma_update(None, "user_version", 3).unwrap();
        let (_, notice) = SchedStore::open(&path).unwrap();
        assert!(notice.unwrap().contains("not understood"));
    }
}
