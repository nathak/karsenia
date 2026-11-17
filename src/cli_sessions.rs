//! Sessions the `claude` command line program kept before: where they are, which ones there are, and what an earlier conversation
//! said as events this app's transcript draws. A session is `<config dir>/projects/<folder>/<uuid>.jsonl`, where `<folder>` is the
//! session's working folder with every character that is not a letter or a digit written as `-`, and `claude --resume <uuid>` finds it
//! only from that folder. The files are the person's own, but they are untrusted text: every string is cleaned and cut, nothing is
//! followed through a link, nothing in them is run, and nothing of their content is logged.
use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// How much of a session's head is read to find its folder and title.
const HEAD_BYTES: usize = 64 * 1024;
/// Most session files looked at in one scan.
/// How many session files are looked at (their sizes and times only); the newest are kept.
const MAX_FILES: usize = 50_000;
/// A session file larger than this is listed, but its conversation is not copied.
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// A line longer than this is skipped when a conversation is read.
const MAX_LINE: usize = 1024 * 1024;
const MAX_TOOL_TEXT: usize = 8 * 1024;
const MAX_TOOL_INPUT: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct CliSession {
    pub id: String,
    pub path: PathBuf,
    pub cwd: String,
    pub title: String,
    pub branch: String,
    pub modified_ms: u64,
    pub size: u64,
    /// `claude --resume` can find the session from `cwd`: the folder the file is in is the one `cwd` makes.
    pub folder_ok: bool,
}

/// The folder name the CLI gives a working folder: each UTF-16 code unit that is not an ASCII letter or digit becomes `-` (so a
/// Korean letter is one `-` and a letter outside the basic plane is two).
pub fn encode_dir(cwd: &str) -> String {
    cwd.encode_utf16().map(|u| if u < 128 && (u as u8).is_ascii_alphanumeric() { u as u8 as char } else { '-' }).collect()
}

/// Control characters become spaces, runs of spaces one, and the text is cut to `max` characters.
pub fn clean(text: &str, max: usize) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    flat.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

/// A line-keeping variant for message text: control characters other than newline and tab go, nothing is joined.
fn clean_keep_lines(text: &str) -> String {
    text.chars().filter(|c| *c == '\n' || *c == '\t' || !c.is_control()).collect()
}

fn is_uuid_name(name: &str) -> Option<&str> {
    let id = name.strip_suffix(".jsonl")?;
    let b = id.as_bytes();
    let ok = b.len() == 36 && b.iter().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { *c == b'-' } else { c.is_ascii_digit() || (b'a'..=b'f').contains(c) });
    ok.then_some(id)
}

/// The text of a user line that is a person's words, not a wrapper the CLI wrote around a command or a reminder.
fn prompt_text(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).filter(|t| !crate::workbench::inject::is_block(t)).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    let t = text.trim_start();
    if let Some(rest) = t.strip_prefix("<command-name>") {
        // A slash command: shown as the command with its arguments.
        let name = rest.split("</command-name>").next().unwrap_or_default().trim();
        let args = t.split("<command-args>").nth(1).and_then(|a| a.split("</command-args>").next()).unwrap_or_default().trim();
        return Some(format!("{name} {args}").trim().to_owned()).filter(|s| !s.is_empty());
    }
    for wrapper in ["<local-command-", "<command-message>", "Caveat:", "<system-reminder>"] {
        if t.starts_with(wrapper) {
            return None;
        }
    }
    (!t.trim().is_empty()).then_some(text)
}

fn read_head(path: &Path) -> Option<Vec<Value>> {
    let mut buf = Vec::new();
    std::fs::File::open(path).ok()?.take(HEAD_BYTES as u64).read_to_end(&mut buf).ok()?;
    let whole = buf.len() < HEAD_BYTES;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    // A line cut at the end of the head is half a line: left out.
    if !whole {
        lines.pop();
    }
    Some(lines.into_iter().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect())
}

