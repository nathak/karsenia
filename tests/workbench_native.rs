//! The app's own workbench: what the person set is added to the message for the model (not to the transcript), the SmartWorkbench
//! plug-in is switched off for the chat's process, and nothing of it happens while the switch is off.
use std::time::Duration;

use tenrec::server::{start, Config};
use tenrec::workbench::Op;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

async fn idle(rx: &mut tokio::sync::broadcast::Receiver<tenrec::session::Envelope>) {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap().unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            return;
        }
    }
}

#[tokio::test]
async fn a_locked_contract_and_pins_reach_the_model_but_not_the_transcript_and_the_plugin_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let scenario = "{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n{\"$wait\":\"user\"}\n";
    std::fs::write(root.join("scenario.jsonl"), scenario).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("notes.txt"), "pinned text\n").unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: Some(wrapper.clone()), extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let reg = &server.registry;

    // Off (the default): nothing is added and the plug-in is not switched off.
    let chat = reg.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    reg.workbench_apply(&chat.sid, Op::SetGoal { text: "ship it".into() }).unwrap();
    reg.workbench_apply(&chat.sid, Op::Lock).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("first".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    chat.stop();
    let log = std::fs::read_to_string(root.join("fake.log")).unwrap();
    assert!(!log.contains("workbench_contract") && !log.contains("enabledPlugins"), "{log}");

    // On: the contract and the pins are added to the line sent to claude, the transcript shows the words only.
    reg.settings().update(|s| s.workbench_native = true);
    let chat = reg.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    reg.workbench_apply(&chat.sid, Op::SetGoal { text: "ship it".into() }).unwrap();
    reg.workbench_apply(&chat.sid, Op::AddConstraint { text: "no new deps".into(), hard: true }).unwrap();
    reg.workbench_apply(&chat.sid, Op::PinFile { path: "notes.txt".into(), from: None, to: None }).unwrap();
    reg.workbench_apply(&chat.sid, Op::Lock).unwrap();
    assert!(reg.workbench_apply(&chat.sid, Op::SetGoal { text: "x".into() }).is_err(), "locked");
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("second".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    let log = std::fs::read_to_string(root.join("fake.log")).unwrap();
    let line = log.lines().find(|l| l.starts_with("IN ") && l.contains("workbench_contract")).expect("the contract was sent");
    assert!(line.contains("ship it") && line.contains("no new deps") && line.contains("pinned text") && line.contains("workbench_pins"), "{line}");
    assert!(log.lines().any(|l| l.starts_with("ARGV ") && l.contains("--settings") && l.contains("\"smartworkbench@smart-workbench\":false")), "{log}");
    let (replay, _) = chat.subscribe(0).await.unwrap();
    let shown: Vec<_> = replay.iter().filter(|e| e.kind == "user").collect();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].data["text"], "second");
    assert!(!shown[0].data.to_string().contains("workbench"), "the transcript has the person's words only");

    // Deleting the chat removes its workbench file.
    chat.stop();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let file = tenrec::workbench::store::path_of(&root.join("data"), &chat.sid);
    assert!(file.exists());
    reg.delete(&chat.sid).await.unwrap();
    assert!(!file.exists());
}

#[tokio::test]
async fn a_command_goes_to_the_program_alone_so_it_is_run_and_not_read_as_a_question() {
    // Measured with claude 2.1.326: `/cost` as the only block is run by the program itself (no model call); with one more text block
    // after it the whole message is a prompt for the model (`/compact` then gets an answer instead of running).
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let turn = "{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n";
    std::fs::write(root.join("scenario.jsonl"), turn.repeat(5)).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: Some(wrapper.clone()), extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let reg = &server.registry;
    reg.settings().update(|s| s.workbench_native = true);
    let chat = reg.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    reg.workbench_apply(&chat.sid, Op::SetGoal { text: "ship it".into() }).unwrap();
    reg.workbench_apply(&chat.sid, Op::Lock).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();

    let note = tenrec::claude::protocol::Image { media_type: "text/plain".into(), data: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"some notes\n"), name: Some("n.txt".into()) };
    chat.send("/cost".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    chat.send("/my-skill arg".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    chat.send("/home/me/a.rs fix this".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    chat.send("/cost".into(), vec![note]).await.unwrap();
    idle(&mut rx).await;
    chat.send("a question".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    let log = std::fs::read_to_string(root.join("fake.log")).unwrap();
    let lines: Vec<&str> = log.lines().filter(|l| l.starts_with("IN ")).collect();
    assert_eq!(lines.len(), 5, "{log}");
    let blocks = |line: &str| serde_json::from_str::<serde_json::Value>(line.trim_start_matches("IN ")).unwrap()["message"]["content"].as_array().unwrap().len();
    // A command (a skill or plug-in command too) is sent alone.
    for line in &lines[..2] {
        assert_eq!(blocks(line), 1, "a command is sent alone: {line}");
        assert!(!line.contains("workbench_contract"), "{line}");
    }
    assert!(lines[0].contains("\"text\":\"/cost\"") && lines[1].contains("/my-skill arg"));
    // A path is not a command, a command sent with a file is a prompt anyway, and an ordinary message: each keeps the contract.
    for line in &lines[2..] {
        assert!(line.contains("workbench_contract") && line.contains("ship it"), "{line}");
    }
}

#[tokio::test]
async fn switching_the_workbench_on_restarts_a_running_process_at_the_next_message() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let scenario = "{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n{\"$wait\":\"user\"}\n";
    std::fs::write(root.join("scenario.jsonl"), scenario).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: Some(wrapper.clone()), extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("one".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    server.registry.settings().update(|s| s.workbench_native = true);
    chat.send("two".into(), vec![]).await.unwrap();
    for _ in 0..60 {
        if std::fs::read_to_string(root.join("fake.log")).unwrap_or_default().matches("ARGV").count() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let log = std::fs::read_to_string(root.join("fake.log")).unwrap();
    let argvs: Vec<&str> = log.lines().filter(|l| l.starts_with("ARGV ")).collect();
    assert_eq!(argvs.len(), 2, "a new process was started: {log}");
    assert!(!argvs[0].contains("enabledPlugins") && argvs[1].contains("enabledPlugins"), "{argvs:?}");
}

#[tokio::test]
async fn without_the_apps_binary_or_in_a_container_the_plugin_is_left_on() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("scenario.jsonl"), "{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n{\"$wait\":\"user\"}\n").unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    // No binary known (no hook can be made): the switch changes nothing, so a chat never has neither the plug-in nor the guard.
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    server.registry.settings().update(|s| s.workbench_native = true);
    let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("hi".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    let log = std::fs::read_to_string(root.join("fake.log")).unwrap();
    assert!(!log.contains("enabledPlugins") && !log.contains("PreToolUse"), "{log}");
}
