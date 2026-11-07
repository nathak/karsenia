//! Against the real `claude` (costs a few cents, needs a login). Run on purpose:
//!   SWB_LIVE=1 cargo test --test live -- --ignored
use std::time::Duration;

use swb_app::registry::Registry;
use swb_app::session::Envelope;
use tokio::sync::broadcast::Receiver;

async fn until(rx: &mut Receiver<Envelope>, what: impl Fn(&Envelope) -> bool) -> Envelope {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(90), rx.recv()).await.expect("timed out").expect("the stream ended");
        if what(&got) {
            return got;
        }
    }
}

#[tokio::test]
#[ignore = "uses the real claude; run with SWB_LIVE=1 and --ignored"]
async fn live_a_permission_is_asked_and_an_approved_command_runs() {
    if std::env::var_os("SWB_LIVE").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let registry = Registry::open(dir.path().join("data"), None).unwrap();
    let session = registry.create(project.to_str().unwrap(), Some("haiku".into())).unwrap();
    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send("Create an empty file named live.txt in the current directory with the Bash tool (touch live.txt).".into(), vec![]).await.unwrap();

    let ask = until(&mut rx, |e| e.kind == "permission").await;
    assert_eq!(ask.data["tool_name"], "Bash");
    assert!(!project.join("live.txt").exists(), "nothing runs before the answer");
    session.permission(ask.data["request_id"].as_str().unwrap().to_owned(), true, None, false).await.unwrap();
    until(&mut rx, |e| e.kind == "state" && e.data["state"] == "idle").await;
    assert!(project.join("live.txt").exists());
}

#[tokio::test]
#[ignore = "uses the real claude; run with SWB_LIVE=1 and --ignored"]
async fn live_claude_calls_the_history_search_tool_after_a_click() {
    if std::env::var_os("SWB_LIVE").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let server = swb_app::server::start(swb_app::server::Config {
        port: 0,
        data_dir: dir.path().join("data"),
        bin: None,
        mcp_bin: Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_smart-workbench-app"))),
        extract: Default::default(),
        propose: Default::default(),
    })
    .await
    .unwrap();
    let session = server.registry.create(project.to_str().unwrap(), Some("haiku".into())).unwrap();
    let cwd = session.info().await.unwrap().meta.cwd.to_string_lossy().into_owned();

    // An earlier chat of the same project, put straight into the index.
    let old = Envelope { v: 1, sid: "earlier".into(), seq: 1, ts: 1_793_800_000_000, kind: "user".into(), data: serde_json::json!({"text": "we decided the codeword is PELICAN-NONCE-7731 for the release"}) };
    server.registry.index().unwrap().add(&cwd, 0, &old);
    server.registry.index().unwrap().flush();

    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send("Use the search_history tool to look for PELICAN-NONCE and tell me, in one sentence, what the excerpt says the codeword is.".into(), vec![]).await.unwrap();
    let ask = until(&mut rx, |e| e.kind == "permission").await;
    assert_eq!(ask.data["tool_name"], "mcp__swb_app__search_history");
    session.permission(ask.data["request_id"].as_str().unwrap().to_owned(), true, None, true).await.unwrap();

    let mut said = String::new();
    loop {
        let e = until(&mut rx, |_| true).await;
        if e.kind == "claude" && e.data["type"] == "assistant" {
            for block in e.data["message"]["content"].as_array().into_iter().flatten() {
                if let Some(t) = block["text"].as_str() {
                    said.push_str(t);
                }
            }
        }
        if e.kind == "state" && e.data["state"] == "idle" {
            break;
        }
    }
    assert!(said.contains("PELICAN-NONCE-7731"), "Claude's answer should quote the old chat: {said}");
}

