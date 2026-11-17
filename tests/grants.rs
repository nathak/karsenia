//! "Allow for this session" / "always allow": the rule that goes back to `claude` is built by the host from the request's own
//! suggestion (shapes measured with Claude Code 2.1.325), never from anything a page sends, and a refused rule leaves the request open.
use std::time::Duration;

use serde_json::{json, Value};
use tenrec::server::{start, Config};
use tenrec::session::Grant;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

const BASH: &str = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"touch probe-file.txt"},"permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"touch probe-file.txt"}],"behavior":"allow","destination":"localSettings"},{"type":"addDirectories","directories":["/p"],"destination":"session"},{"type":"setMode","mode":"acceptEdits","destination":"session"}],"tool_use_id":"t1"}}"#;
const WRITE: &str = r#"{"type":"control_request","request_id":"r2","request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write","input":{"file_path":"/p/a.txt","content":"x"},"permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}],"server_policy_ask_only":true,"tool_use_id":"t2"}}"#;
const ASK_ONLY_BASH: &str = r#"{"type":"control_request","request_id":"r3","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"ls"},"permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"ls"}],"behavior":"allow","destination":"localSettings"}],"server_policy_ask_only":true,"tool_use_id":"t3"}}"#;

async fn wait_event(rx: &mut tokio::sync::broadcast::Receiver<tenrec::session::Envelope>, kind: &str) -> Value {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.expect("timed out").unwrap();
        if got.kind == kind {
            return got.data;
        }
    }
}

#[tokio::test]
async fn a_rule_to_remember_is_built_from_claudes_own_suggestion_and_a_refusal_leaves_the_request_open() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let scenario = format!("{{\"$wait\":\"user\"}}\n{BASH}\n{{\"$wait\":\"control_response\"}}\n{WRITE}\n{{\"$wait\":\"control_response\"}}\n{ASK_ONLY_BASH}\n{{\"$wait\":\"control_response\"}}\n{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}}\n{{\"$wait\":\"user\"}}\n");
    std::fs::write(root.join("scenario.jsonl"), scenario).unwrap();
    let log = root.join("fake.log");
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), log.display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("go".into(), vec![]).await.unwrap();

    // The first request: the dialogs are told what could be remembered, as text, and not how it is built.
    let ask = wait_event(&mut rx, "permission").await;
    assert_eq!(ask["request_id"], "r1");
    assert_eq!(ask["grants"], json!({"session": "Bash(touch probe-file.txt)", "always": "Bash(touch probe-file.txt)", "exact": true}));
    chat.answer("r1".into(), true, None, Grant::Always).await.unwrap();
    let resolved = wait_event(&mut rx, "permission_resolved").await;
    assert_eq!((resolved["outcome"].as_str(), resolved["grant"].as_str(), resolved["rule"].as_str()), (Some("allowed"), Some("always"), Some("Bash(touch probe-file.txt)")));

    // A request with nothing to remember: a rule is refused, the request stays, and a plain answer still works.
    let write = wait_event(&mut rx, "permission").await;
    assert_eq!(write["request_id"], "r2");
    assert_eq!(write["grants"], json!({"session": null, "always": null, "exact": false}));
    assert!(chat.answer("r2".into(), true, None, Grant::Session).await.is_err());
    assert!(chat.answer("r2".into(), true, None, Grant::Always).await.is_err());
    chat.answer("r2".into(), true, None, Grant::None).await.unwrap();
    wait_event(&mut rx, "permission_resolved").await;

    // The request that carries the undocumented ask-only flag gets no rule either, even with a good suggestion.
    let only = wait_event(&mut rx, "permission").await;
    assert_eq!(only["grants"]["session"], Value::Null);
    assert!(chat.answer("r3".into(), true, None, Grant::Session).await.is_err());
    chat.answer("r3".into(), false, Some("no".into()), Grant::Session).await.unwrap();
    wait_event(&mut rx, "permission_resolved").await;

    // The stand-in writes what it reads a moment after the host sent it.
    let (mut text, mut replies);
    let mut tries = 0;
    loop {
        text = std::fs::read_to_string(&log).unwrap();
        replies = text.lines().filter_map(|l| l.strip_prefix("IN ")).filter_map(|l| serde_json::from_str(l).ok()).filter(|v: &Value| v["type"] == "control_response").collect::<Vec<Value>>();
        tries += 1;
        if replies.len() == 3 || tries > 50 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(replies.len(), 3, "{text}");
    let first = &replies[0]["response"]["response"];
    assert_eq!(first["behavior"], "allow");
    assert_eq!(first["updatedInput"], json!({"command": "touch probe-file.txt"}), "the request's own input goes back");
    assert_eq!(first["updatedPermissions"], json!([{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "touch probe-file.txt"}], "behavior": "allow", "destination": "localSettings"}]));
    assert!(!text.contains("addDirectories") && !text.contains("setMode"), "nothing else of the suggestions is passed on");
    assert!(replies[1]["response"]["response"].get("updatedPermissions").is_none(), "an allow without a rule carries none");
    assert_eq!(replies[2]["response"]["response"]["behavior"], "deny");
    assert!(replies[2]["response"]["response"].get("updatedPermissions").is_none(), "a denial never carries a rule");
    chat.stop();
}
