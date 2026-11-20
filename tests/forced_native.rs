//! In the desktop app the built-in workbench is forced: the plug-in is off for every chat whatever the settings file says (and the file
//! is left as it was).
use std::time::Duration;

use tenrec::server::{start, Config};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

#[tokio::test]
async fn forcing_the_builtin_workbench_switches_the_plugin_off_without_touching_the_settings_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("scenario.jsonl"), "{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n").unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    tenrec::settings::force_native();
    // No program of the app is known, so the guard hook cannot exist, yet the plug-in is off all the same.
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    assert!(server.registry.settings().native());
    assert!(!server.registry.settings().get().workbench_native, "the file's switch is not changed");
    let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("hi".into(), vec![]).await.unwrap();
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap().unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            break;
        }
    }
    let log = std::fs::read_to_string(root.join("fake.log")).unwrap();
    assert!(log.lines().any(|l| l.starts_with("ARGV ") && l.contains("\"smartworkbench@smart-workbench\":false")), "{log}");
}
