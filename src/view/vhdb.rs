//! What the VHDB page shows of the VHDB plug-in, read from the files the plug-in keeps in `<git common dir>/vhdb`: the switches in
//! `config.json` and the decisions written to `log/vhdb-*.jsonl` (events of `kind: "decision"`). Read only. `remote.json` holds the
//! upload token and is never opened. Whatever the files say is untrusted text: it is cut, stripped of control characters and drawn
//! as plain text. Searching the history (`file_history`, `query_history`) needs the plug-in's own engine and is not done here.
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::git::{find_git, list_dir, open_regular, read_limited};

/// Where the plug-in keeps its files for the project of `cwd` (the folder may not exist).
pub fn vhdb_dir(cwd: &Path) -> Option<PathBuf> {
    Some(find_git(cwd)?.common.join("vhdb"))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Switches {
    pub record: bool,
    pub inject: bool,
    pub guard: bool,
    pub upload: bool,
    pub budget_tokens: u32,
    pub deadline_ms: u32,
}

impl Default for Switches {
    /// What the plug-in uses while there is no `config.json`: everything off.
    fn default() -> Self {
        Switches { record: false, inject: false, guard: false, upload: false, budget_tokens: 1200, deadline_ms: 250 }
    }
}

/// The plug-in's own rules: a switch is on only when it is exactly `true`; a number outside its range is the default.
pub fn parse_switches(text: &str) -> Switches {
    let raw: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let on = |k: &str| raw[k] == Value::Bool(true);
    let num = |k: &str, lo: f64, hi: f64, d: u32| match raw[k].as_f64() {
        Some(v) if v >= lo && v <= hi => v.floor() as u32,
        _ => d,
    };
    let d = Switches::default();
    Switches { record: on("record"), inject: on("inject"), guard: on("guard"), upload: on("upload"), budget_tokens: num("budgetTokens", 200.0, 2500.0, d.budget_tokens), deadline_ms: num("deadlineMs", 50.0, 5000.0, d.deadline_ms) }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub id: String,
    pub topic: String,
    pub chosen: String,
    pub reason: String,
    pub files: Vec<String>,
    /// Milliseconds since the epoch.
    pub at: u64,
    /// No later decision replaces this one.
    pub active: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    /// The plug-in's folder exists for this project.
    pub found: bool,
    pub switches: Switches,
    pub log_files: usize,
    /// The newest change of a log file (ms since the epoch), 0 for none.
    pub last_at: u64,
    /// Newest first.
    pub decisions: Vec<Decision>,
}

const MAX_LOG_FILES: usize = 20;
const TAIL_BYTES: u64 = 256 * 1024;
const MAX_DECISIONS: usize = 30;
const MAX_FILES_PER_DECISION: usize = 5;

fn clean(v: &Value, cap: usize) -> String {
    v.as_str().unwrap_or_default().chars().filter(|c| !c.is_control() || *c == ' ').take(cap).collect::<String>().trim().to_owned()
}

/// The last `TAIL_BYTES` of a file as text, from the first whole line.
fn tail(path: &Path) -> Option<String> {
    let mut file = open_regular(path)?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    // Cut in the middle of a line: that line is dropped.
    Some(if start > 0 { text.split_once('\n').map(|(_, rest)| rest.to_owned()).unwrap_or_default() } else { text })
}

/// One log line as a decision, when it is one (`kind: "decision"` with an id of the plug-in's form `d-` and ten hex digits).
fn parse_decision(line: &str) -> Option<(Decision, Option<String>)> {
    if !line.contains("\"decision\"") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    if v["kind"] != "decision" {
        return None;
    }
    let id = v["id"].as_str()?;
    let is_id = |s: &str| s.len() == 12 && s.starts_with("d-") && s[2..].chars().all(|c| c.is_ascii_hexdigit());
    if !is_id(id) {
        return None;
    }
    let supersedes = v["supersedes"].as_str().filter(|s| is_id(s)).map(str::to_owned);
    let files = v["files"].as_array().map(|f| f.iter().take(MAX_FILES_PER_DECISION).map(|x| clean(x, 120)).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
    let decision = Decision { id: id.to_owned(), topic: clean(&v["topic"], 120), chosen: clean(&v["chosen"], 300), reason: clean(&v["reason"], 300), files, at: v["at"].as_u64().unwrap_or(0), active: true };
    Some((decision, supersedes))
}

/// The state of the plug-in's folder, every session together. The plug-in does not tie a decision to a Claude session: `record_decision`
/// writes it to a log named after the MCP server's own start (`vhdb-mcp-<ms>-<pid>.jsonl`), so the decisions are the project's.
pub fn read(dir: &Path) -> Summary {
    if !dir.is_dir() {
        return Summary::default();
    }
    let switches = read_limited(&dir.join("config.json"), 8192).map(|b| parse_switches(&String::from_utf8_lossy(&b))).unwrap_or_default();
    let mut files = list_dir(&dir.join("log"), 500);
    files.retain(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("vhdb-") && n.ends_with(".jsonl")));
    // The newest logs first: a project with many sessions keeps the latest decisions in view.
    files.sort_by_key(|p| std::cmp::Reverse(fs::metadata(p).and_then(|m| m.modified()).ok()));
    files.truncate(MAX_LOG_FILES);
    let mut last_at = 0;
    let mut all: Vec<(Decision, Option<String>)> = Vec::new();
    for path in &files {
        if let Ok(modified) = fs::symlink_metadata(path).and_then(|m| m.modified()) {
            let ms = modified.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
            last_at = last_at.max(ms);
        }
        if let Some(text) = tail(path) {
            all.extend(text.lines().filter_map(parse_decision));
        }
    }
    // A decision is replaced when any decision names it in `supersedes`.
    let replaced: std::collections::HashSet<String> = all.iter().filter_map(|(_, s)| s.clone()).collect();
    let mut decisions: Vec<Decision> = all.into_iter().map(|(mut d, _)| {
        d.active = !replaced.contains(&d.id);
        d
    }).collect();
    decisions.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.id.cmp(&b.id)));
    // The same id written twice shows once, the newest.
    let mut seen = std::collections::HashSet::new();
    decisions.retain(|d| seen.insert(d.id.clone()));
    decisions.truncate(MAX_DECISIONS);
    Summary { found: true, switches, log_files: files.len(), last_at, decisions }
}