fn describe(path: &Path, folder: &str, id: &str, size: u64, modified_ms: u64) -> Option<CliSession> {
    let head = read_head(path)?;
    let live = |e: &&Value| matches!(e["type"].as_str(), Some("user" | "assistant")) && e["isSidechain"] != true;
    let first = head.iter().find(live)?;
    // The folder is used as the CLI wrote it (spaces and length included): a changed name would be a different folder. One that
    // cannot be a path, or is absurdly long, leaves the session out.
    let cwd = first["cwd"].as_str()?.to_owned();
    if cwd.is_empty() || cwd.len() > 4096 || cwd.chars().any(char::is_control) {
        return None;
    }
    let title = head
        .iter()
        .filter(|e| e["type"] == "user" && e["isMeta"] != true && e["isSidechain"] != true)
        .find_map(|e| prompt_text(&e["message"]["content"]))
        .or_else(|| head.iter().find(|e| e["type"] == "summary").and_then(|e| e["summary"].as_str().map(str::to_owned)))
        .map(|t| clean(&t, 80))
        .unwrap_or_default();
    Some(CliSession { id: id.to_owned(), path: path.to_owned(), folder_ok: encode_dir(&cwd) == folder, cwd, title, branch: clean(first["gitBranch"].as_str().unwrap_or_default(), 100), modified_ms, size })
}

/// The sessions under `<config_dir>/projects`, newest first, at most `limit`. Folders and files that are links are not entered or
/// read; a file whose name is not a session id (the sub-agents' `agent-*.jsonl`) is not one.
pub fn list(config_dir: &Path, limit: usize) -> Vec<CliSession> {
    let projects = config_dir.join("projects");
    let real_dir = |p: &Path| std::fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false);
    if !real_dir(&projects) {
        return Vec::new();
    }
    let mut found: Vec<(u64, u64, String, String, PathBuf)> = Vec::new();
    'folders: for folder in std::fs::read_dir(&projects).into_iter().flatten().flatten() {
        let dir = folder.path();
        let Some(folder_name) = dir.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
        if !real_dir(&dir) {
            continue;
        }
        for file in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(kind) = file.file_type() else { continue };
            if !kind.is_file() {
                continue;
            }
            let name = file.file_name();
            let Some(id) = name.to_str().and_then(is_uuid_name) else { continue };
            let Ok(meta) = file.metadata() else { continue };
            if meta.len() == 0 {
                continue;
            }
            let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
            found.push((modified, meta.len(), id.to_owned(), folder_name.clone(), file.path()));
            if found.len() >= MAX_FILES {
                break 'folders;
            }
        }
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    // The heads of the newest are read; some will not be sessions (no conversation line), so a few more than `limit` are tried.
    found.into_iter().take(limit.saturating_mul(2)).filter_map(|(modified, size, id, folder, path)| describe(&path, &folder, &id, size, modified)).take(limit).collect()
}

// ---- a conversation as events ----

