//! A chat that belongs to an account and uses a provider that takes the account's login: it can turn the gateway on and off between turns
//! and go on with the same conversation (`--resume`), and what was said through the gateway is marked. Run against a fake `claude`.
use std::time::Duration;

use serde_json::Value;
use tenrec::providers::{Auth, Form};
use tenrec::server::{start, Config};
use tenrec::session::SessionHandle;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

async fn turn(chat: &SessionHandle, text: &str) {
    let (_, mut rx) = chat.subscribe(u64::MAX).await.unwrap();
    chat.send(text.into(), vec![]).await.unwrap();
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap().unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            break;
        }
    }
}

#[tokio::test]
async fn a_gateway_can_be_turned_on_and_off_in_a_chat_of_an_account_and_the_conversation_goes_on() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(
        root.join("scenario.jsonl"),
        "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"csid-1\",\"model\":\"fake\"}\n{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"session_id\":\"csid-1\"}\n{\"$wait\":\"user\"}\n",
    )
    .unwrap();
    let env_log = root.join("env.log");
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_ENV_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), env_log.display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let providers = server.registry.providers();
    let login = providers.save(Form { name: "Air".into(), base_url: "https://gw.example.com/x".into(), auth: Auth::AccountLogin, token: Some("airkey-123456".into()), headers: "AIR-Api-Key: {key}".into(), ..Default::default() }, 1).unwrap();
    let plain = providers.save(Form { name: "Plain".into(), base_url: "https://other.example.com".into(), token: Some("plain-key-123456".into()), ..Default::default() }, 1).unwrap();
    let folder = root.join("p");
    std::fs::create_dir_all(&folder).unwrap();
    let starts = || -> Vec<Value> { std::fs::read_to_string(&env_log).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect() };
    let has = |start: &Value, flag: &str| start["argv"].as_array().is_some_and(|a| a.iter().any(|x| x == flag));

    let chat = server.registry.create(folder.to_str().unwrap(), None, None).unwrap();
    turn(&chat, "first, direct").await;
    assert!(starts()[0]["env"].get("ANTHROPIC_BASE_URL").is_none(), "no gateway yet");

    // The gateway goes on: the same conversation, the gateway's address and headers, no credential variable.
    chat.set_provider(Some(login.clone())).await.unwrap();
    turn(&chat, "second, through the gateway").await;
    let second = starts()[1].clone();
    assert!(has(&second, "--resume") && has(&second, "--setting-sources"), "{second}");
    assert_eq!(second["env"]["ANTHROPIC_BASE_URL"], "https://gw.example.com/x");
    assert_eq!(second["env"]["ANTHROPIC_CUSTOM_HEADERS"], "AIR-Api-Key: airkey-123456");
    assert!(second["env"].get("ANTHROPIC_AUTH_TOKEN").is_none() && second["env"].get("ANTHROPIC_API_KEY").is_none());
    let info = chat.info().await.unwrap();
    assert!(info.meta.provider.as_ref().is_some_and(|p| p.login) && info.meta.account.is_none(), "{:?}", info.meta.provider);

    // Off again: the same conversation, no gateway.
    chat.set_provider(None).await.unwrap();
    turn(&chat, "third, direct again").await;
    let third = starts()[2].clone();
    assert!(has(&third, "--resume"));
    assert!(third["env"].get("ANTHROPIC_BASE_URL").is_none());

    // What was said through the gateway is marked, the rest is not.
    let (events, _) = chat.subscribe(0).await.unwrap();
    let marked = |text: &str| events.iter().find(|e| e.kind == "user" && e.data["text"] == text).map(|e| e.data["via_gateway"] == true);
    assert_eq!((marked("first, direct"), marked("second, through the gateway"), marked("third, direct again")), (Some(false), Some(true), Some(false)));
    assert!(events.iter().any(|e| e.kind == "provider"), "the switch is in the log");
    let texts = chat.user_texts(0, 0).await.unwrap();
    assert!(texts.iter().all(|(_, t)| !t.contains("through the gateway")) && texts.len() == 2, "auto-save does not read what went through the gateway: {texts:?}");

    // Refused: an ordinary provider, a made-up one, and a provider chat that stays with its provider.
    assert!(chat.set_provider(Some(plain.clone())).await.is_err());
    assert!(chat.set_provider(Some("nope".into())).await.is_err());
    let stuck = server.registry.create(folder.to_str().unwrap(), None, Some(&plain)).unwrap();
    assert!(stuck.set_provider(None).await.is_err(), "an ordinary provider chat keeps its provider");
    assert!(stuck.set_provider(Some(login.clone())).await.is_err());

    // A chat made with the login provider belongs to its account (here the default one) and has the realm of it.
    let born = server.registry.create(folder.to_str().unwrap(), None, Some(&login)).unwrap();
    let born_info = born.info().await.unwrap();
    assert!(born_info.meta.account.is_none() && born_info.meta.provider.as_ref().is_some_and(|p| p.login));
    assert!(born.set_provider(None).await.is_ok(), "and it can turn the gateway off");
    // A container chat of such a provider is refused.
    assert!(server.registry.create_in(folder.to_str().unwrap(), None, Some(&login), true, None).is_err());
}