/// What the plug-in's hooks keep for one Claude session: `log/vhdb-<id>.jsonl`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mine {
    /// The session has a log file.
    Log,
    /// The plug-in wrote no log for it.
    NoLog,
    /// The chat has no Claude session id yet.
    NoSession,
    /// The session id is not one the plug-in names a log file after as it is, so its file is not looked for (no name is guessed).
    Unmappable,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionLog {
    pub state: Mine,
    /// The log file's last change (ms since the epoch), 0 when there is none.
    pub last_at: u64,
}

/// The name of the session's log file, `vhdb-<id>.jsonl`: the plug-in names it after the id as it is when the id is 1 to 64 letters,
/// digits, `_` or `-`, and after a hash of it otherwise (a case this does not reproduce: None).
pub fn session_log_name(id: &str) -> Option<String> {
    let ok = !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    ok.then(|| format!("vhdb-{id}.jsonl"))
}

/// Whether the plug-in keeps a log for this Claude session, and when it last changed. Only a plain file counts (never a link).
pub fn session_log(dir: &Path, claude_session_id: Option<&str>) -> SessionLog {
    let none = |state| SessionLog { state, last_at: 0 };
    let Some(id) = claude_session_id else { return none(Mine::NoSession) };
    let Some(name) = session_log_name(id) else { return none(Mine::Unmappable) };
    match fs::symlink_metadata(dir.join("log").join(name)) {
        Ok(m) if m.is_file() => SessionLog { state: Mine::Log, last_at: m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0) },
        _ => none(Mine::NoLog),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tenrec-vhdb-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&dir).expect("scratch folder");
        dir
    }

    const D1: &str = r#"{"v":1,"seq":1,"at":1000,"session":"vhdb-a","turn":0,"kind":"decision","id":"d-aaaaaaaaaa","topic":"토큰 갱신 위치","chosen":"interceptor","reason":"경쟁상태","files":["src/auth.js"]}"#;

    #[test]
    fn switches_follow_the_plugins_rules() {
        assert_eq!(parse_switches(""), Switches::default());
        assert_eq!(parse_switches("[1]"), Switches::default());
        let s = parse_switches(r#"{"record":true,"inject":"true","guard":1,"upload":true,"budgetTokens":900.7,"deadlineMs":10}"#);
        assert!(s.record && !s.inject && !s.guard && s.upload, "only an exact true counts");
        assert_eq!((s.budget_tokens, s.deadline_ms), (900, 250), "out of range is the default");
        assert_eq!(parse_switches(r#"{"budgetTokens":5000}"#).budget_tokens, 1200);
    }

    #[test]
    fn a_missing_folder_is_not_found_and_an_empty_one_has_everything_off() {
        let root = scratch();
        assert_eq!(read(&root.join("none")), Summary::default());
        fs::create_dir_all(root.join("vhdb")).unwrap();
        let s = read(&root.join("vhdb"));
        assert!(s.found && !s.switches.record && s.log_files == 0 && s.decisions.is_empty());
    }

    #[test]
    fn decisions_are_read_newest_first_and_a_replaced_one_says_so() {
        let root = scratch();
        let dir = root.join("vhdb");
        fs::create_dir_all(dir.join("log")).unwrap();
        fs::write(dir.join("config.json"), r#"{"record":true}"#).unwrap();
        let d2 = r#"{"kind":"decision","id":"d-bbbbbbbbbb","at":2000,"topic":"새 결정","chosen":"B","reason":"r","supersedes":"d-aaaaaaaaaa","files":[]}"#;
        let noise = r#"{"kind":"tool","text":"decision"}"#;
        fs::write(dir.join("log/vhdb-a.jsonl"), format!("{D1}\n{noise}\nnot json \"decision\"\n")).unwrap();
        fs::write(dir.join("log/vhdb-b.jsonl"), format!("{d2}\n")).unwrap();
        fs::write(dir.join("log/other.txt"), D1).unwrap();
        fs::write(dir.join("remote.json"), "{\"token\":\"SECRET\"}").unwrap();
        let s = read(&dir);
        assert!(s.found && s.switches.record && s.log_files == 2 && s.last_at > 0);
        assert_eq!(s.decisions.len(), 2);
        assert_eq!(s.decisions[0].id, "d-bbbbbbbbbb");
        assert!(s.decisions[0].active && !s.decisions[1].active, "{:?}", s.decisions);
        assert_eq!(s.decisions[1].files, vec!["src/auth.js"]);
        assert!(!format!("{s:?}").contains("SECRET"), "the upload token file is never read");
    }

    #[test]
    fn text_from_the_files_is_cut_and_stripped_and_bad_ids_are_ignored() {
        let root = scratch();
        let dir = root.join("vhdb");
        fs::create_dir_all(dir.join("log")).unwrap();
        let long = "x".repeat(900);
        let bad = format!(r#"{{"kind":"decision","id":"d-cccccccccc","at":5,"topic":"a\u001b[31mb","chosen":"{long}","reason":"ok","files":["a","b","c","d","e","f","g"]}}"#);
        let wrong_id = r#"{"kind":"decision","id":"../../etc","at":6,"topic":"x"}"#;
        fs::write(dir.join("log/vhdb-a.jsonl"), format!("{bad}\n{wrong_id}\n")).unwrap();
        let s = read(&dir);
        assert_eq!(s.decisions.len(), 1);
        let d = &s.decisions[0];
        assert_eq!(d.topic, "a[31mb", "the escape character is gone");
        assert_eq!(d.chosen.chars().count(), 300);
        assert_eq!(d.files.len(), MAX_FILES_PER_DECISION);
    }

    #[test]
    fn only_the_end_of_a_big_log_is_read_and_a_cut_line_is_dropped() {
        let root = scratch();
        let dir = root.join("vhdb");
        fs::create_dir_all(dir.join("log")).unwrap();
        let mut text = String::new();
        // An old decision at the start, then padding well past the tail limit, then a new one.
        text.push_str(D1);
        text.push('\n');
        while text.len() < (TAIL_BYTES as usize) + 4096 {
            text.push_str("{\"kind\":\"tool\",\"pad\":\"............................................\"}\n");
        }
        text.push_str(r#"{"kind":"decision","id":"d-dddddddddd","at":9000,"topic":"late","chosen":"c","reason":"r"}"#);
        text.push('\n');
        fs::write(dir.join("log/vhdb-a.jsonl"), text).unwrap();
        let s = read(&dir);
        assert_eq!(s.decisions.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["d-dddddddddd"]);
    }

    #[test]
    fn the_folder_is_found_next_to_the_git_data_even_from_a_worktree() {
        let root = scratch();
        fs::create_dir_all(root.join("p/.git")).unwrap();
        fs::create_dir_all(root.join("p/src")).unwrap();
        assert_eq!(vhdb_dir(&root.join("p/src")), Some(root.join("p/.git/vhdb")));
        assert_eq!(vhdb_dir(&root.join("elsewhere")), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_a_link_or_a_huge_config_in_the_folder_does_no_harm() {
        let root = scratch();
        let dir = root.join("vhdb");
        fs::create_dir_all(dir.join("log")).unwrap();
        // A pipe as the config and as a log, and a link to a real log: none is opened.
        for p in [dir.join("config.json"), dir.join("log/vhdb-pipe.jsonl")] {
            let c = std::ffi::CString::new(p.to_str().unwrap()).unwrap();
            // SAFETY: mkfifo with a valid NUL-terminated path.
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        }
        fs::write(root.join("real.jsonl"), format!("{D1}\n")).unwrap();
        std::os::unix::fs::symlink(root.join("real.jsonl"), dir.join("log/vhdb-link.jsonl")).unwrap();
        let s = read(&dir);
        assert!(s.found && !s.switches.record && s.decisions.is_empty(), "{s:?}");
        // A config far over the limit is cut: what is left of it is not valid JSON, so the defaults apply.
        let _ = fs::remove_file(dir.join("config.json"));
        let mut text = String::from(r#"{"record":true,"pad":""#);
        text.push_str(&"x".repeat(50_000));
        text.push_str("\"}");
        fs::write(dir.join("config.json"), text).unwrap();
        assert!(!read(&dir).switches.record);
    }
    #[test]
    fn decisions_the_plugin_wrote_from_its_mcp_server_are_the_projects_and_not_a_sessions() {
        let root = scratch();
        let dir = root.join("vhdb");
        fs::create_dir_all(dir.join("log")).unwrap();
        // `record_decision` writes to a log named after the MCP server's own start, not after a Claude session.
        fs::write(dir.join("log/vhdb-mcp-1700000000000-4242.jsonl"), format!("{D1}\n")).unwrap();
        fs::write(dir.join("log/vhdb-sess-a.jsonl"), "{\"kind\":\"tool\"}\n").unwrap();
        let s = read(&dir);
        assert_eq!(s.decisions.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["d-aaaaaaaaaa"]);
        assert_eq!(session_log(&dir, Some("sess-a")).state, Mine::Log, "the session's own log is a different file and holds no decision");
    }

    #[test]
    fn the_sessions_own_log_is_found_by_the_plugins_name_and_nothing_is_guessed() {
        let root = scratch();
        let dir = root.join("vhdb");
        fs::create_dir_all(dir.join("log")).unwrap();
        fs::write(dir.join("log/vhdb-sess-a.jsonl"), "{}\n").unwrap();
        let a = session_log(&dir, Some("sess-a"));
        assert_eq!(a.state, Mine::Log);
        assert!(a.last_at > 0);
        assert_eq!(session_log(&dir, Some("zz")), SessionLog { state: Mine::NoLog, last_at: 0 });
        assert_eq!(session_log(&dir, None).state, Mine::NoSession);
        for odd in ["x/y", "../a", "", "a b", &"x".repeat(65)] {
            assert_eq!(session_log(&dir, Some(odd)).state, Mine::Unmappable, "{odd:?}");
        }
        assert_eq!(session_log_name("5d6288aa-12_AB").as_deref(), Some("vhdb-5d6288aa-12_AB.jsonl"));
        assert_eq!(session_log(&root.join("nowhere"), Some("sess-a")).state, Mine::NoLog);
        #[cfg(unix)]
        {
            fs::write(root.join("real.jsonl"), "{}\n").unwrap();
            std::os::unix::fs::symlink(root.join("real.jsonl"), dir.join("log/vhdb-linked.jsonl")).unwrap();
            assert_eq!(session_log(&dir, Some("linked")).state, Mine::NoLog, "a link is not a log");
        }
    }
}
