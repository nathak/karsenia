//! A normal chat does not start `claude` in a project whose own configuration is risky until the person has decided; a project with
//! nothing risky never asks; trust holds only for the configuration that was shown.
use std::time::Duration;

use tenrec::server::{start, Config};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");
const RISKY: &str = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"./x.sh --token SECRET"}]}]}}"#;

async fn idle(rx: &mut tokio::sync::broadcast::Receiver<tenrec::session::Envelope>) {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.expect("timed out").unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            return;
        }
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    project: std::path::PathBuf,
    server: tenrec::server::Running,
}

async fn rig(settings: Option<&str>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let step = "{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n";
    std::fs::write(root.join("scenario.jsonl"), format!("{step}{step}")).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(project.join(".claude")).unwrap();
    if let Some(body) = settings {
        std::fs::write(project.join(".claude/settings.json"), body).unwrap();
    }
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    Rig { _dir: dir, root, project, server }
}

impl Rig {
    fn starts(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("fake.log")).unwrap_or_default().lines().filter(|l| l.starts_with("ARGV ")).map(str::to_owned).collect()
    }
}

/// The hash of the last `project_check` event a subscriber has seen.
fn shown(rx: &mut tokio::sync::broadcast::Receiver<tenrec::session::Envelope>) -> Option<String> {
    let mut hash = None;
    while let Ok(e) = rx.try_recv() {
        if e.kind == "project_check" {
            hash = e.data["hash"].as_str().map(str::to_owned);
        }
    }
    hash
}

