//! What the Team page shows of the SmartWorkbench plug-in's guard and done conditions: the snapshot file the plug-in writes for each
//! Claude session, `<its desktop folder>/state/<claude session id>.json` (version 1; written atomically, 64 KB at most, and only while
//! the plug-in's own desktop app is running, so a missing or old file is normal). Read only, and only that file: nothing else in that
//! folder is opened and nothing is written there. The text is the person's own goals and rules: it is shown in the window and never
//! logged. The plug-in has no "guard pipeline stages": what it reports (profile, pause, rule count, recent checks, open questions and
//! proposals) is shown, and nothing is made up.
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::git::read_limited;

pub const MAX_BYTES: u64 = 64 * 1024;
/// A snapshot older than this is shown with a note that the desktop app may not be running (it only colours the age line).
pub const STALE_MS: u64 = 30 * 60 * 1000;
const MAX_LIST: usize = 20;
const MAX_DONE: usize = 30;
const MAX_EVIDENCE: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Refusal {
    BadId,
    NotRegular,
    TooBig,
    NotJson,
    Version,
    OtherSession,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DoneStatus {
    Pending,
    Observed,
    Verified,
    Failed,
    Waived,
    Unknown,
}

impl DoneStatus {
    /// A condition counts as met when it was verified or waived.
    pub fn met(self) -> bool {
        matches!(self, DoneStatus::Verified | DoneStatus::Waived)
    }

    pub fn word(self) -> &'static str {
        match self {
            DoneStatus::Pending => "pending",
            DoneStatus::Observed => "observed",
            DoneStatus::Verified => "verified",
            DoneStatus::Failed => "failed",
            DoneStatus::Waived => "waived",
            DoneStatus::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Profile {
    Off,
    Permissive,
    Balanced,
    Strict,
    Unknown,
}

impl Profile {
    pub fn word(self) -> &'static str {
        match self {
            Profile::Off => "off",
            Profile::Permissive => "permissive",
            Profile::Balanced => "balanced",
            Profile::Strict => "strict",
            Profile::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DoneCond {
    pub text: String,
    pub link: String,
    pub status: DoneStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Evidence {
    /// test, build, typecheck, lint or ci (anything else is dropped).
    pub kind: String,
    pub ok: bool,
    pub at: u64,
}

/// The team settings a snapshot reports for its session. A role's model or effort the plug-in did not give (null, absent, or a value
/// that is not a plain model or effort word) is None: it is shown as "no value", never as a default.
#[derive(Debug, Clone, PartialEq)]
pub struct SwbTeam {
    pub enabled: bool,
    /// `lead`, `explorer`, `advisor`, each with its model and effort.
    pub roles: [(&'static str, Option<String>, Option<String>); 3],
    pub codex: bool,
}

fn role_word(v: &Value, cap: usize, ok: fn(char) -> bool) -> Option<String> {
    let s = v.as_str()?;
    (!s.is_empty() && s.chars().count() <= cap && s.chars().all(ok)).then(|| s.to_owned())
}

/// The snapshot's `team` object, when it has one the plug-in's way (an object with a true or false `enabled`); None otherwise
/// (an older plug-in, or a shape that is not this one): nothing is made up.
fn parse_team(v: &Value) -> Option<SwbTeam> {
    let team = v.as_object()?;
    let enabled = match team.get("enabled")? {
        Value::Bool(b) => *b,
        _ => return None,
    };
    let get = |group: &str, role: &str| team.get(group).and_then(|g| g.get(role)).unwrap_or(&Value::Null).clone();
    let role = |name: &'static str| (name, role_word(&get("roles", name), 40, super::team::is_model_char), role_word(&get("efforts", name), 16, |c| c.is_ascii_lowercase()));
    Some(SwbTeam { enabled, roles: [role("lead"), role("explorer"), role("advisor")], codex: team.get("codex") == Some(&Value::Bool(true)) })
}

/// What a snapshot says. Only `parse` makes one.
#[derive(Debug, Clone, PartialEq)]
pub struct Swb {
    /// When the plug-in wrote it (ms since the epoch).
    pub at: u64,
    pub goal: String,
    pub status: String,
    pub locked: bool,
    pub constraints: Vec<String>,
    pub non_goals: Vec<String>,
    pub done: Vec<DoneCond>,
    pub profile: Profile,
    pub paused: bool,
    pub rules: u32,
    pub evidence: Vec<Evidence>,
    /// Only counted: what a question or a proposal says is not shown.
    pub open_asks: usize,
    pub open_proposals: usize,
    /// The session's team settings, when the snapshot has them.
    pub team: Option<SwbTeam>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Snapshot {
    Missing,
    Refused(Refusal),
    Found(Box<Swb>),
}

/// A plain session id: the characters an id is made of, so it cannot name another folder.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The folder that holds the snapshots: `over` (the plug-in's folder variable; it must be an absolute path) or the plug-in's folder in
/// the home folder, then `state`. None when no folder can be named: nothing is guessed.
pub fn state_dir(over: Option<&OsStr>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = match over.filter(|o| !o.is_empty()) {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            dir.is_absolute().then_some(dir)?
        }
        None => home?.join(".claude").join("smartworkbench-desktop"),
    };
    Some(base.join("state"))
}

pub fn state_dir_from_env() -> Option<PathBuf> {
    state_dir(std::env::var_os("SWB_DESKTOP_HOME").as_deref(), crate::platform::home_dir())
}

fn clean(v: &Value, cap: usize) -> String {
    v.as_str().unwrap_or_default().chars().filter(|c| !c.is_control() || *c == ' ').take(cap).collect::<String>().trim().to_owned()
}

fn texts(items: Option<&Vec<Value>>, field: Option<&str>) -> Vec<String> {
    items
        .map(|a| {
            a.iter()
                .take(MAX_LIST)
                .map(|x| clean(field.map_or(x, |f| &x[f]), 200))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// A snapshot's bytes as the window shows them.
pub fn parse(bytes: &[u8], id: &str) -> Result<Swb, Refusal> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| Refusal::NotJson)?;
    if !v.is_object() {
        return Err(Refusal::NotJson);
    }
    if v["v"].as_u64() != Some(1) {
        return Err(Refusal::Version);
    }
    if v["session"].as_str() != Some(id) {
        return Err(Refusal::OtherSession);
    }
    let task = &v["task"];
    let done = task["doneConditions"]
        .as_array()
        .map(|a| {
            a.iter()
                .take(MAX_DONE)
                .map(|c| DoneCond {
                    text: clean(&c["text"], 300),
                    link: clean(&c["link"], 200),
                    status: match c["status"].as_str() {
                        Some("pending") => DoneStatus::Pending,
                        Some("observed") => DoneStatus::Observed,
                        Some("verified") => DoneStatus::Verified,
                        Some("failed") => DoneStatus::Failed,
                        Some("waived") => DoneStatus::Waived,
                        _ => DoneStatus::Unknown,
                    },
                })
                .filter(|c| !c.text.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let evidence = v["evidence"]
        .as_array()
        .map(|a| {
            a.iter()
                .take(MAX_EVIDENCE)
                .filter_map(|e| {
                    let kind = e["kind"].as_str().filter(|k| ["test", "build", "typecheck", "lint", "ci"].contains(k))?;
                    Some(Evidence { kind: kind.to_owned(), ok: e["ok"] == Value::Bool(true), at: e["at"].as_u64().unwrap_or(0) })
                })
                .collect()
        })
        .unwrap_or_default();
    let guard = &v["guard"];
    Ok(Swb {
        at: v["at"].as_u64().unwrap_or(0),
        goal: clean(&task["goal"], 500),
        status: clean(&task["status"], 20),
        locked: task["locked"] == Value::Bool(true),
        constraints: texts(task["constraints"].as_array(), Some("text")),
        non_goals: texts(task["nonGoals"].as_array(), None),
        done,
        profile: match guard["profile"].as_str() {
            Some("off") => Profile::Off,
            Some("permissive") => Profile::Permissive,
            Some("balanced") => Profile::Balanced,
            Some("strict") => Profile::Strict,
            _ => Profile::Unknown,
        },
        paused: guard["paused"] == Value::Bool(true),
        rules: guard["rules"].as_u64().unwrap_or(0).min(100_000) as u32,
        evidence,
        open_asks: v["asks"].as_array().map_or(0, |a| a.len().min(1000)),
        open_proposals: v["proposals"].as_array().map_or(0, |a| a.len().min(1000)),
        team: parse_team(&v["team"]),
    })
}

/// The snapshot of a Claude session. Only a regular file of the right size is read (a link, a folder or a pipe is refused, a file
/// over the plug-in's own limit is not parsed).
pub fn read(dir: &Path, id: &str) -> Snapshot {
    if !valid_id(id) {
        return Snapshot::Refused(Refusal::BadId);
    }
    let path = dir.join(format!("{id}.json"));
    if std::fs::symlink_metadata(&path).is_err() {
        return Snapshot::Missing;
    }
    let Some(bytes) = read_limited(&path, MAX_BYTES + 1) else { return Snapshot::Refused(Refusal::NotRegular) };
    if bytes.len() as u64 > MAX_BYTES {
        return Snapshot::Refused(Refusal::TooBig);
    }
    match parse(&bytes, id) {
        Ok(swb) => Snapshot::Found(Box::new(swb)),
        Err(why) => Snapshot::Refused(why),
    }
}

impl Swb {
    /// How many conditions are met.
    pub fn met(&self) -> usize {
        self.done.iter().filter(|c| c.status.met()).count()
    }

    /// The newest result of each kind of check, in the order the kinds first appear.
    pub fn latest_evidence(&self) -> Vec<Evidence> {
        let mut out: Vec<Evidence> = Vec::new();
        for e in &self.evidence {
            match out.iter_mut().find(|o| o.kind == e.kind) {
                Some(old) if old.at <= e.at => *old = e.clone(),
                Some(_) => {}
                None => out.push(e.clone()),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAP: &str = r#"{"v":1,"at":5000,"session":"abc-1","scope":"session","rev":3,"future":{"x":1},
      "task":{"status":"active","locked":true,"goal":"ship the page","constraints":[{"id":"c1","text":"no new deps","priority":"hard"}],"nonGoals":["rewrite"],
        "doneConditions":[{"id":"d1","text":"tests pass","link":null,"status":"verified"},{"id":"d2","text":"docs\u001b[31m updated","link":"https://x.test/a","status":"pending"},
        {"id":"d3","text":"odd","status":"strange"},{"id":"d4","text":"","status":"pending"},{"id":"d5","text":"waived one","status":"waived"},{"id":"d6","text":"seen","status":"observed"}]},
      "team":{"enabled":true},
      "guard":{"profile":"strict","paused":true,"rules":12,"file":{"hash":"x"}},
      "evidence":[{"kind":"test","ok":false,"at":10},{"kind":"test","ok":true,"at":20},{"kind":"build","ok":false,"at":15},{"kind":"weird","ok":true,"at":30}],
      "proposals":[{"id":"p1"}],"asks":[{"id":"a1"},{"id":"a2"}]}"#;

    fn scratch() -> tempfile::TempDir {
        tempfile::tempdir().expect("scratch folder")
    }

    #[test]
    fn only_a_plain_session_id_names_a_file() {
        assert!(valid_id("5d6288aa-12_AB"));
        for bad in ["", "..", "../x", "a/b", "a\\b", ".x", "a.b", "a b", "가", &"x".repeat(129)] {
            assert!(!valid_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_folder_comes_from_the_variable_when_it_is_absolute_else_from_home() {
        let home = Some(PathBuf::from("/h"));
        assert_eq!(state_dir(None, home.clone()), Some(PathBuf::from("/h/.claude/smartworkbench-desktop/state")));
        assert_eq!(state_dir(Some(OsStr::new("")), home.clone()), Some(PathBuf::from("/h/.claude/smartworkbench-desktop/state")));
        assert_eq!(state_dir(Some(OsStr::new("/x/swb")), home.clone()), Some(PathBuf::from("/x/swb/state")));
        assert_eq!(state_dir(Some(OsStr::new("rel/dir")), home), None, "a relative folder is not guessed at");
        assert_eq!(state_dir(None, None), None);
    }

    #[test]
    fn a_snapshot_gives_conditions_guard_and_checks_and_ignores_what_it_does_not_know() {
        let s = parse(SNAP.as_bytes(), "abc-1").expect("parsed");
        assert_eq!((s.at, s.goal.as_str(), s.status.as_str(), s.locked, s.paused, s.rules, s.open_asks, s.open_proposals), (5000, "ship the page", "active", true, true, 12, 2, 1));
        assert_eq!(s.profile, Profile::Strict);
        assert_eq!(s.constraints, ["no new deps"]);
        assert_eq!(s.non_goals, ["rewrite"]);
        let done: Vec<(&str, DoneStatus)> = s.done.iter().map(|c| (c.text.as_str(), c.status)).collect();
        assert_eq!(done, [("tests pass", DoneStatus::Verified), ("docs[31m updated", DoneStatus::Pending), ("odd", DoneStatus::Unknown), ("waived one", DoneStatus::Waived), ("seen", DoneStatus::Observed)]);
        assert_eq!(s.met(), 2, "verified and waived count; observed does not");
        assert_eq!(s.done[1].link, "https://x.test/a");
        // A check of a kind the plug-in does not have is dropped; the newest of each kind is the one shown.
        let latest: Vec<(String, bool)> = s.latest_evidence().into_iter().map(|e| (e.kind, e.ok)).collect();
        assert_eq!(latest, [("test".to_owned(), true), ("build".to_owned(), false)]);
    }

    #[test]
    fn another_version_session_or_garbage_is_refused() {
        assert_eq!(parse(SNAP.as_bytes(), "other"), Err(Refusal::OtherSession));
        assert_eq!(parse(SNAP.replace("\"v\":1", "\"v\":2").as_bytes(), "abc-1"), Err(Refusal::Version));
        assert_eq!(parse(SNAP.replace("\"v\":1", "\"v\":\"1\"").as_bytes(), "abc-1"), Err(Refusal::Version));
        assert_eq!(parse(br#"{"session":"s"}"#, "s"), Err(Refusal::Version));
        assert_eq!(parse(b"{", "s"), Err(Refusal::NotJson));
        assert_eq!(parse(b"[]", "s"), Err(Refusal::NotJson));
        assert!(parse(br#"{"v":1,"session":"s"}"#, "s").is_ok(), "a bare snapshot is fine; a field of the wrong type is a default");
        let odd = parse(br#"{"v":1,"session":"s","task":{"goal":5,"doneConditions":"x"},"guard":{"profile":"weird","rules":"many"},"asks":3}"#, "s").expect("parsed");
        assert_eq!((odd.goal.as_str(), odd.done.len(), odd.profile, odd.rules, odd.open_asks), ("", 0, Profile::Unknown, 0, 0));
    }

    #[test]
    fn text_and_lists_are_cut() {
        let long = "x".repeat(900);
        let many: Vec<String> = (0..100).map(|i| format!(r#"{{"text":"c{i}","status":"pending"}}"#)).collect();
        let text = format!(r#"{{"v":1,"session":"s","task":{{"goal":"{long}","nonGoals":[{}],"doneConditions":[{}]}}}}"#, vec![r#""n""#; 50].join(","), many.join(","));
        let s = parse(text.as_bytes(), "s").expect("parsed");
        assert_eq!((s.goal.chars().count(), s.non_goals.len(), s.done.len()), (500, MAX_LIST, MAX_DONE));
    }

    #[test]
    fn a_file_is_read_only_when_it_is_a_plain_file_of_the_right_size() {
        let dir = scratch();
        let state = dir.path();
        assert_eq!(read(state, "nope"), Snapshot::Missing);
        assert_eq!(read(state, "../x"), Snapshot::Refused(Refusal::BadId));
        std::fs::write(state.join("abc-1.json"), SNAP).unwrap();
        assert!(matches!(read(state, "abc-1"), Snapshot::Found(_)));
        // Another session's file is not taken for this one; a file over the limit is not parsed.
        std::fs::write(state.join("other.json"), SNAP).unwrap();
        assert_eq!(read(state, "other"), Snapshot::Refused(Refusal::OtherSession));
        let pad = "x".repeat(MAX_BYTES as usize);
        std::fs::write(state.join("big.json"), format!("{{\"v\":1,\"session\":\"big\",\"pad\":\"{pad}\"}}")).unwrap();
        assert_eq!(read(state, "big"), Snapshot::Refused(Refusal::TooBig));
        // A file within the plug-in's own limit is read.
        let fit = format!("{{\"v\":1,\"session\":\"fit\",\"pad\":\"{}\"}}", "x".repeat(MAX_BYTES as usize - 40));
        assert!(fit.len() as u64 <= MAX_BYTES);
        std::fs::write(state.join("fit.json"), fit).unwrap();
        assert!(matches!(read(state, "fit"), Snapshot::Found(_)));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_a_folder_or_a_pipe_in_place_of_the_file_is_refused() {
        let dir = scratch();
        let state = dir.path();
        std::fs::write(state.join("real.json"), SNAP).unwrap();
        std::os::unix::fs::symlink(state.join("real.json"), state.join("link.json")).unwrap();
        assert_eq!(read(state, "link"), Snapshot::Refused(Refusal::NotRegular));
        std::fs::create_dir(state.join("dir.json")).unwrap();
        assert_eq!(read(state, "dir"), Snapshot::Refused(Refusal::NotRegular));
        let pipe = state.join("pipe.json");
        let c = std::ffi::CString::new(pipe.to_str().unwrap()).unwrap();
        // SAFETY: mkfifo with a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        // Would block for good if it were opened for reading.
        assert_eq!(read(state, "pipe"), Snapshot::Refused(Refusal::NotRegular));
    }

    #[test]
    fn other_files_in_the_folder_are_not_looked_at() {
        let dir = scratch();
        std::fs::write(dir.path().join("abc-1.json"), SNAP).unwrap();
        std::fs::write(dir.path().join("other-name.json"), r#"{"v":1,"session":"abc-1","task":{"goal":"WRONG"}}"#).unwrap();
        match read(dir.path(), "abc-1") {
            Snapshot::Found(s) => assert_eq!(s.goal, "ship the page"),
            other => panic!("{other:?}"),
        }
    }
    #[test]
    fn the_team_settings_come_from_the_snapshot_and_a_missing_value_stays_missing() {
        let team = |body: &str| parse(SNAP.replace(r#""team":{"enabled":true}"#, &format!(r#""team":{body}"#)).as_bytes(), "abc-1").expect("parse").team;
        // The sample's own team object: on, nothing else said, so no role has a model.
        let plain = parse(SNAP.as_bytes(), "abc-1").expect("parse").team.expect("team");
        assert!(plain.enabled && !plain.codex);
        assert!(plain.roles.iter().all(|(_, m, e)| m.is_none() && e.is_none()), "a role the plug-in did not name is not given a default");
        let full = team(r#"{"enabled":false,"codex":true,"roles":{"lead":"sonnet","explorer":null,"advisor":"opus[1m]"},"efforts":{"lead":"default","explorer":null,"advisor":"max"}}"#).expect("team");
        assert!(!full.enabled && full.codex);
        assert_eq!(full.roles[0], ("lead", Some("sonnet".to_owned()), Some("default".to_owned())));
        assert_eq!(full.roles[1], ("explorer", None, None));
        assert_eq!(full.roles[2], ("advisor", Some("opus[1m]".to_owned()), Some("max".to_owned())));
        // Not the plug-in's shape: no team at all.
        assert!(team(r#"{"enabled":"true"}"#).is_none());
        assert!(team(r#"{"roles":{"lead":"sonnet"}}"#).is_none());
        assert!(team("[]").is_none());
        assert!(team("null").is_none());
        // A model that is too long or has odd characters, and an effort that is not a plain word, are no value.
        let odd = team(&format!(r#"{{"enabled":true,"roles":{{"lead":"{}","explorer":"a b","advisor":"x\u001b"}},"efforts":{{"lead":"HIGH","explorer":"{}"}}}}"#, "m".repeat(41), "e".repeat(17))).expect("team");
        assert!(odd.roles.iter().all(|(_, m, e)| m.is_none() && e.is_none()));
        // A snapshot without a team key (an older plug-in).
        let none = parse(SNAP.replace(r#""team":{"enabled":true},"#, "").as_bytes(), "abc-1").expect("parse");
        assert!(none.team.is_none());
    }
}
