//! Container chats, against a stand-in for docker that records every call and runs the fake `claude` for `run`. The real runtimes
//! were not available where this was written, so this checks what the host ASKS for and how it cleans up, not what docker does with it.
#![allow(clippy::result_large_err)]
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use swb_app::providers::Form;
use swb_app::server::{start, Config, Running};
use swb_app::session::SessionHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");
const KEY: &str = "sk-test-token-9999";
const IMAGE: &str = "my-claude:latest";

struct Fx {
    _dir: tempfile::TempDir,
    root: PathBuf,
    runtime: PathBuf,
    calls: PathBuf,
    env_log: PathBuf,
    server: Running,
}

/// A `docker` that logs `CALL [arg] [arg] ...`, answers `ps` from a file, and for `run` starts the fake claude after the image name.
fn write_runtime(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let (calls, env_log) = (root.join("calls.log"), root.join("env.log"));
    std::fs::write(root.join("scenario.jsonl"), "{\"$wait\":\"user\"}\n{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"C1\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n{\"$wait\":\"user\"}\n").unwrap();
    let runtime = root.join("bin").join("docker");
    std::fs::create_dir_all(runtime.parent().unwrap()).unwrap();
    std::fs::write(
        &runtime,
        format!(
            r#"#!/bin/sh
{{ printf 'CALL'; for a in "$@"; do printf ' [%s]' "$a"; done; echo; }} >> '{calls}'
case "$1" in
  run)
    shift
    while [ $# -gt 0 ]; do
      if [ "$1" = '{IMAGE}' ]; then shift; break; fi
      shift
    done
    shift
    FAKE_SCENARIO='{scenario}' FAKE_ENV_LOG='{env_log}' exec '{FAKE}' "$@"
    ;;
  ps) cat '{ps}' 2>/dev/null; exit 0 ;;
  *) exit 0 ;;
esac
"#,
            calls = calls.display(),
            scenario = root.join("scenario.jsonl").display(),
            env_log = env_log.display(),
            ps = root.join("ps.txt").display(),
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
    (runtime, calls, env_log)
}

fn settings_file(root: &Path, runtime: &Path) {
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data").join("settings.json"), json!({"container_runtime": runtime.display().to_string(), "container_image": IMAGE}).to_string()).unwrap();
}

async fn fixture(configured: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (runtime, calls, env_log) = write_runtime(&root);
    if configured {
        settings_file(&root, &runtime);
    }
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, "#!/bin/sh\nexit 97\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    Fx { _dir: dir, root, runtime, calls, env_log, server }
}

fn remote(fx: &Fx) -> String {
    fx.server.registry.providers().save(Form { name: "GW".into(), base_url: "https://gw.example.com/x".into(), token: Some(KEY.into()), ..Default::default() }, 1).unwrap()
}

fn project(fx: &Fx) -> PathBuf {
    let path = fx.root.join("proj");
    std::fs::create_dir_all(&path).unwrap();
    path.canonicalize().unwrap()
}

fn calls(fx: &Fx) -> Vec<String> {
    std::fs::read_to_string(&fx.calls).unwrap_or_default().lines().map(str::to_owned).collect()
}

async fn turn(session: &SessionHandle) {
    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send("hello".into(), vec![]).await.unwrap();
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap().unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            return;
        }
    }
}

async fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..120 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
async fn a_container_chat_runs_through_the_runtime_with_only_its_folders_and_no_key_in_the_arguments_and_a_normal_chat_does_not() {
    let fx = fixture(true).await;
    let id = remote(&fx);
    let folder = project(&fx);
    let chat = fx.server.registry.create_in(folder.to_str().unwrap(), None, Some(&id), true).unwrap();
    turn(&chat).await;

    let run = calls(&fx).into_iter().find(|c| c.starts_with("CALL [run]")).expect("the runtime was asked to run");
    assert!(!run.contains(KEY), "the key is in the runtime's arguments: {run}");
    for flag in ["[--rm]", "[-i]", "[--init]", "[--pull=never]", "[--cap-drop=ALL]", "[--security-opt=no-new-privileges]", "[--read-only]"] {
        assert!(run.contains(flag), "{flag} missing: {run}");
    }
    assert!(run.contains(&format!("[--name] [swb-{}-", chat.sid)) && run.contains("[swb.app=1]") && run.contains("[swb.data="));
    let config = fx.root.join("data").join("chatcfg").join(&chat.sid);
    assert!(run.contains(&format!("[{0}:{0}:rw]", folder.display())), "the project is mounted at its own path: {run}");
    assert!(run.contains(&format!("[{}:/swb/config:rw]", config.display())));
    assert_eq!(run.matches("[-v]").count(), 2, "two mounts and no more: {run}");
    assert!(!run.contains(&format!("[{}]", fx.root.join("data").display())), "the data folder is not mounted");
    assert!(run.contains("[-e] [ANTHROPIC_AUTH_TOKEN]") && run.contains("[-e] [CLAUDE_CONFIG_DIR=/swb/config]"));
    assert!(run.contains(&format!("[{IMAGE}] [claude] [-p]")) && run.contains("[--setting-sources] [user]"), "claude runs after the image with the provider arguments: {run}");

    // What the process inside would be given: the key by name from the client's environment, and not the host's settings path.
    let inside: Value = serde_json::from_str(std::fs::read_to_string(&fx.env_log).unwrap().lines().next().unwrap()).unwrap();
    assert_eq!(inside["env"]["ANTHROPIC_AUTH_TOKEN"], json!(KEY));
    assert!(inside["env"].get("CLAUDE_CONFIG_DIR").is_none(), "the host path of the settings folder is not in the client's environment");

    // A normal chat is untouched by the container settings: no runtime call for it.
    let before = calls(&fx).len();
    let normal = fx.server.registry.create(folder.to_str().unwrap(), None, None).unwrap();
    let err = normal.send("hello".into(), vec![]).await;
    assert!(calls(&fx).len() == before, "a normal chat went through the runtime: {err:?}");
}

#[tokio::test]
async fn the_container_is_removed_by_name_when_the_chat_is_stopped_or_killed_and_leftovers_are_swept_at_start() {
    let fx = fixture(true).await;
    let id = remote(&fx);
    let chat = fx.server.registry.create_in(project(&fx).to_str().unwrap(), None, Some(&id), true).unwrap();
    turn(&chat).await;
    let name = calls(&fx).iter().find_map(|c| c.split("[--name] [").nth(1).map(|r| r.split(']').next().unwrap().to_owned())).unwrap();
    assert!(name.starts_with(&format!("swb-{}-", chat.sid)));

    chat.kill();
    wait_for("the kill to remove the container", || calls(&fx).iter().any(|c| *c == format!("CALL [rm] [-f] [{name}]"))).await;

    // Stop removes it after the client had its chance to pass the signal on.
    let second = fx.server.registry.create_in(project(&fx).to_str().unwrap(), None, Some(&id), true).unwrap();
    turn(&second).await;
    let second_name = calls(&fx).iter().rev().find_map(|c| c.split("[--name] [").nth(1).map(|r| r.split(']').next().unwrap().to_owned())).unwrap();
    assert_ne!(second_name, name, "every start has a name of its own");
    second.stop();
    wait_for("stop to remove the container", || calls(&fx).iter().any(|c| *c == format!("CALL [rm] [-f] [{second_name}]"))).await;

    // A later start of the app removes what a crash left, found by the label of its own data folder.
    std::fs::write(fx.root.join("ps.txt"), "abc123\nzz9\n$(touch pwned)\n").unwrap();
    let _again = swb_app::registry::Registry::open(fx.root.join("data"), None).unwrap();
    wait_for("the sweep", || calls(&fx).iter().any(|c| *c == "CALL [rm] [-f] [zz9]")).await;
    let all = calls(&fx);
    assert!(all.iter().any(|c| c.starts_with("CALL [ps] [-aq] [--filter] [label=swb.data=")), "{all:?}");
    assert!(all.iter().any(|c| *c == "CALL [rm] [-f] [abc123]"));
    assert!(!all.iter().any(|c| c.contains("pwned")), "an id that is not plain letters and digits is never passed on");
    assert!(!fx.root.join("pwned").exists());
}

