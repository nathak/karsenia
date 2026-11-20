//! Any file can be attached: a PDF reaches the model as a document, a small text file is put in the message, and anything else is saved
//! in the chat's folder of this app and named in the message; the transcript's event holds names and sizes, never the content; deleting
//! the chat removes the saved files.
use std::time::Duration;

use base64::Engine;
use tenrec::claude::protocol::Image;
use tenrec::server::{start, Config};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

fn file(bytes: &[u8], name: &str) -> Image {
    Image { media_type: "application/octet-stream".into(), data: base64::engine::general_purpose::STANDARD.encode(bytes), name: Some(name.into()) }
}

#[tokio::test]
async fn pdf_text_and_other_files_reach_the_model_and_the_transcript_holds_no_content() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("scenario.jsonl"), "{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n").unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: Some(wrapper.clone()), extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    let files = vec![file(b"%PDF-1.4 body", "plan.pdf"), file(b"line one\nline two\n", "notes.txt"), file(b"\x00\x01\x02 zip", "../../evil.zip")];
    chat.send("see these".into(), files).await.unwrap();
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap().unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            break;
        }
    }
    let log = std::fs::read_to_string(root.join("fake.log")).unwrap();
    let line = log.lines().find(|l| l.starts_with("IN ") && l.contains("see these")).expect("the message was sent");
    assert!(line.contains("\"type\":\"document\"") && line.contains("application/pdf"), "{line}");
    assert!(line.contains("<attached_file name=\\\"notes.txt\\\">") && line.contains("line one"), "{line}");
    let saved = root.join("data").join("attachments").join(&chat.sid).join("evil.zip");
    assert!(saved.exists(), "saved under its safe name: {saved:?}");
    assert!(line.contains(&saved.display().to_string()) && line.contains("size=\\\"7\\\""), "{line}");

    // The transcript: names, sizes and kinds; no content.
    let (replay, _) = chat.subscribe(0).await.unwrap();
    let user = replay.iter().find(|e| e.kind == "user").expect("a user event");
    let shown = user.data["files"].as_array().unwrap();
    assert_eq!(shown.iter().map(|f| f["kind"].as_str().unwrap()).collect::<Vec<_>>(), ["pdf", "text", "file"]);
    assert!(!user.data.to_string().contains("line one") && !user.data.to_string().contains("JVBER"), "{}", user.data);
    let disk = std::fs::read_dir(root.join("data")).unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "jsonl")).map(|e| std::fs::read_to_string(e.path()).unwrap()).collect::<String>();
    assert!(!disk.contains("line one"), "the log has no file content");

    // Deleting the chat removes what was saved for it.
    chat.stop();
    tokio::time::sleep(Duration::from_millis(300)).await;
    server.registry.delete(&chat.sid).await.unwrap();
    assert!(!root.join("data").join("attachments").join(&chat.sid).exists());
}
