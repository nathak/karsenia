//! The desktop window's in-process link (`server::local`): the same operations and replies as a page's WebSocket.
use std::time::Duration;

use serde_json::{json, Value};
use tenrec::server::{local, start_core, Config};
use tokio::sync::mpsc::UnboundedReceiver;

/// The next reply of `kind`, skipping the others (app-wide events arrive in between).
async fn next(rx: &mut UnboundedReceiver<String>, kind: &str) -> Value {
    let found = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(text) = rx.recv().await {
            let value: Value = serde_json::from_str(&text).expect("JSON reply");
            if value["kind"] == kind {
                return value;
            }
        }
        panic!("the link closed before {kind}");
    })
    .await;
    found.unwrap_or_else(|_| panic!("timed out waiting for {kind}"))
}

#[tokio::test]
async fn the_desktop_link_answers_like_a_page_and_ends_with_its_senders() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = start_core(Config { port: 0, data_dir: dir.path().join("data"), bin: Some("claude-not-started".into()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).expect("core");
    let (tx, mut rx) = local(registry);

    tx.send(json!({"op": "memory_add", "account": null, "rid": 1, "text": "I like short answers", "scope": "user", "kind": "preference"})).expect("send");
    let result = next(&mut rx, "memory_result").await;
    assert_eq!(result["data"]["ok"], true, "{result}");
    assert_eq!(result["data"]["rid"], 1);
    next(&mut rx, "memory_changed").await;

    tx.send(json!({"op": "memory_list", "account": null})).expect("send");
    let list = next(&mut rx, "memory").await;
    assert!(list["data"]["overview"]["user"].to_string().contains("I like short answers"), "{list}");

    // An operation that names no account is not answered from the default one.
    tx.send(json!({"op": "memory_list"})).expect("send");
    let list = next(&mut rx, "memory").await;
    assert_eq!(list["data"]["available"], false, "{list}");

    // The terminal is off unless the window was started with it on.
    tx.send(json!({"op": "term_open"})).expect("send");
    let error = next(&mut rx, "term_error").await;
    assert!(error["data"]["message"].as_str().unwrap_or_default().contains("terminal is off"), "{error}");

    tx.send(json!({"op": "attach", "sid": "nope"})).expect("send");
    let error = next(&mut rx, "error").await;
    assert!(error["data"]["message"].as_str().unwrap_or_default().contains("does not follow"), "{error}");

    drop(tx);
    let closed = tokio::time::timeout(Duration::from_secs(2), async {
        while rx.recv().await.is_some() {}
    })
    .await;
    assert!(closed.is_ok(), "the link should end once every sender is gone");
}

/// The desktop shows "Deleted, Undo" only for `ok: true`: a refused delete must say `ok: false`.
#[tokio::test]
async fn a_refused_delete_says_not_ok() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = start_core(Config { port: 0, data_dir: dir.path().join("data"), bin: Some("claude-not-started".into()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).expect("core");
    let (tx, mut rx) = local(registry);
    tx.send(json!({"op": "memory_delete", "account": null, "rid": 5, "id": 999_999})).expect("send");
    let result = next(&mut rx, "memory_result").await;
    assert_eq!(result["data"]["ok"], false, "{result}");
}

/// With the terminal on, the window gets a shell in the chat's folder, its output comes as frames, and it ends with the link.
#[cfg(unix)]
#[tokio::test]
async fn the_desktop_link_runs_a_shell_only_when_started_with_the_terminal() {
    use tenrec::server::local_with_terminal;
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).expect("project");
    let registry = start_core(Config { port: 0, data_dir: dir.path().join("data"), bin: Some("claude-not-started".into()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).expect("core");
    let (tx, mut rx, mut bin) = local_with_terminal(registry, true);

    tx.send(json!({"op": "new", "account": null, "cwd": project.display().to_string()})).expect("send");
    let created = next(&mut rx, "created").await;
    let sid = created["sid"].as_str().expect("sid").to_owned();
    tx.send(json!({"op": "term_open", "sid": sid, "req": 1, "cols": 80, "rows": 24})).expect("send");
    let opened = next(&mut rx, "term_opened").await;
    let tid = opened["data"]["tid"].as_u64().expect("tid");
    tx.send(json!({"op": "term_input", "tid": tid, "data": "echo tenrec-$((20+22))\n"})).expect("send");
    let seen = tokio::time::timeout(Duration::from_secs(10), async {
        let mut all = Vec::new();
        while let Some(frame) = bin.recv().await {
            assert_eq!(frame[0], 1, "output frames first");
            all.extend_from_slice(&frame[5..]);
            if String::from_utf8_lossy(&all).contains("tenrec-42") {
                return true;
            }
        }
        false
    })
    .await
    .expect("timed out");
    assert!(seen, "the shell's answer arrives as an output frame");
    // The shell ends when the window's link does.
    drop(tx);
    let ended = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(frame) = bin.recv().await {
            if frame[0] == 2 {
                return true;
            }
        }
        false
    })
    .await;
    assert!(matches!(ended, Ok(true)), "an exit frame follows the link's end");
}