#[tokio::test]
async fn a_container_chat_needs_a_provider_a_reachable_address_and_the_persons_settings() {
    let fx = fixture(false).await;
    let id = remote(&fx);
    let folder = project(&fx);
    let folder = folder.to_str().unwrap();
    let registry = &fx.server.registry;
    assert!(registry.create_in(folder, None, Some(&id), true).is_err(), "no runtime and image are set");
    registry.settings().update(|s| {
        s.container_runtime = Some(fx.runtime.display().to_string());
        s.container_image = Some(IMAGE.into());
    });
    assert!(registry.create_in(folder, None, None, true).is_err(), "an account chat cannot run in a container: its credentials would have to go in");
    let local = registry.providers().save(Form { name: "Local".into(), base_url: "http://127.0.0.1:11434".into(), ..Default::default() }, 1).unwrap();
    assert!(registry.create_in(folder, None, Some(&local), true).is_err(), "a container's localhost is not this computer's");
    let chat = registry.create_in(folder, None, Some(&id), true).unwrap();

    // The settings are checked again at every start: a changed or emptied setting stops the chat, it does not fall back to no container.
    registry.settings().update(|s| s.container_image = Some("--privileged".into()));
    let err = chat.send("hello".into(), vec![]).await.unwrap_err();
    assert!(err.contains("container settings"), "{err}");
    assert!(calls(&fx).iter().all(|c| !c.starts_with("CALL [run]")), "nothing was started");
    registry.settings().update(|s| s.container_runtime = None);
    assert!(chat.send("hello".into(), vec![]).await.is_err());
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn socket(fx: &Fx) -> Ws {
    let port = fx.server.addr.port();
    let mut request = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    request.headers_mut().insert("Origin", HeaderValue::from_str(&format!("http://127.0.0.1:{port}")).unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    ws.send(Message::Text(json!({"op": "auth", "token": fx.server.token}).to_string())).await.unwrap();
    ws
}

async fn next_kind(ws: &mut Ws, kind: &str) -> Value {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("timed out").expect("closed").expect("error");
        if let Message::Text(text) = got {
            let v: Value = serde_json::from_str(&text).unwrap();
            if v["kind"] == kind {
                return v["data"].clone();
            }
        }
    }
}

#[tokio::test]
async fn the_person_sets_the_runtime_and_image_in_the_page_and_odd_ones_are_refused() {
    let fx = fixture(false).await;
    let mut ws = socket(&fx).await;
    let try_set = |runtime: String, image: &str| json!({"op": "provider_container", "rid": 1, "runtime": runtime, "image": image});
    for (what, op) in [
        ("a program that is not docker or podman", try_set("/bin/ls".into(), IMAGE)),
        ("a name looked up in PATH", try_set("docker".into(), IMAGE)),
        ("an image that is a flag", try_set(fx.runtime.display().to_string(), "--privileged")),
        ("an image with a space", try_set(fx.runtime.display().to_string(), "a b")),
    ] {
        ws.send(Message::Text(op.to_string())).await.unwrap();
        assert_eq!(next_kind(&mut ws, "provider_result").await["ok"], json!(false), "{what} must be refused");
    }
    ws.send(Message::Text(json!({"op": "provider_list"}).to_string())).await.unwrap();
    assert_eq!(next_kind(&mut ws, "providers").await["container"]["ready"], json!(false));

    ws.send(Message::Text(try_set(fx.runtime.display().to_string(), IMAGE).to_string())).await.unwrap();
    assert_eq!(next_kind(&mut ws, "provider_result").await["ok"], json!(true));
    ws.send(Message::Text(json!({"op": "provider_list"}).to_string())).await.unwrap();
    let listing = next_kind(&mut ws, "providers").await;
    assert_eq!((listing["container"]["ready"].clone(), listing["container"]["image"].clone()), (json!(true), json!(IMAGE)));

    // A new container chat through the page's own operation.
    let id = remote(&fx);
    ws.send(Message::Text(json!({"op": "new", "cwd": project(&fx).display().to_string(), "provider": id, "container": true}).to_string())).await.unwrap();
    let created = next_kind(&mut ws, "created").await;
    assert_eq!(created["container"], json!(true));
    assert_eq!(created["provider"]["host"], json!("gw.example.com"));
    // And one for the account with the flag set is refused.
    ws.send(Message::Text(json!({"op": "new", "cwd": project(&fx).display().to_string(), "container": true}).to_string())).await.unwrap();
    next_kind(&mut ws, "error").await;

    ws.send(Message::Text(try_set(String::new(), "").to_string())).await.unwrap();
    assert_eq!(next_kind(&mut ws, "provider_result").await["ok"], json!(true), "both empty clears them");
    ws.send(Message::Text(json!({"op": "provider_list"}).to_string())).await.unwrap();
    assert_eq!(next_kind(&mut ws, "providers").await["container"]["ready"], json!(false));
}