#[derive(Debug, Default)]
pub struct History {
    /// (time in ms, kind, data): the kinds the transcript draws, `user` and `claude`.
    pub events: Vec<(u64, &'static str, Value)>,
    /// How many conversation events the file had, and how many of them are not in `events`.
    pub total: usize,
    pub dropped: usize,
}

/// `2026-11-13T05:27:59.656Z` as milliseconds since the epoch.
pub fn parse_time(text: &str) -> Option<u64> {
    let b = text.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |a: usize, z: usize| text.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, s) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let millis: i64 = text.get(19..).and_then(|r| r.strip_prefix('.')).map(|r| r.chars().take_while(char::is_ascii_digit).take(3).collect::<String>()).filter(|d| !d.is_empty()).map(|d| format!("{d:0<3}").parse().unwrap_or(0)).unwrap_or(0);
    // Days since 1970-01-01 of a civil date.
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * ((mo + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(((days * 24 + h) * 60 + mi) * 60 * 1000 + s * 1000 + millis).ok()
}

fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// The blocks of a tool result as text only, each cut; images are left out.
fn tool_result_content(content: &Value) -> Value {
    match content {
        Value::String(s) => json!(cut(&clean_keep_lines(s), MAX_TOOL_TEXT)),
        Value::Array(blocks) => json!(blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).map(|t| json!({"type": "text", "text": cut(&clean_keep_lines(t), MAX_TOOL_TEXT)})).collect::<Vec<_>>()),
        _ => json!(""),
    }
}

fn convert_block(block: &Value) -> Option<Value> {
    match block["type"].as_str()? {
        "text" => Some(json!({"type": "text", "text": clean_keep_lines(block["text"].as_str().unwrap_or_default())})),
        "tool_use" => {
            let input = &block["input"];
            let input = if input.to_string().len() > MAX_TOOL_INPUT { json!({"_truncated": true}) } else { input.clone() };
            Some(json!({"type": "tool_use", "id": clean(block["id"].as_str().unwrap_or_default(), 100), "name": clean(block["name"].as_str().unwrap_or_default(), 100), "input": input}))
        }
        _ => None,
    }
}

/// One conversation line as the events it becomes (a user line can be a message and tool results).
fn convert_line(line: &Value) -> Vec<(&'static str, Value)> {
    let content = &line["message"]["content"];
    let mut out = Vec::new();
    match line["type"].as_str() {
        Some("assistant") => {
            let blocks: Vec<Value> = content.as_array().into_iter().flatten().filter_map(convert_block).collect();
            if !blocks.is_empty() {
                let model = clean(line["message"]["model"].as_str().unwrap_or_default(), 100);
                out.push(("claude", json!({"type": "assistant", "message": {"role": "assistant", "model": model, "content": blocks}, "parent_tool_use_id": null, "imported": true})));
            }
        }
        Some("user") => {
            if let Some(text) = prompt_text(content) {
                out.push(("user", json!({"text": clean_keep_lines(&text), "images": [], "imported": true})));
            }
            let results: Vec<Value> = content
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| b["type"] == "tool_result")
                .map(|b| json!({"type": "tool_result", "tool_use_id": clean(b["tool_use_id"].as_str().unwrap_or_default(), 100), "is_error": b["is_error"] == true, "content": tool_result_content(&b["content"])}))
                .collect();
            if !results.is_empty() {
                out.push(("claude", json!({"type": "user", "message": {"role": "user", "content": results}, "parent_tool_use_id": null, "imported": true})));
            }
        }
        _ => {}
    }
    out
}

