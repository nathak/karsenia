//! The HTTP and WebSocket server: static UI files, one WebSocket for everything a conversation needs, a path-checked image endpoint.
//! It binds 127.0.0.1 only and every request must carry the run's token (see `auth`).
pub mod auth;
pub mod files;
pub mod mcp_host;
pub mod memory_ops;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::claude::protocol::Image;
use crate::index::SearchQuery;
use crate::random_hex;
use crate::registry::Registry;

/// The biggest WebSocket message: five 5 MB images are about 35 MB in base64.
pub const MAX_WS_MESSAGE: usize = 40 * 1024 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
    pub token: Arc<String>,
    pub port: u16,
}

pub struct Config {
    /// 0 picks a free port.
    pub port: u16,
    pub data_dir: PathBuf,
    /// The `claude` program (default: SWB_CLAUDE, else `claude`).
    pub bin: Option<String>,
    /// The program `claude` runs as the MCP server (`<exe> mcp`); None turns history search for Claude off.
    pub mcp_bin: Option<PathBuf>,
    /// When auto-save reads (tests make the waits short).
    pub extract: crate::extract::ExtractConfig,
}

pub struct Running {
    pub registry: Arc<Registry>,
    pub addr: SocketAddr,
    pub token: String,
    pub task: JoinHandle<()>,
}

impl Running {
    /// The address to open: the token rides in the fragment, which is never sent over HTTP.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/#t={}", self.addr.port(), self.token)
    }
}

pub async fn start(config: Config) -> anyhow::Result<Running> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], config.port))).await?;
    let addr = listener.local_addr()?;
    let token = random_hex(32);
    let registry = Arc::new(Registry::open_full(config.data_dir, config.bin, config.mcp_bin, config.extract)?);
    if let Some(gate) = registry.gate() {
        match mcp_host::listen(&gate) {
            Ok(listener) => {
                tokio::spawn(mcp_host::serve(listener, gate, registry.clone()));
            }
            Err(error) => tracing::warn!("the MCP socket is off: {error}"),
        }
    }
    let state = AppState { registry: registry.clone(), token: Arc::new(token.clone()), port: addr.port() };
    let app = Router::new()
        .route("/ws", get(ws_upgrade))
        .route("/api/file", get(file))
        .fallback(get(static_file))
        .layer(middleware::from_fn_with_state(state.clone(), auth::guard))
        .with_state(state);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    Ok(Running { registry, addr, token, task })
}

// ---- static files ----

macro_rules! asset {
    ($path:literal, $type:literal) => {
        ($path, $type, include_bytes!(concat!("../../ui/", $path)) as &[u8])
    };
}

const ASSETS: [(&str, &str, &[u8]); 6] = [
    asset!("index.html", "text/html; charset=utf-8"),
    asset!("app.js", "text/javascript; charset=utf-8"),
    asset!("theme.css", "text/css; charset=utf-8"),
    asset!("util.js", "text/javascript; charset=utf-8"),
    asset!("vendor/marked.js", "text/javascript; charset=utf-8"),
    asset!("vendor/purify.js", "text/javascript; charset=utf-8"),
];

async fn static_file(uri: axum::http::Uri) -> Response {
    let wanted = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        other => other,
    };
    let Some((name, kind, bytes)) = ASSETS.iter().find(|(name, _, _)| *name == wanted) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // SWB_UI_DIR serves the UI from disk, so editing it needs no rebuild while developing.
    let body: Vec<u8> = match std::env::var_os("SWB_UI_DIR") {
        Some(dir) => std::fs::read(PathBuf::from(dir).join(name)).unwrap_or_else(|_| bytes.to_vec()),
        None => bytes.to_vec(),
    };
    ([(header::CONTENT_TYPE, *kind)], body).into_response()
}

// ---- image files ----

#[derive(Deserialize)]
struct FileQuery {
    sid: String,
    path: String,
}

