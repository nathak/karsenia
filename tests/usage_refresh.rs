//! Asking the program for the usage now: its `/usage` answer replaces a window the stream saw long ago, at most one ask runs per
//! account and not within a minute of the last, an answer that is not understood leaves the numbers as they were, and a Codex account
//! has no Claude login to ask.
use chrono::{Duration, Local};
use serde_json::json;
use tenrec::accounts::AccountKind;
use tenrec::server::{start, Config};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

fn when(hours: i64) -> String {
    (Local::now() + Duration::hours(hours)).format("%b %-d, %-I:%M%P").to_string()
}

#[tokio::test]
async fn asking_for_the_usage_replaces_an_old_window_and_is_held_to_one_a_minute() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let usage_file = root.join("usage.txt");
    std::fs::write(&usage_file, format!("Current session: 100% used · resets {} (Asia/Seoul)\nCurrent week (all models): 32% used · resets {} (Asia/Seoul)", when(2), when(24 * 3))).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_USAGE='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", usage_file.display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let reg = &server.registry;

    // The stream saw a window that has long ended: the card would say "reset".
    let now = tenrec::now_ms();
    reg.usage().record(None, &json!({"unifiedWindows": {"five_hour": {"utilization": 0.3, "resetsAt": now / 1000 - 7200, "status": "allowed"}}}), now - 10_000_000);
    assert!(reg.usage().snapshot()[""]["five_hour"].resets_at < now / 1000);

    assert_eq!(reg.refresh_usage(None).await, Ok(true));
    let snap = reg.usage().snapshot();
    let five = &snap[""]["five_hour"];
    assert_eq!(five.util, Some(1.0));
    assert!(five.resets_at > now / 1000, "the window the program reports is the current one");
    assert_eq!(snap[""]["seven_day"].util, Some(0.32));
    let log = std::fs::read_to_string(root.join("fake.log")).unwrap();
    assert!(log.contains("USAGE_ASKED") && log.contains("--no-session-persistence"), "{log}");

    // Not again at once.
    assert_eq!(reg.refresh_usage(None).await, Err("soon"));

    // An extra account is its own: an answer that says nothing understood keeps what it had.
    let b = reg.add_account("b", AccountKind::Claude).unwrap();
    reg.usage().record(Some(&b.id), &json!({"unifiedWindows": {"five_hour": {"utilization": 0.4, "resetsAt": now / 1000 + 3600, "status": "allowed"}}}), now);
    std::fs::write(&usage_file, "Hello, nothing about limits here").unwrap();
    assert_eq!(reg.refresh_usage(Some(&b.id)).await, Err("unreadable"));
    assert_eq!(reg.usage().snapshot()[&b.id]["five_hour"].util, Some(0.4));

    // A Codex account has no Claude login to ask.
    let c = reg.add_account("c", AccountKind::Codex).unwrap();
    assert_eq!(reg.refresh_usage(Some(&c.id)).await, Err("failed"));
}
