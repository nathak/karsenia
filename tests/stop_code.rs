//! A process this host ends is reported as exited with no code, even when the program answers the stop with an exit code of its own
//! (on Windows an ended process gets the code 1, which is also what a failure looks like): ending a chat is not told as an error.
use std::time::Duration;

use tenrec::server::{start, Config};

#[tokio::test]
async fn a_process_the_host_stops_is_exited_with_no_code_even_if_it_exits_with_one() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let wrapper = root.join("claude");
    // A program that answers the stop signal with `exit 1`.
    std::fs::write(&wrapper, "#!/bin/sh\ntrap 'exit 1' TERM\nsleep 30 &\nwait $!\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("go".into(), vec![]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    chat.stop();
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.expect("an event in time").unwrap();
        if got.kind == "state" && got.data["state"] == "exited" {
            assert!(got.data["code"].is_null(), "the host ended it: no code, got {:?}", got.data);
            break;
        }
    }
}