async fn file(State(state): State<AppState>, headers: HeaderMap, Query(query): Query<FileQuery>) -> Response {
    if !auth::bearer_ok(&headers, &state.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(handle) = state.registry.get(&query.sid) else { return StatusCode::NOT_FOUND.into_response() };
    let Some(info) = handle.info().await else { return StatusCode::NOT_FOUND.into_response() };
    let Some(real) = files::resolve(&info.meta.cwd, &query.path) else { return StatusCode::NOT_FOUND.into_response() };
    let Some(kind) = files::image_type(&real) else { return StatusCode::NOT_FOUND.into_response() };
    // Read through one open handle and never past the limit, whatever the file grew to since it was checked.
    let Ok(opened) = tokio::fs::File::open(&real).await else { return StatusCode::NOT_FOUND.into_response() };
    let mut bytes = Vec::new();
    if opened.take(files::MAX_FILE + 1).read_to_end(&mut bytes).await.is_err() || bytes.len() as u64 > files::MAX_FILE {
        return StatusCode::NOT_FOUND.into_response();
    }
    ([(header::CONTENT_TYPE, kind)], bytes).into_response()
}

// ---- the WebSocket ----

async fn ws_upgrade(State(state): State<AppState>, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Response {
    if !auth::origin_ok(&headers, state.port) {
        return StatusCode::FORBIDDEN.into_response();
    }
    upgrade.max_message_size(MAX_WS_MESSAGE).max_frame_size(MAX_WS_MESSAGE).on_upgrade(move |socket| connection(socket, state))
}

pub(crate) fn plain(kind: &str, sid: Option<&str>, data: Value) -> String {
    json!({"v": 1, "kind": kind, "sid": sid, "data": data}).to_string()
}

async fn connection(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();

    // The first message must be the token, soon.
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next()).await;
    let given = match first {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<Value>(&text).ok(),
        _ => None,
    };
    let authed = given.as_ref().map(|v| v["op"] == "auth" && v["token"].as_str().map(|t| auth::ct_eq(t, &state.token)).unwrap_or(false)).unwrap_or(false);
    if !authed {
        let _ = sink.send(Message::Close(None)).await;
        return;
    }

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if sink.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    });

    // What every page should hear (a note changed, an auto-save happened) goes to this one too.
    let mut heard = state.registry.events.subscribe();
    let forward = {
        let out = out_tx.clone();
        tokio::spawn(async move {
            loop {
                match heard.recv().await {
                    Ok(value) => {
                        if out.send(value.to_string()).is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        })
    };
    let mut attached: std::collections::HashMap<String, JoinHandle<()>> = std::collections::HashMap::new();
    let sessions = serde_json::to_value(state.registry.list().await).unwrap_or(Value::Null);
    let _ = out_tx.send(plain("hello", None, json!({"sessions": sessions})));

    while let Some(Ok(message)) = stream.next().await {
        let Message::Text(text) = message else {
            if matches!(message, Message::Close(_)) {
                break;
            }
            continue;
        };
        let Ok(op) = serde_json::from_str::<Value>(&text) else {
            let _ = out_tx.send(plain("error", None, json!({"message": "not JSON"})));
            continue;
        };
        handle_op(&state, &op, &out_tx, &mut attached).await;
    }

    for (_, task) in attached {
        task.abort();
    }
    forward.abort();
    writer.abort();
    // A closed page does not stop the sessions or answer what waits: permissions stay pending until a person answers.
}

async fn handle_op(state: &AppState, op: &Value, out: &mpsc::UnboundedSender<String>, attached: &mut std::collections::HashMap<String, JoinHandle<()>>) {
    let name = op["op"].as_str().unwrap_or_default();
    let sid = op["sid"].as_str().unwrap_or_default().to_owned();
    let fail = |message: &str| {
        let _ = out.send(plain("error", Some(&sid), json!({"message": message})));
    };
    match name {
        "list" => {
            let sessions = serde_json::to_value(state.registry.list().await).unwrap_or(Value::Null);
            let _ = out.send(plain("hello", None, json!({"sessions": sessions})));
        }
        "new" => {
            let cwd = op["cwd"].as_str().unwrap_or_default();
            let model = op["model"].as_str().map(str::to_owned);
            match state.registry.create(cwd, model) {
                Ok(handle) => {
                    let info = handle.info().await;
                    let _ = out.send(plain("created", Some(&handle.sid), serde_json::to_value(info).unwrap_or(Value::Null)));
                    attach(&handle, 0, out, attached).await;
                }
                Err(message) => fail(&message),
            }
        }
        "attach" => match state.registry.get(&sid) {
            Some(handle) => attach(&handle, op["after_seq"].as_u64().unwrap_or(0), out, attached).await,
            None => fail("no such session"),
        },
        "send" => match state.registry.get(&sid) {
            Some(handle) => {
                let images: Vec<Image> = serde_json::from_value(op["images"].clone()).unwrap_or_default();
                if let Err(message) = handle.send(op["text"].as_str().unwrap_or_default().to_owned(), images).await {
                    fail(&message);
                }
            }
            None => fail("no such session"),
        },
        "permission" => match state.registry.get(&sid) {
            Some(handle) => {
                let request_id = op["request_id"].as_str().unwrap_or_default().to_owned();
                let allow = op["allow"].as_bool() == Some(true);
                let message = op["message"].as_str().map(str::to_owned);
                let remember = op["remember"].as_bool() == Some(true);
                if let Err(message) = handle.permission(request_id, allow, message, remember).await {
                    fail(&message);
                }
            }
            None => fail("no such session"),
        },
        "search" => {
            let req = op["req"].clone();
            let query = SearchQuery {
                q: op["q"].as_str().unwrap_or_default().to_owned(),
                cwd: op["cwd"].as_str().map(str::to_owned),
                sid: op["sid"].as_str().map(str::to_owned),
                since: op["since"].as_u64(),
                limit: op["limit"].as_u64().unwrap_or(30) as usize,
            };
            let (out, registry) = (out.clone(), state.registry.clone());
            // A search can take a moment: it runs beside the socket's other messages.
            tokio::spawn(async move {
                let found = match registry.index() {
                    Some(index) => tokio::task::spawn_blocking(move || index.search(&query)).await.ok().and_then(Result::ok),
                    None => None,
                };
                let data = match found {
                    Some(hits) => {
                        let titles: std::collections::HashMap<String, String> = registry.list().await.into_iter().map(|i| (i.meta.sid, i.meta.title)).collect();
                        let hits: Vec<Value> = hits
                            .into_iter()
                            .map(|hit| {
                                let title = titles.get(&hit.sid).cloned().unwrap_or_default();
                                let mut value = serde_json::to_value(&hit).unwrap_or(Value::Null);
                                value["title"] = Value::String(title);
                                value
                            })
                            .collect();
                        json!({"req": req, "hits": hits})
                    }
                    None => json!({"req": req, "hits": [], "error": "search is not available"}),
                };
                let _ = out.send(plain("search_results", None, data));
            });
        }
        "interrupt" => match state.registry.get(&sid) {
            Some(handle) => {
                if let Err(message) = handle.interrupt().await {
                    fail(&message);
                }
            }
            None => fail("no such session"),
        },
        "stop" => match state.registry.get(&sid) {
            Some(handle) => handle.stop(),
            None => fail("no such session"),
        },
        other if other.starts_with("memory_") => memory_ops::handle(state, other, op, out),
        _ => fail("unknown op"),
    }
}

/// Sends the events missed since `after`, then follows the session.
async fn attach(handle: &crate::session::SessionHandle, after: u64, out: &mpsc::UnboundedSender<String>, attached: &mut std::collections::HashMap<String, JoinHandle<()>>) {
    if let Some(old) = attached.remove(&handle.sid) {
        old.abort();
    }
    let Some((missed, mut rx)) = handle.subscribe(after).await else { return };
    let mut last = after;
    for envelope in missed {
        last = last.max(envelope.seq);
        let _ = out.send(serde_json::to_string(&envelope).unwrap_or_default());
    }
    let _ = out.send(plain("attached", Some(&handle.sid), json!({})));
    let out = out.clone();
    let follow = handle.clone();
    let task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(envelope) => {
                    last = last.max(envelope.seq);
                    if out.send(serde_json::to_string(&envelope).unwrap_or_default()).is_err() {
                        break;
                    }
                }
                // Events were dropped for a slow reader: take them again from the log, so a permission is never missed.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let Some((missed, fresh)) = follow.subscribe(last).await else { break };
                    rx = fresh;
                    for envelope in missed {
                        last = last.max(envelope.seq);
                        let _ = out.send(serde_json::to_string(&envelope).unwrap_or_default());
                    }
                }
                Err(_) => break,
            }
        }
    });
    attached.insert(handle.sid.clone(), task);
}
