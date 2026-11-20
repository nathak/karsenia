//! A chat reopened after the app was closed does not wait for background jobs: the process that ran them is gone.
use tenrec::registry::Registry;

#[tokio::test]
async fn reopening_a_chat_forgets_the_background_jobs_of_the_process_that_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let sid = {
        let registry = Registry::open(root.join("data"), Some("/nonexistent/claude".into())).unwrap();
        let chat = registry.create(project.to_str().unwrap(), None, None).unwrap();
        // The log as the program left it: idle, with a job still listed in the background.
        let log = tenrec::session::log_path(&root.join("data"), &chat.sid);
        let line = |seq: u64, kind: &str, data: serde_json::Value| serde_json::json!({"v": 1, "sid": chat.sid, "seq": seq, "ts": 1, "kind": kind, "data": data}).to_string();
        let text = [
            line(1, "claude", serde_json::json!({"type": "system", "subtype": "init", "model": "m"})),
            line(2, "claude", serde_json::json!({"type": "system", "subtype": "background_tasks_changed", "tasks": [{"task_id": "a", "task_type": "local_bash", "description": "make"}]})),
            line(3, "state", serde_json::json!({"state": "idle"})),
        ]
        .join("\n");
        std::fs::write(&log, format!("{text}\n")).unwrap();
        chat.sid.clone()
    };
    let reopened = Registry::open(root.join("data"), Some("/nonexistent/claude".into())).unwrap();
    let (replay, _) = reopened.get(&sid).unwrap().subscribe(0).await.unwrap();
    let last_bg = replay.iter().rev().find(|e| e.kind == "claude" && e.data["subtype"] == "background_tasks_changed").expect("a background event");
    assert!(last_bg.data["tasks"].as_array().unwrap().is_empty(), "{:?}", last_bg.data);
}
