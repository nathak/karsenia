//! A provider that uses an account's login: its chat gets the provider's address and header lines and no credential of its own, in the
//! environment of a fake `claude`. It changes this process's environment, so it is the only test in its file.
use std::time::Duration;

use serde_json::{json, Value};
use tenrec::providers::{Auth, Form};
use tenrec::server::{start, Config};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

#[tokio::test]
async fn a_chat_of_a_login_provider_gets_the_address_and_headers_and_none_of_the_hosts_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("scenario.jsonl"), "{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n{\"$wait\":\"user\"}\n").unwrap();
    let env_log = root.join("env.log");
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_ENV_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), env_log.display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

    // SAFETY: the only test in this file, and nothing else runs yet.
    unsafe {
        for (k, v) in [("ANTHROPIC_API_KEY", "host-api-key-111"), ("ANTHROPIC_AUTH_TOKEN", "host-auth-token-222"), ("CLAUDE_CODE_OAUTH_TOKEN", "host-oauth-444"), ("CLAUDE_CONFIG_DIR", "/home/someone/.claude")] {
            std::env::set_var(k, v);
        }
    }
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let id = server
        .registry
        .providers()
        .save(Form { name: "Air".into(), base_url: "https://gw.example.com/x".into(), auth: Auth::AccountLogin, token: Some("airkey-123456".into()), headers: "AIR-Api-Key: {key}\nAIR-Coding-Agent-Mode: subscription".into(), ..Default::default() }, 1)
        .unwrap();
    let folder = root.join("p");
    std::fs::create_dir_all(&folder).unwrap();
    let chat = server.registry.create(folder.to_str().unwrap(), None, Some(&id)).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("hello".into(), vec![]).await.unwrap();
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap().unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            break;
        }
    }
    chat.stop();
    let starts: Vec<Value> = std::fs::read_to_string(&env_log).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let env = starts[0]["env"].clone();
    let shown = env.to_string();
    for leaked in ["host-api-key-111", "host-auth-token-222", "host-oauth-444"] {
        assert!(!shown.contains(leaked), "{leaked} reached the chat: {shown}");
    }
    assert!(env.get("ANTHROPIC_AUTH_TOKEN").is_none() && env.get("ANTHROPIC_API_KEY").is_none(), "no credential of the provider's own: {shown}");
    assert_eq!(env["ANTHROPIC_BASE_URL"], json!("https://gw.example.com/x"));
    assert_eq!(env["ANTHROPIC_CUSTOM_HEADERS"], json!("AIR-Api-Key: airkey-123456\nAIR-Coding-Agent-Mode: subscription"));
    // The default account's folder is where the host's environment says it is: the variable is left as it was.
    assert_eq!(env["CLAUDE_CONFIG_DIR"], json!("/home/someone/.claude"));
}