#[tokio::test]
#[ignore = "uses the real claude; run with SWB_LIVE=1 and --ignored"]
async fn live_memory_a_stated_preference_is_auto_saved_and_a_new_chat_knows_it() {
    if std::env::var_os("SWB_LIVE").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let server = swb_app::server::start(swb_app::server::Config {
        port: 0,
        data_dir: dir.path().join("data"),
        bin: None,
        mcp_bin: Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_smart-workbench-app"))),
        extract: swb_app::extract::ExtractConfig { debounce: Duration::from_millis(500), batch_turns: 3, daily_cap: 40, retries: 1 },
        propose: Default::default(),
    })
    .await
    .unwrap();
    server.registry.settings().update(|s| s.memory_auto = swb_app::settings::Auto::Project);

    let first = server.registry.create(project.to_str().unwrap(), Some("haiku".into())).unwrap();
    let (_, mut rx) = first.subscribe(0).await.unwrap();
    first.send("I use pnpm as the package manager in this project. Reply with just OK.".into(), vec![]).await.unwrap();
    until(&mut rx, |e| e.kind == "state" && e.data["state"] == "idle").await;

    let store = server.registry.memory().unwrap();
    let cwd = first.info().await.unwrap().meta.cwd.to_string_lossy().into_owned();
    let mut saved = false;
    for _ in 0..120 {
        if store.overview(Some(&cwd)).project.iter().any(|m| m.text.to_lowercase().contains("pnpm")) {
            saved = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(saved, "the preference should be auto-saved: {:?}", store.overview(Some(&cwd)));

    let second = server.registry.create(project.to_str().unwrap(), Some("haiku".into())).unwrap();
    let (_, mut rx2) = second.subscribe(0).await.unwrap();
    second.send("Which package manager does this project use, according to the notes you were given? Answer with one word and do not use any tool.".into(), vec![]).await.unwrap();
    let mut said = String::new();
    let mut used_tool = false;
    loop {
        let e = until(&mut rx2, |_| true).await;
        if e.kind == "claude" && e.data["type"] == "assistant" {
            for block in e.data["message"]["content"].as_array().into_iter().flatten() {
                if let Some(t) = block["text"].as_str() {
                    said.push_str(t);
                }
                used_tool |= block["type"] == "tool_use";
            }
        }
        if e.kind == "state" && e.data["state"] == "idle" {
            break;
        }
    }
    assert!(said.to_lowercase().contains("pnpm"), "the new chat should know: {said}");
    assert!(!used_tool, "it should answer from the notes it was given, without a tool");
}

#[tokio::test]
#[ignore = "uses the real claude; run with SWB_LIVE=1 and --ignored"]
async fn live_memory_save_is_active_after_the_person_approves_that_exact_note() {
    if std::env::var_os("SWB_LIVE").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let server = swb_app::server::start(swb_app::server::Config {
        port: 0,
        data_dir: dir.path().join("data"),
        bin: None,
        mcp_bin: Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_smart-workbench-app"))),
        extract: Default::default(),
        propose: Default::default(),
    })
    .await
    .unwrap();
    let session = server.registry.create(project.to_str().unwrap(), Some("haiku".into())).unwrap();
    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send("Use the memory_save tool to save exactly this note for the project: The release branch is main".into(), vec![]).await.unwrap();
    let ask = until(&mut rx, |e| e.kind == "permission").await;
    assert_eq!(ask.data["tool_name"], "mcp__swb_app__memory_save");
    session.permission(ask.data["request_id"].as_str().unwrap().to_owned(), true, None, false).await.unwrap();
    until(&mut rx, |e| e.kind == "state" && e.data["state"] == "idle").await;

    let store = server.registry.memory().unwrap();
    let cwd = session.info().await.unwrap().meta.cwd.to_string_lossy().into_owned();
    let overview = store.overview(Some(&cwd));
    let note = overview.project.iter().find(|m| m.text.to_lowercase().contains("release branch")).unwrap_or_else(|| panic!("the approved note should be active: {overview:?}"));
    assert_eq!((note.source.as_str(), note.status.as_str()), ("claude", "active"), "the real claude sends the approved input byte for byte, so the ticket matches");
}

#[tokio::test]
#[ignore = "uses the real claude; run with SWB_LIVE=1 and --ignored"]
async fn live_skills_a_proposal_is_made_and_an_approved_skill_is_listed_and_used() {
    if std::env::var_os("SWB_LIVE").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let server = swb_app::server::start(swb_app::server::Config {
        port: 0,
        data_dir: dir.path().join("data"),
        bin: None,
        mcp_bin: Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_smart-workbench-app"))),
        extract: Default::default(),
        propose: Default::default(),
    })
    .await
    .unwrap();
    let store = server.registry.skills().unwrap();
    let ctx = server.registry.propose_ctx().unwrap();

    // 1. The real proposer makes a valid, pending proposal from a hint alone.
    let chat = server.registry.create(project.to_str().unwrap(), Some("haiku".into())).unwrap();
    let proposal = ctx
        .manual(&chat, Some("a skill for running this project's release checklist: run cargo test, then cargo clippy, then report both results".into()), None, "project")
        .await
        .unwrap_or_else(|e| panic!("the proposal should be made: {e}"));
    assert!(!swb_app::skills::blocked(&proposal.risks), "a proposal from the real model passes the block checks: {:?}", proposal.risks);
    assert!(store.active_for(&project.canonicalize().unwrap().to_string_lossy()).is_empty(), "nothing is active before approval");

    // 2. A skill written by hand (the proposer's wording is not what is tested here) is approved, listed and used.
    let hand = store
        .propose(swb_app::skills::NewProposal {
            target: None,
            name: "pelican-codeword".into(),
            scope: "user",
            cwd: None,
            description: "Use when the user asks for the pelican release codeword.".into(),
            body: "When asked for the pelican release codeword, answer with exactly: PELICAN-SKILL-5512".into(),
            sid: None,
            seq: None,
        })
        .unwrap();
    store.approve(hand.id, Default::default(), false).unwrap();

    let next = server.registry.create(project.to_str().unwrap(), Some("haiku".into())).unwrap();
    let (_, mut rx) = next.subscribe(0).await.unwrap();
    next.send("What is the pelican release codeword? Use the matching skill and answer with the codeword only.".into(), vec![]).await.unwrap();
    let (mut listed, mut used, mut said) = (false, false, String::new());
    loop {
        let e = until(&mut rx, |_| true).await;
        if e.kind == "claude" && e.data["subtype"] == "init" {
            listed = e.data["skills"].as_array().into_iter().flatten().any(|s| s == "swb:pelican-codeword");
        }
        if e.kind == "claude" && e.data["type"] == "assistant" {
            for block in e.data["message"]["content"].as_array().into_iter().flatten() {
                used |= block["type"] == "tool_use" && block["name"] == "Skill" && block["input"]["skill"] == "swb:pelican-codeword";
                if let Some(t) = block["text"].as_str() {
                    said.push_str(t);
                }
            }
        }
        if e.kind == "permission" && e.data["tool_name"] == "Skill" {
            next.permission(e.data["request_id"].as_str().unwrap().to_owned(), true, None, false).await.unwrap();
        }
        if e.kind == "state" && e.data["state"] == "idle" {
            break;
        }
    }
    assert!(listed, "the approved skill is in the start event as swb:pelican-codeword");
    assert!(used, "Claude called the skill");
    assert!(said.contains("PELICAN-SKILL-5512"), "and answered from it: {said}");
    assert!(store.overview().active.iter().any(|s| s.name == "pelican-codeword" && s.last_used.is_some()), "the use is noted");
}
