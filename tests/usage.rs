//! The limits a chat's `claude` reports (`rate_limit_event`) are kept for the account the chat belongs to, never for a provider chat,
//! survive a restart and go with a removed account.
use std::path::PathBuf;
use std::time::Duration;

use tenrec::providers::Form;
use tenrec::registry::Registry;
use tenrec::session::SessionHandle;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");
fn event() -> String {
    let soon = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 3600;
    format!(r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"allowed","rateLimitType":"five_hour","unifiedWindows":{{"five_hour":{{"utilization":0.85,"resetsAt":{soon},"status":"allowed_warning"}},"seven_day":{{"utilization":0.1,"resetsAt":{},"status":"allowed"}}}}}}}}"#, soon + 86400)
}

struct Fx {
    _dir: tempfile::TempDir,
    root: PathBuf,
    registry: Registry,
}

fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let event = event();
    std::fs::write(root.join("scenario.jsonl"), format!("{{\"$wait\":\"user\"}}\n{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"U1\"}}\n{event}\n{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}}\n{{\"$wait\":\"user\"}}\n")).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(root.join("p")).unwrap();
    let registry = Registry::open(root.join("data"), Some(wrapper.display().to_string())).unwrap();
    Fx { _dir: dir, root, registry }
}

async fn turn(chat: &SessionHandle) {
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("hello".into(), vec![]).await.unwrap();
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.expect("timed out").expect("the stream ended");
        if got.kind == "state" && got.data["state"] == "idle" {
            return;
        }
    }
}

#[tokio::test]
async fn an_account_chats_limits_are_kept_for_that_account_and_survive_a_restart_and_go_with_it() {
    let fx = fixture();
    let folder = fx.root.join("p");
    let id = fx.registry.add_account("Work").unwrap().id;
    assert!(fx.registry.usage().snapshot().is_empty());

    let chat = fx.registry.create_in(folder.to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    turn(&chat).await;
    let snap = fx.registry.usage().snapshot();
    assert_eq!(snap[&id]["five_hour"].util, Some(0.85));
    assert_eq!(snap[&id]["five_hour"].status, "allowed_warning");
    assert_eq!(snap[&id]["seven_day"].util, Some(0.1));
    assert!(!snap.contains_key(""), "the default account was not touched");

    // The numbers are in a private file and nothing else is.
    let file = fx.root.join("data").join("usage.json");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("five_hour") && !text.contains("hello"), "{text}");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);

    // A default chat counts for the default account.
    let plain = fx.registry.create_in(folder.to_str().unwrap(), None, None, false, None).unwrap();
    turn(&plain).await;
    assert_eq!(fx.registry.usage().snapshot()[""]["five_hour"].util, Some(0.85));

    // After a restart the numbers are back; an account removed since is not.
    chat.stop();
    plain.stop();
    drop(fx.registry);
    let reopened = Registry::open(fx.root.join("data"), None).unwrap();
    assert_eq!(reopened.usage().snapshot()[&id]["five_hour"].util, Some(0.85));
    let warnings = reopened.delete_account(&id).await.unwrap();
    let _ = warnings;
    assert!(!reopened.usage().snapshot().contains_key(&id));
    assert!(!std::fs::read_to_string(&file).unwrap().contains(&id), "the removal was saved");
    assert!(reopened.usage().snapshot().contains_key(""));
}

#[tokio::test]
async fn a_provider_chat_has_no_anthropic_limits_to_report() {
    let fx = fixture();
    let provider = fx.registry.providers().save(Form { name: "gw".into(), base_url: "https://gw.example.com/x".into(), token: Some("sk-test-token-9999".into()), model: "gw-model".into(), ..Default::default() }, 1).unwrap();
    let chat = fx.registry.create(fx.root.join("p").to_str().unwrap(), None, Some(&provider)).unwrap();
    turn(&chat).await;
    assert!(fx.registry.usage().snapshot().is_empty(), "{:?}", fx.registry.usage().snapshot());
    assert!(!fx.root.join("data").join("usage.json").exists());
}
