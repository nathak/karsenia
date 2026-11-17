//! What an extra account's `claude` inherits from the host's own environment. It changes this process's environment, so it is the only
//! test in its file (tests of one file share a process and run side by side).
use std::time::Duration;

use serde_json::{json, Value};
use tenrec::server::{start, Config};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

#[tokio::test]
async fn the_default_accounts_credentials_and_redirects_in_the_hosts_environment_never_reach_an_account_chat_its_status_check_or_its_runs() {
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
        for (k, v) in [
            ("ANTHROPIC_API_KEY", "acct-api-key-111"),
            ("ANTHROPIC_AUTH_TOKEN", "acct-auth-token-222"),
            ("ANTHROPIC_BASE_URL", "https://evil.example.com"),
            ("ANTHROPIC_MODEL", "personal-model"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "acct-oauth-444"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("AWS_BEARER_TOKEN_BEDROCK", "acct-aws-555"),
            ("CLAUDE_CONFIG_DIR", "/home/someone/.claude"),
            // What a work network needs stays.
            ("HTTPS_PROXY", "http://proxy.work.example:3128"),
            ("NODE_EXTRA_CA_CERTS", "/etc/work-ca.pem"),
        ] {
            std::env::set_var(k, v);
        }
    }
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let id = server.registry.add_account("Work").unwrap().id;
    let folder = root.join("p");
    std::fs::create_dir_all(&folder).unwrap();

    let run = |account: Option<String>| {
        let server = &server;
        let folder = folder.clone();
        async move {
            let chat = server.registry.create_in(folder.to_str().unwrap(), None, None, false, account.as_deref()).unwrap();
            let (_, mut rx) = chat.subscribe(0).await.unwrap();
            chat.send("hello".into(), vec![]).await.unwrap();
            loop {
                let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap().unwrap();
                if got.kind == "state" && got.data["state"] == "idle" {
                    break;
                }
            }
            chat.stop();
        }
    };
    let starts = || -> Vec<Value> { std::fs::read_to_string(&env_log).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect() };

    run(Some(id.clone())).await;
    let all = starts();
    let config = root.join("data").join("accounts").join(&id).canonicalize().unwrap().display().to_string();
    // Both the status question and the chat itself were started as the account.
    assert_eq!(all.iter().filter(|v| v["argv"][0] == "auth").count(), 1, "the login status was asked once");
    for start in &all {
        let shown = start["env"].to_string();
        for leaked in ["acct-api-key-111", "acct-auth-token-222", "evil.example.com", "personal-model", "acct-oauth-444", "acct-aws-555", "/home/someone/.claude"] {
            assert!(!shown.contains(leaked), "{leaked} reached an account's process: {shown}");
        }
        assert_eq!(start["env"]["CLAUDE_CONFIG_DIR"], json!(config), "every process of the account runs in its own settings folder");
        assert!(start["env"].get("CLAUDE_CODE_USE_BEDROCK").is_none());
    }

    // The control: a default chat gets the environment as it is, so the check above is able to fail.
    run(None).await;
    let normal = starts().into_iter().filter(|v| v["argv"][0] != "auth").nth(1).unwrap()["env"].clone();
    assert_eq!(normal["ANTHROPIC_API_KEY"], json!("acct-api-key-111"));
    assert_eq!(normal["CLAUDE_CONFIG_DIR"], json!("/home/someone/.claude"));
}
