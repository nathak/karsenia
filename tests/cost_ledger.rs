//! The cost of each turn is the difference of the program's running totals, added to the day's sum of the account, kept across a
//! restart (the baseline comes from the chat's log) and never counted twice; a chat of a provider counts under the provider.
use std::time::Duration;

use tenrec::server::{start, Config};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

fn result(total: f64) -> String {
    format!("{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"total_cost_usd\":{total}}}\n")
}

/// A fake program that answers each message with a result of the next running total.
fn wrapper(root: &std::path::Path, name: &str, totals: &[f64]) -> String {
    let file = root.join(format!("{name}.jsonl"));
    let mut scenario = String::from("{\"$wait\":\"user\"}\n{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"S1\"}\n");
    for (i, total) in totals.iter().enumerate() {
        if i > 0 {
            scenario.push_str("{\"$wait\":\"user\"}\n");
        }
        scenario.push_str(&result(*total));
    }
    std::fs::write(&file, scenario).unwrap();
    let path = root.join(name);
    std::fs::write(&path, format!("#!/bin/sh\nFAKE_SCENARIO='{}' exec '{}' \"$@\"\n", file.display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

async fn idle(rx: &mut tokio::sync::broadcast::Receiver<tenrec::session::Envelope>) {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.expect("an event in time").unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            return;
        }
    }
}

fn today(server: &tenrec::server::Running) -> f64 {
    server.registry.costs().snapshot().get(&tenrec::costs::local_day()).and_then(|d| d.get("")).copied().unwrap_or(0.0)
}

#[tokio::test]
async fn a_turns_cost_is_the_difference_of_running_totals_and_a_restart_does_not_count_twice() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let cfg = |bin: String| Config { port: 0, data_dir: root.join("data"), bin: Some(bin), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() };

    // One process, three turns with totals 0.1, 0.3, 0.6: the day holds 0.6, the chat's own events tell each turn.
    let first = wrapper(&root, "first", &[0.1, 0.3, 0.6]);
    let server = start(cfg(first)).await.unwrap();
    let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    let sid = chat.sid.clone();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    for text in ["a", "b", "c"] {
        chat.send(text.into(), vec![]).await.unwrap();
        idle(&mut rx).await;
    }
    assert!((today(&server) - 0.6).abs() < 1e-9, "{}", today(&server));
    let (replay, _) = chat.subscribe(0).await.unwrap();
    let turns: Vec<f64> = replay.iter().filter(|e| e.kind == "cost").map(|e| e.data["turn"].as_f64().unwrap()).collect();
    assert_eq!(turns.len(), 3);
    assert!((turns[0] - 0.1).abs() < 1e-9 && (turns[1] - 0.2).abs() < 1e-9 && (turns[2] - 0.3).abs() < 1e-9, "{turns:?}");
    let last = replay.iter().rev().find(|e| e.kind == "cost").unwrap();
    assert!((last.data["chat"].as_f64().unwrap() - 0.6).abs() < 1e-9);
    chat.stop();
    drop(server);

    // The app is started again and the chat is resumed: the new process's first total (0.9) includes the 0.6 it restored, so the turn
    // is 0.3, not 0.9. A replay of the old log counts nothing.
    let second = wrapper(&root, "second", &[0.9]);
    let server = start(cfg(second)).await.unwrap();
    assert!((today(&server) - 0.6).abs() < 1e-9, "opening the log again adds nothing: {}", today(&server));
    let chat = server.registry.get(&sid).expect("the chat is back");
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("d".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    assert!((today(&server) - 0.9).abs() < 1e-9, "{}", today(&server));
    chat.stop();
    drop(server);

    // A program that does not carry the spend over (an older one) begins again at its own total: 0.2 is the turn.
    let third = wrapper(&root, "third", &[0.2]);
    let server = start(cfg(third)).await.unwrap();
    let chat = server.registry.get(&sid).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("e".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    assert!((today(&server) - 1.1).abs() < 1e-9, "{}", today(&server));
}

/// A session handed to another account and fast-forwarded back brings that account's spend with it (the running total of the program
/// then includes it): the baseline moves there, so the next turn is only its own cost and the ledger does not count the spend twice.
#[tokio::test]
async fn a_session_that_comes_back_from_another_account_does_not_count_that_spend_again() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let bin = wrapper(&root, "absorb", &[0.1, 5.3]);
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(bin), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("a".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    assert!((today(&server) - 0.1).abs() < 1e-9);
    // The other account's turns, taken in: its results end at a running total of 5.0 (already counted there).
    let events = vec![(1u64, "claude".to_owned(), serde_json::json!({"type": "result", "subtype": "success", "is_error": false, "total_cost_usd": 5.0}))];
    chat.absorb(events, tenrec::session::Handed { parent: "other".into(), parent_hash: 1 }).await.unwrap();
    assert!((today(&server) - 0.1).abs() < 1e-9, "taking events in adds nothing to the ledger");
    chat.send("b".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    // The program reports 5.3: the turn is 0.3.
    assert!((today(&server) - 0.4).abs() < 1e-9, "{}", today(&server));
}
