//! `tenrec guard`: the PreToolUse hook of a chat. It is run as Claude Code runs it (the call on stdin, the answer on stdout).
use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use tenrec::workbench::{store, Op, Profile, Workbench};

const TENREC: &str = env!("CARGO_BIN_EXE_tenrec");

struct Setup {
    dir: tempfile::TempDir,
}

impl Setup {
    fn new(profile: Profile, paused: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut wb = Workbench::default();
        wb.apply(Op::SetProfile { profile }).unwrap();
        wb.apply(Op::SetPaused { paused }).unwrap();
        store::save(&dir.path().join("data"), "chat1", &wb).unwrap();
        std::fs::create_dir_all(dir.path().join("project/.claude")).unwrap();
        Self { dir }
    }

    fn run(&self, input: &str) -> (i32, Option<Value>) {
        let mut child = Command::new(TENREC).args(["guard", "--data", self.dir.path().join("data").to_str().unwrap(), "--project", self.dir.path().join("project").to_str().unwrap(), "--sid", "chat1"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
        let out = child.wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        (out.status.code().unwrap(), (!text.is_empty()).then(|| serde_json::from_str(&text).unwrap()))
    }

    fn call(&self, tool: &str, input: Value) -> (i32, Option<Value>) {
        let project = self.dir.path().join("project");
        self.run(&json!({"session_id": "x", "cwd": project, "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": input}).to_string())
    }
}

fn decision(answer: &(i32, Option<Value>)) -> &str {
    answer.1.as_ref().and_then(|v| v["hookSpecificOutput"]["permissionDecision"].as_str()).unwrap_or("none")
}

/// The words of a destructive git command, put together here so that this file does not itself read as one.
fn force_push() -> String {
    ["git", "push", "--force", "origin", "main"].join(" ")
}

#[test]
fn a_destructive_call_is_denied_a_send_is_asked_ordinary_work_is_left_alone_and_nothing_is_ever_allowed() {
    let s = Setup::new(Profile::Balanced, false);
    let force = s.call("Bash", json!({"command": force_push()}));
    assert_eq!((force.0, decision(&force)), (0, "deny"));
    assert!(force.1.as_ref().unwrap()["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().contains("destructive-git"));
    assert_eq!(decision(&s.call("Bash", json!({"command": "git push origin main"}))), "ask");
    assert_eq!(decision(&s.call("Read", json!({"file_path": ".env"}))), "ask");
    let quiet = s.call("Bash", json!({"command": "ls -la"}));
    assert_eq!((quiet.0, quiet.1), (0, None), "no opinion on ordinary work");
    assert_eq!(decision(&s.call("Edit", json!({"file_path": "src/a.rs"}))), "none");
}

#[test]
fn the_profile_and_the_pause_switch_change_the_answer() {
    let permissive = Setup::new(Profile::Permissive, false);
    assert_eq!(decision(&permissive.call("Bash", json!({"command": "git push origin main"}))), "none", "low skips most questions");
    assert_eq!(decision(&permissive.call("Read", json!({"file_path": ".env"}))), "ask", "but secrets are still asked");
    assert_eq!(decision(&permissive.call("Bash", json!({"command": force_push()}))), "deny");
    let strict = Setup::new(Profile::Strict, false);
    assert_eq!(decision(&strict.call("Edit", json!({"file_path": "src/a.rs"}))), "ask", "high asks medium-risk calls too");
    let paused = Setup::new(Profile::Balanced, true);
    assert_eq!(decision(&paused.call("Edit", json!({"file_path": "src/a.rs"}))), "ask");
    assert_eq!(decision(&paused.call("Read", json!({"file_path": "src/a.rs"}))), "none", "a low-risk call is not asked while paused");
    let off = Setup::new(Profile::Off, false);
    assert_eq!(decision(&off.call("Bash", json!({"command": force_push()}))), "none", "off: the built-in rules stop nothing");
}

#[test]
fn the_projects_rule_file_applies_and_cannot_loosen_a_block() {
    let s = Setup::new(Profile::Balanced, false);
    std::fs::write(
        s.dir.path().join("project/.claude/smartworkbench.json"),
        r#"{"rules":[
        {"tool":"Bash","command":"^make release","policy":"block","reason":"releases go through CI"},
        {"tool":"Bash","command":"^git push","policy":"allow"}
    ]}"#,
    )
    .unwrap();
    assert_eq!(decision(&s.call("Bash", json!({"command": "make release"}))), "deny");
    // An allow rule does not lift a block.
    assert_eq!(decision(&s.call("Bash", json!({"command": force_push()}))), "deny");
    // A file whose profile is stricter raises the chat's own.
    std::fs::write(s.dir.path().join("project/.claude/smartworkbench.json"), r#"{"profile":"strict","rules":[]}"#).unwrap();
    assert_eq!(decision(&s.call("Edit", json!({"file_path": "src/a.rs"}))), "ask");
    // A rule file that is not valid JSON would drop the block rules silently: it blocks until fixed.
    std::fs::write(s.dir.path().join("project/.claude/smartworkbench.json"), "{ \"rules\": [ ").unwrap();
    assert_eq!(decision(&s.call("Bash", json!({"command": "ls"}))), "deny");
}

#[test]
fn what_it_cannot_decide_is_denied_not_let_through() {
    let s = Setup::new(Profile::Balanced, false);
    assert_eq!(decision(&s.run("this is not json")), "deny");
    assert_eq!(decision(&s.run(r#"{"cwd":"/","tool_input":{}}"#)), "deny");
    // A workbench file that cannot be read.
    std::fs::write(store::path_of(&s.dir.path().join("data"), "chat1"), "{ not json").unwrap();
    assert_eq!(decision(&s.call("Bash", json!({"command": "ls"}))), "deny");
    // Missing arguments: exit 2, which Claude Code also treats as a block.
    let out = Command::new(TENREC).arg("guard").stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn a_rule_file_that_is_not_a_plain_file_denies_and_the_project_is_the_chats_not_the_shells_folder() {
    let s = Setup::new(Profile::Balanced, false);
    let rules = s.dir.path().join("project/.claude/smartworkbench.json");
    std::fs::write(&rules, r#"{"rules":[{"tool":"Bash","command":"^make release","policy":"block","reason":"through CI"}]}"#).unwrap();
    // The shell has moved into a subfolder: the rules of the project still apply.
    std::fs::create_dir_all(s.dir.path().join("project/src")).unwrap();
    let moved = s.run(&json!({"cwd": s.dir.path().join("project/src"), "tool_name": "Bash", "tool_input": {"command": "make release"}}).to_string());
    assert_eq!(decision(&moved), "deny");
    #[cfg(unix)]
    {
        // A pipe in the place of the file would make a hook wait: it is a block at once.
        std::fs::remove_file(&rules).unwrap();
        assert!(Command::new("mkfifo").arg(&rules).status().unwrap().success());
        let started = std::time::Instant::now();
        assert_eq!(decision(&s.call("Bash", json!({"command": "ls"}))), "deny");
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        std::fs::remove_file(&rules).unwrap();
        std::os::unix::fs::symlink("/dev/zero", &rules).unwrap();
        assert_eq!(decision(&s.call("Bash", json!({"command": "ls"}))), "deny");
    }
}

#[test]
fn a_call_that_names_the_data_folder_a_question_in_a_mode_that_asks_nothing_and_a_missing_data_folder_are_denied() {
    let s = Setup::new(Profile::Balanced, false);
    let data = s.dir.path().join("data");
    let edit = s.call("Bash", json!({"command": format!("sed -i s/balanced/off/ {}/workbench/chat/chat1.json", data.display())}));
    assert_eq!(decision(&edit), "deny");
    // The same file written another way: a path that climbs out and back, a home-relative path, a glob around the file's place.
    for command in [format!("echo x > {}/../data/workbench/chat/chat1.json", data.display()), "cat ~/.local/share/tenrec/workbench/chat/*.json".to_owned(), "sed -i s/a/b/ */workbench/chat/chat1.json".to_owned()] {
        assert_eq!(decision(&s.call("Bash", json!({"command": command.clone()}))), "deny", "{command}");
    }
    let project = s.dir.path().join("project");
    let bypass = s.run(&json!({"cwd": project, "permission_mode": "bypassPermissions", "tool_name": "Read", "tool_input": {"file_path": ".env"}}).to_string());
    assert_eq!(decision(&bypass), "deny", "an ask cannot be put in bypass mode");
    let plain = s.run(&json!({"cwd": project, "permission_mode": "default", "tool_name": "Read", "tool_input": {"file_path": ".env"}}).to_string());
    assert_eq!(decision(&plain), "ask");
    let out = Command::new(TENREC).args(["guard", "--data", "/no/such/folder", "--sid", "chat1"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    out.stdin.as_ref();
    let done = out.wait_with_output().unwrap();
    // No input was sent and the pipe closed: it cannot decide.
    assert!(done.status.code() == Some(2) || String::from_utf8_lossy(&done.stdout).contains("deny"));
}

#[test]
fn input_over_the_limit_is_refused() {
    let s = Setup::new(Profile::Balanced, false);
    let big = json!({"cwd": s.dir.path().join("project"), "tool_name": "Bash", "tool_input": {"command": "a".repeat(2 * 1024 * 1024)}}).to_string();
    assert_eq!(s.run(&big).0, 2);
}
