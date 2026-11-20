//! The app's own team mode: with it on, the chat's `claude` gets the two helper subagents (`--agents`) and the lead's effort
//! (`--effort`); a change of a helper starts the process again with `--resume`, the transcript says so, and a provider chat has no team.
use std::time::Duration;

use tenrec::server::{start, Config};
use tenrec::workbench::team::{Effort, Sub};
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

fn argvs(root: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(root.join("fake.log")).unwrap_or_default().lines().filter(|l| l.starts_with("ARGV ")).map(str::to_owned).collect()
}

async fn wait_argvs(root: &std::path::Path, n: usize) {
    for _ in 0..80 {
        if argvs(root).len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn team_mode_passes_the_helpers_and_a_change_restarts_the_chat_with_resume() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let step = "{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n";
    std::fs::write(root.join("scenario.jsonl"), format!("{step}{step}{step}")).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: Some(wrapper.clone()), extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let reg = &server.registry;
    reg.settings().update(|s| s.workbench_native = true);

    // Off: no helpers, no effort.
    let chat = reg.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("one".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    let first = argvs(&root);
    assert!(!first[0].contains("--agents") && !first[0].contains("--effort"), "{first:?}");

    // On, with an advisor model: the running process is ended at once (the transcript says so) and the next message starts a new one that
    // carries the helpers, the effort and the one guidance line (which the transcript does not show).
    reg.team_apply(&chat.sid, Op::SetRole { role: Sub::Advisor, model: Some("sonnet".into()), effort: Some(Effort::Low) }).await.unwrap();
    reg.team_apply(&chat.sid, Op::SetLeadEffort { effort: Some(Effort::High) }).await.unwrap();
    assert_eq!(argvs(&root).len(), 1, "nothing changes for a chat while team mode is off");
    reg.team_apply(&chat.sid, Op::SetTeam { on: true }).await.unwrap();
    let (replay, _) = chat.subscribe(0).await.unwrap();
    assert!(replay.iter().any(|e| e.kind == "team" && e.data["summary"].as_str().is_some_and(|s| s.starts_with("on · lead high"))), "{:?}", replay.iter().map(|e| &e.kind).collect::<Vec<_>>());
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("two".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    wait_argvs(&root, 2).await;
    let all = argvs(&root);
    assert_eq!(all.len(), 2, "{all:?}");
    let last = &all[1];
    assert!(last.contains("--agents") && last.contains("\"explorer\"") && last.contains("\"model\":\"sonnet\"") && last.contains("\"effort\":\"low\""), "{last}");
    assert!(last.contains("--effort high"), "{last}");
    let log = std::fs::read_to_string(root.join("fake.log")).unwrap();
    assert!(log.lines().any(|l| l.starts_with("IN ") && l.contains("workbench_team")), "{log}");
    let (replay, _) = chat.subscribe(0).await.unwrap();
    assert!(!replay.iter().filter(|e| e.kind == "user").any(|e| e.data.to_string().contains("workbench_team")));

    // Refused: a bad model leaves things as they were; only team ops are taken.
    assert_eq!(reg.team_apply(&chat.sid, Op::SetRole { role: Sub::Explorer, model: Some("a b".into()), effort: None }).await.err(), Some("team_err_model"));
    assert_eq!(reg.team_apply(&chat.sid, Op::Lock).await.err(), Some("team_err_op"));
}