#[tokio::test]
async fn a_risky_project_asks_first_and_trust_holds_only_for_what_was_shown() {
    let r = rig(Some(RISKY)).await;
    let chat = r.server.registry.create_in(r.project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    let err = chat.send("hi".into(), vec![]).await.unwrap_err();
    assert!(err.contains("decision") && err.contains("hooks") && err.contains("Stop") && err.contains("./x.sh") && !err.contains("SECRET") && !err.contains("--token"), "{err}");
    assert!(r.starts().is_empty(), "nothing was started");
    let hash = shown(&mut rx).expect("the findings were sent");

    // A decision on a hash that is not the current one is refused.
    assert!(chat.project_decide("0000".into(), false, false).await.is_err());
    chat.project_decide(hash.clone(), false, false).await.unwrap();
    let info = r.server.registry.list().await.into_iter().find(|i| i.meta.sid == chat.sid).unwrap();
    assert_eq!(info.meta.project_cfg, Some(tenrec::session::ProjectChoice::Trusted { hash: hash.clone() }));

    // The hook changes before the chat starts: the trust was for what was shown, so it asks again, with a new hash.
    std::fs::write(r.project.join(".claude/settings.json"), RISKY.replace("x.sh", "y.sh")).unwrap();
    assert!(chat.send("hi".into(), vec![]).await.is_err());
    assert!(r.starts().is_empty());
    let again = shown(&mut rx).expect("shown again");
    assert_ne!(again, hash);
    // Deciding on the old hash is refused now; on the new one the chat starts, with the project's settings read as always.
    assert!(chat.project_decide(hash, false, false).await.is_err());
    chat.project_decide(again, false, false).await.unwrap();
    chat.send("hi".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    assert_eq!(r.starts().len(), 1);
    assert!(!r.starts()[0].contains("--setting-sources"), "{:?}", r.starts());
}

#[tokio::test]
async fn starting_without_the_project_settings_passes_user_only_sources() {
    let r = rig(Some(RISKY)).await;
    let chat = r.server.registry.create_in(r.project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    assert!(chat.send("hi".into(), vec![]).await.is_err());
    chat.project_decide(String::new(), true, false).await.unwrap();
    chat.send("hi".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    let starts = r.starts();
    assert_eq!(starts.len(), 1);
    assert!(starts[0].contains("--setting-sources user"), "{}", starts[0]);
    // The decision stays with the chat: it is not asked again, even if the file changes.
    std::fs::write(r.project.join(".claude/settings.json"), RISKY.replace("x.sh", "z.sh")).unwrap();
    chat.send("again".into(), vec![]).await.unwrap();
}

#[tokio::test]
async fn nothing_risky_never_asks_and_an_old_chat_meta_without_the_field_is_read() {
    let r = rig(Some(r#"{"permissions":{"allow":["Bash(npm test)"]},"env":{"DISABLE_TELEMETRY":"1"}}"#)).await;
    let chat = r.server.registry.create_in(r.project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("hi".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    assert!(shown(&mut rx).is_none());
    assert!(!r.starts()[0].contains("--setting-sources"));
    let old: tenrec::session::Meta = {
        let info = r.server.registry.list().await.into_iter().find(|i| i.meta.sid == chat.sid).unwrap();
        let mut v = serde_json::to_value(&info.meta).unwrap();
        v.as_object_mut().unwrap().remove("project_cfg");
        serde_json::from_value(v).unwrap()
    };
    assert_eq!(old.project_cfg, None);
}

#[cfg(unix)]
#[tokio::test]
async fn a_linked_settings_file_is_never_trusted_only_left_out() {
    let r = rig(None).await;
    let real = r.root.join("real.json");
    std::fs::write(&real, r#"{"hooks":{"Stop":[{"hooks":[{"command":"./a.sh"}]}]}}"#).unwrap();
    std::os::unix::fs::symlink(&real, r.project.join(".claude/settings.json")).unwrap();
    let chat = r.server.registry.create_in(r.project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    assert!(chat.send("hi".into(), vec![]).await.is_err());
    let hash = shown(&mut rx).expect("shown");
    let err = chat.project_decide(hash, false, false).await.unwrap_err();
    assert!(err.contains("link"), "{err}");
    assert!(chat.send("hi".into(), vec![]).await.is_err(), "still asks");
    chat.project_decide(String::new(), true, false).await.unwrap();
    chat.send("hi".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    assert!(r.starts()[0].contains("--setting-sources user"));
}

#[tokio::test]
async fn leaving_the_settings_out_is_refused_while_the_process_runs_and_a_named_chat_waits_for_the_decision() {
    let r = rig(Some(r#"{"permissions":{"allow":["Edit"]}}"#)).await;
    // A named chat is warmed at once: the gate holds that back too, and the first send says why.
    let named = r.server.registry.create_in(r.project.to_str().unwrap(), None, None, false, None).unwrap();
    named.rename("n1".into()).await.unwrap();
    assert!(named.warm().await.is_err());
    assert!(r.starts().is_empty());
    let (_, mut rx) = named.subscribe(0).await.unwrap();
    named.project_decide(String::new(), true, false).await.unwrap();
    named.send("hi".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    assert_eq!(r.starts().len(), 1);
    // The process runs now with whatever it read: a later "without" cannot change that.
    let other = rig(Some("{}")).await;
    let chat = other.server.registry.create_in(other.project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("hi".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    assert!(chat.project_decide(String::new(), true, false).await.is_err());
}

// ---- trust remembered for the project (only when the person chose "remember for this project") ----

/// A second chat in the same folder; the question is asked again, or not.
fn another(r: &Rig) -> tenrec::session::SessionHandle {
    r.server.registry.create_in(r.project.to_str().unwrap(), None, None, false, None).unwrap()
}

#[tokio::test]
async fn a_remembered_trust_lets_a_new_chat_in_the_same_folder_start_without_asking_and_any_change_asks_again() {
    let r = rig(Some(RISKY)).await;
    let first = another(&r);
    let (_, mut rx) = first.subscribe(0).await.unwrap();
    assert!(first.send("hi".into(), vec![]).await.is_err());
    let hash = shown(&mut rx).unwrap();
    first.project_decide(hash.clone(), false, true).await.unwrap();

    let second = another(&r);
    let (_, mut rx2) = second.subscribe(0).await.unwrap();
    second.send("hi".into(), vec![]).await.unwrap();
    idle(&mut rx2).await;
    assert_eq!(r.starts().len(), 1, "the second chat started without a question");
    let info = r.server.registry.list().await.into_iter().find(|i| i.meta.sid == second.sid).unwrap();
    assert_eq!(info.meta.project_cfg, Some(tenrec::session::ProjectChoice::Trusted { hash: hash.clone() }), "and it holds what it was told, like any chat");

    // A harmless change does not change the hash.
    std::fs::write(r.project.join(".claude/settings.json"), RISKY.replacen("{\"hooks\"", "{\"cleanupPeriodDays\":5,\"hooks\"", 1)).unwrap();
    let third = another(&r);
    let (_, mut rx3) = third.subscribe(0).await.unwrap();
    third.send("hi".into(), vec![]).await.unwrap();
    idle(&mut rx3).await;
    // A change of the risky part asks again, with a new hash.
    std::fs::write(r.project.join(".claude/settings.json"), RISKY.replace("x.sh", "y.sh")).unwrap();
    let fourth = another(&r);
    let (_, mut rx4) = fourth.subscribe(0).await.unwrap();
    assert!(fourth.send("hi".into(), vec![]).await.is_err());
    assert_ne!(shown(&mut rx4).unwrap(), hash);
}

#[tokio::test]
async fn trust_that_was_not_asked_to_be_remembered_stays_with_its_chat() {
    let r = rig(Some(RISKY)).await;
    let first = another(&r);
    let (_, mut rx) = first.subscribe(0).await.unwrap();
    assert!(first.send("hi".into(), vec![]).await.is_err());
    let hash = shown(&mut rx).unwrap();
    first.project_decide(hash, false, false).await.unwrap();
    assert!(another(&r).send("hi".into(), vec![]).await.is_err(), "a plain trust is for that chat only");

    // Leaving the settings out is never remembered either.
    let left_out = another(&r);
    assert!(left_out.send("hi".into(), vec![]).await.is_err());
    left_out.project_decide(String::new(), true, true).await.unwrap();
    assert!(another(&r).send("hi".into(), vec![]).await.is_err());
    assert!(!r.root.join("data/project_trust.json").exists() || !std::fs::read_to_string(r.root.join("data/project_trust.json")).unwrap().contains("hash"), "nothing was recorded");
}

#[tokio::test]
async fn a_hand_written_trust_only_skips_the_question_when_the_files_as_they_are_match_it() {
    let r = rig(Some(RISKY)).await;
    let key = format!("\n{}", r.project.display());
    let write = |hash: &str, strong: Option<&str>| {
        let mut entry = serde_json::json!({"hash": hash});
        if let Some(strong) = strong {
            entry["strong"] = strong.into();
        }
        std::fs::create_dir_all(r.root.join("data")).unwrap();
        std::fs::write(r.root.join("data/project_trust.json"), serde_json::json!({"v": 1, "projects": {key.clone(): entry}}).to_string()).unwrap();
    };
    let scan = tenrec::project_check::scan(&r.project);
    write("0000", Some("ffff"));
    assert!(another(&r).send("hi".into(), vec![]).await.is_err(), "wrong hashes");
    write(&scan.hash, None);
    assert!(another(&r).send("hi".into(), vec![]).await.is_err(), "a weak hash alone");
    write(&scan.hash, Some("ffff"));
    assert!(another(&r).send("hi".into(), vec![]).await.is_err(), "the strong one must agree");
    write(&scan.hash, Some(&scan.strong));
    let chat = another(&r);
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("hi".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
}

#[tokio::test]
async fn forgetting_the_trust_asks_again_for_new_chats_only() {
    let r = rig(Some(RISKY)).await;
    let first = another(&r);
    let (_, mut rx) = first.subscribe(0).await.unwrap();
    assert!(first.send("hi".into(), vec![]).await.is_err());
    let hash = shown(&mut rx).unwrap();
    first.project_decide(hash, false, true).await.unwrap();
    let second = another(&r);
    let (_, mut rx2) = second.subscribe(0).await.unwrap();
    second.send("hi".into(), vec![]).await.unwrap();
    idle(&mut rx2).await;

    assert!(r.server.registry.project_defaults(r.project.to_str().unwrap(), None)["trusted"].as_bool().unwrap_or(false), "the form can say it is remembered");
    assert!(r.server.registry.forget_trust(r.project.to_str().unwrap(), None));
    assert!(!r.server.registry.forget_trust(r.project.to_str().unwrap(), None), "nothing left");
    assert!(another(&r).send("hi".into(), vec![]).await.is_err(), "a new chat asks again");
    second.send("again".into(), vec![]).await.unwrap();
}