/// The conversation of a session file as events: the last `max_events` of it and at most `max_bytes`, in order, with the branches the
/// person rewound from left out. An Err is an i18n key.
pub fn read_history(path: &Path, max_events: usize, max_bytes: usize) -> Result<History, &'static str> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| "imp_read_fail")?;
    if !meta.is_file() {
        return Err("imp_read_fail");
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err("imp_too_big");
    }
    let file = std::fs::File::open(path).map_err(|_| "imp_read_fail")?;
    let mut lines: Vec<Value> = Vec::new();
    let mut skipped = 0usize;
    for line in std::io::BufReader::new(file).split(b'\n') {
        let Ok(bytes) = line else { return Err("imp_read_fail") };
        if bytes.len() > MAX_LINE {
            skipped += 1;
            continue;
        }
        if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
            lines.push(v);
        }
    }
    let is_turn = |l: &Value| matches!(l["type"].as_str(), Some("user" | "assistant")) && l["isSidechain"] != true && l["isMeta"] != true && l["isCompactSummary"] != true;
    let targets: Vec<usize> = (0..lines.len()).filter(|i| is_turn(&lines[*i])).collect();
    let by_uuid: HashMap<&str, usize> = lines.iter().enumerate().filter_map(|(i, l)| l["uuid"].as_str().map(|u| (u, i))).collect();
    // From the last turn back along the parents: a branch the person rewound from is not on that path.
    let mut path: Vec<usize> = Vec::new();
    let mut at = targets.last().copied();
    // Set when the chain ends at a parent that is not in the file, or goes round: then it says nothing about what was kept.
    let mut broken = false;
    let mut seen = vec![false; lines.len()];
    while let Some(i) = at {
        // A line met twice is a cycle (a forged parent): each line is taken once, so the path is never longer than the file.
        if std::mem::replace(&mut seen[i], true) {
            broken = true;
            break;
        }
        if is_turn(&lines[i]) {
            path.push(i);
        }
        let parent = lines[i]["parentUuid"].as_str().or_else(|| lines[i]["logicalParentUuid"].as_str());
        at = match parent {
            // The first line of the conversation has no parent.
            None => None,
            Some(p) => {
                let found = by_uuid.get(p).copied();
                broken |= found.is_none();
                found
            }
        };
    }
    path.reverse();
    // A chain that broke early is not the conversation: the file's own order is used instead. A complete chain is, however short.
    let order: Vec<usize> = if broken && path.len() * 2 < targets.len() { targets.clone() } else { path };
    let mut events: Vec<(u64, &'static str, Value)> = Vec::new();
    // A line without a time takes the one before it (the first, the file's own); times never go backwards.
    let file_time = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
    let mut last_ts = 0u64;
    for i in &order {
        let ts = lines[*i]["timestamp"].as_str().and_then(parse_time).unwrap_or(if last_ts > 0 { last_ts } else { file_time }).max(last_ts);
        last_ts = ts;
        for (kind, data) in convert_line(&lines[*i]) {
            events.push((ts, kind, data));
        }
    }
    let total = events.len() + skipped;
    // The tail is kept: the last events, up to the byte limit.
    let mut keep = events.len().saturating_sub(max_events);
    let mut bytes = 0usize;
    for (n, e) in events.iter().enumerate().skip(keep).rev() {
        bytes += e.2.to_string().len();
        if bytes > max_bytes {
            keep = n + 1;
            break;
        }
    }
    let mut events = events.split_off(keep);
    // A tool result at the start would have no call before it.
    while events.first().is_some_and(|(_, kind, d)| *kind == "claude" && d["type"] == "user") {
        events.remove(0);
    }
    let dropped = total - events.len();
    Ok(History { events, total, dropped })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID1: &str = "11111111-2222-3333-4444-555555555555";
    const ID2: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    fn line(kind: &str, uuid: &str, parent: Option<&str>, cwd: &str, message: Value, ts: &str) -> String {
        json!({"type": kind, "uuid": uuid, "parentUuid": parent, "cwd": cwd, "sessionId": ID1, "gitBranch": "main", "isSidechain": false, "timestamp": ts, "message": message}).to_string()
    }

    fn write(dir: &Path, folder: &str, name: &str, lines: &[String]) -> PathBuf {
        let d = dir.join("projects").join(folder);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join(name);
        std::fs::write(&p, lines.join("\n") + "\n").unwrap();
        p
    }

    #[test]
    fn folder_names_follow_the_cli() {
        assert_eq!(encode_dir(r"C:\projects\IR"), "C--projects-IR");
        assert_eq!(encode_dir("/home/me/my.app"), "-home-me-my-app");
        assert_eq!(encode_dir("/w/한글"), "-w---");
        assert_eq!(encode_dir("/w/😀"), "-w---");
    }

    #[test]
    fn a_session_is_listed_with_title_folder_and_whether_it_can_be_resumed() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = "/work/my proj";
        let folder = encode_dir(cwd);
        write(dir.path(), &folder, &format!("{ID1}.jsonl"), &[
            json!({"type": "queue-operation", "sessionId": ID1}).to_string(),
            line("user", "u0", None, cwd, json!({"role": "user", "content": "<local-command-caveat>Caveat: x</local-command-caveat>"}), "2026-11-13T05:00:00.000Z"),
            line("user", "u1", Some("u0"), cwd, json!({"role": "user", "content": "fix   the\u{7} bug"}), "2026-11-13T05:00:01.000Z"),
            "not json at all".into(),
        ]);
        // Not sessions: a sub-agent's file, a file without a conversation.
        write(dir.path(), &folder, "agent-abc.jsonl", &[line("user", "x", None, cwd, json!({"content": "hi"}), "2026-11-13T05:00:00Z")]);
        write(dir.path(), &folder, &format!("{ID2}.jsonl"), &[json!({"type": "summary", "summary": "s"}).to_string()]);
        // Resumable only from its own folder: this one sits under another name.
        write(dir.path(), "-other-name", "99999999-2222-3333-4444-555555555555.jsonl", &[line("user", "u", None, cwd, json!({"content": "elsewhere"}), "2026-11-13T05:00:00Z")]);
        let found = list(dir.path(), 50);
        assert_eq!(found.len(), 2, "{found:?}");
        let a = found.iter().find(|s| s.id == ID1).expect("listed");
        assert_eq!((a.title.as_str(), a.cwd.as_str(), a.branch.as_str(), a.folder_ok), ("fix the bug", cwd, "main", true));
        assert!(!found.iter().find(|s| s.id.starts_with("9999")).expect("listed").folder_ok);
        assert!(list(&dir.path().join("none"), 5).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn links_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let cwd = "/work/p";
        let real = write(other.path(), &encode_dir(cwd), &format!("{ID1}.jsonl"), &[line("user", "u", None, cwd, json!({"content": "hi"}), "2026-11-13T05:00:00Z")]);
        let projects = dir.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        std::os::unix::fs::symlink(real.parent().unwrap(), projects.join("linked-folder")).unwrap();
        std::fs::create_dir_all(projects.join("real")).unwrap();
        std::os::unix::fs::symlink(&real, projects.join("real").join(format!("{ID2}.jsonl"))).unwrap();
        assert!(list(dir.path(), 5).is_empty());
    }

    #[test]
    fn a_first_line_longer_than_the_head_leaves_nothing_to_describe() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = "/w/p";
        let big = line("system", "s", None, cwd, json!({"content": "x".repeat(70_000)}), "2026-11-13T05:00:00Z");
        write(dir.path(), &encode_dir(cwd), &format!("{ID1}.jsonl"), &[big, line("user", "u", None, cwd, json!({"content": "after the big line"}), "2026-11-13T05:00:01Z")]);
        // The head is cut inside the first line, which is dropped as half a line: nothing is left to find a folder in.
        assert!(list(dir.path(), 5).is_empty());
    }

    #[test]
    fn times_parse() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("1970-01-02T00:00:01.5Z"), Some(86_401_500));
        assert_eq!(parse_time("2000-03-01T00:00:00Z"), Some(951_868_800_000));
        assert_eq!(parse_time("nope"), None);
        assert_eq!(parse_time("2026-13-01T00:00:00Z"), None);
    }

    fn convo(dir: &Path) -> PathBuf {
        let cwd = "/w/p";
        write(dir, &encode_dir(cwd), &format!("{ID1}.jsonl"), &[
            line("user", "u1", None, cwd, json!({"role": "user", "content": "question one"}), "2026-11-13T05:00:00.000Z"),
            line("assistant", "a1", Some("u1"), cwd, json!({"role": "assistant", "model": "claude-x", "content": [{"type": "text", "text": "answer"}, {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "/x"}}]}), "2026-11-13T05:00:01.000Z"),
            line("user", "u2", Some("a1"), cwd, json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "R".repeat(20_000)}]}), "2026-11-13T05:00:02.000Z"),
            // A branch the person rewound from, then the line they kept.
            line("assistant", "dead", Some("u2"), cwd, json!({"content": [{"type": "text", "text": "abandoned"}]}), "2026-11-13T05:00:03.000Z"),
            line("assistant", "a2", Some("u2"), cwd, json!({"content": [{"type": "text", "text": "kept"}, {"type": "thinking", "thinking": "hmm", "signature": "SECRET"}]}), "2026-11-13T05:00:04.000Z"),
            json!({"type": "user", "uuid": "side", "parentUuid": "a2", "isSidechain": true, "message": {"content": "sub-agent"}}).to_string(),
        ])
    }

    #[test]
    fn a_conversation_becomes_events_without_the_abandoned_branch() {
        let dir = tempfile::tempdir().unwrap();
        let h = read_history(&convo(dir.path()), 400, 4 << 20).expect("history");
        let kinds: Vec<&str> = h.events.iter().map(|e| e.1).collect();
        assert_eq!(kinds, vec!["user", "claude", "claude", "claude"], "{:?}", h.events);
        assert_eq!(h.events[0].2["text"], "question one");
        assert_eq!(h.events[0].2["imported"], true);
        assert!(h.events.iter().all(|e| e.2["imported"] == true), "every imported event says so");
        let result = &h.events[2].2["message"]["content"][0];
        assert_eq!(result["tool_use_id"], "t1");
        assert!(result["content"].as_str().unwrap().len() <= MAX_TOOL_TEXT + 4, "a long result is cut");
        let text = serde_json::to_string(&h.events).unwrap();
        assert!(!text.contains("abandoned") && !text.contains("sub-agent") && !text.contains("SECRET") && !text.contains("hmm"), "{text}");
        assert!(h.events.windows(2).all(|w| w[0].0 <= w[1].0), "time does not go back");
        assert_eq!(h.dropped, 0);
        // The time of an event is the time the CLI wrote, not when the file was last touched (that is later).
        assert_eq!(h.events[0].0, parse_time("2026-11-13T05:00:00.000Z").unwrap());
    }

    #[test]
    fn a_short_complete_chain_after_a_rewind_is_the_conversation_and_a_folder_keeps_its_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = "/w/my  project";
        let mut lines = vec![line("user", "u1", None, cwd, json!({"role": "user", "content": "start"}), "2026-11-13T05:00:00.000Z")];
        // Four abandoned answers, then the person rewound to the first message and went on.
        for n in 0..4 {
            lines.push(line("assistant", &format!("dead{n}"), Some("u1"), cwd, json!({"content": [{"type": "text", "text": "abandoned"}]}), "2026-11-13T05:00:01.000Z"));
        }
        lines.push(line("assistant", "kept", Some("u1"), cwd, json!({"content": [{"type": "text", "text": "kept"}]}), "2026-11-13T05:00:09.000Z"));
        write(dir.path(), &encode_dir(cwd), &format!("{ID1}.jsonl"), &lines);
        let found = list(dir.path(), 10);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].cwd, cwd);
        assert!(found[0].folder_ok);
        let text = serde_json::to_string(&read_history(&found[0].path, 400, 4 << 20).unwrap().events).unwrap();
        assert!(text.contains("kept") && !text.contains("abandoned"), "{text}");
    }

    #[test]
    fn a_cycle_of_parents_cannot_make_a_path_longer_than_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = "/w/p";
        let mut lines = vec![line("user", "u1", None, cwd, json!({"role": "user", "content": "start"}), "2026-11-13T05:00:00.000Z")];
        for n in 0..20 {
            lines.push(line("assistant", &format!("a{n}"), Some("u1"), cwd, json!({"content": [{"type": "text", "text": format!("answer {n}")}]}), "2026-11-13T05:00:01.000Z"));
        }
        // The last line is its own parent.
        lines.push(line("assistant", "loop", Some("loop"), cwd, json!({"content": [{"type": "text", "text": "L".repeat(1000)}]}), "2026-11-13T05:00:02.000Z"));
        let path = write(dir.path(), &encode_dir(cwd), &format!("{ID1}.jsonl"), &lines);
        let h = read_history(&path, 400, 4 << 20).unwrap();
        assert!(h.events.len() <= lines.len(), "{} events from {} lines", h.events.len(), lines.len());
        assert_eq!(h.events.iter().filter(|e| e.2.to_string().contains("LLLL")).count(), 1);
    }

    #[test]
    fn only_the_tail_is_kept_and_it_never_starts_with_a_result_without_its_call() {
        let dir = tempfile::tempdir().unwrap();
        let path = convo(dir.path());
        // Four events: user, assistant (call), result, assistant. Keeping two would start on the result: it goes too.
        let three = read_history(&path, 3, 4 << 20).expect("history");
        assert_eq!(three.events.len(), 3, "three start on the call, which is fine");
        let h = read_history(&path, 2, 4 << 20).expect("history");
        assert_eq!(h.events.len(), 1, "{:?}", h.events.iter().map(|e| e.1).collect::<Vec<_>>());
        assert_eq!((h.total, h.dropped), (4, 3));
        let small = read_history(&path, 400, 200).expect("history");
        assert!(small.events.len() < 4 && small.dropped > 0);
        assert_eq!(read_history(&dir.path().join("none"), 5, 5).unwrap_err(), "imp_read_fail");
    }

    #[test]
    fn a_huge_file_is_refused_for_history() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(format!("{ID1}.jsonl"));
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert_eq!(read_history(&p, 5, 5).unwrap_err(), "imp_too_big");
    }
}
