//! The HTTP and WebSocket server: static UI files, one WebSocket for everything a conversation needs, a path-checked image endpoint.
//! It binds 127.0.0.1 only and every request must carry the run's token (see `auth`).
pub mod account_ops;
pub mod cost_ops;
pub mod auth;
pub mod files;
pub mod mcp_host;
pub mod fs_ops;
pub mod memory_ops;
pub mod project_fs;
pub mod provider_ops;
pub mod sched_ops;
pub mod skill_ops;
#[cfg(unix)]
pub mod term_ops;
#[cfg(not(unix))]
#[path = "term_ops_stub.rs"]
pub mod term_ops;
pub mod wt_ops;

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
    /// The terminal tab is on (a start option, never a setting a page can change).
    pub terminal: bool,
    /// How many terminals are open in the whole app.
    pub term_slots: Arc<std::sync::atomic::AtomicUsize>,
}

pub struct Config {
    /// 0 picks a free port.
    pub port: u16,
    pub data_dir: PathBuf,
    /// The `claude` program (default: TENREC_CLAUDE, else `claude`).
    pub bin: Option<String>,
    /// The program `claude` runs as the MCP server (`<exe> mcp`); None turns history search for Claude off.
    pub mcp_bin: Option<PathBuf>,
    /// When auto-save reads (tests make the waits short).
    pub extract: crate::extract::ExtractConfig,
    /// When automatic skill proposals happen.
    pub propose: crate::propose::ProposeConfig,
    pub sched: crate::scheduler::SchedConfig,
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

/// Starts the app; the terminal tab is on only when TENREC_TERMINAL=1.
pub async fn start(config: Config) -> anyhow::Result<Running> {
    let terminal = std::env::var("TENREC_TERMINAL").map(|v| v == "1").unwrap_or(false);
    start_with(config, terminal).await
}

/// Opens the data folder and starts what `claude` reaches the app through (the MCP socket, a named pipe on Windows). Used by the
/// web server and by the desktop app, which has no network listener at all. Needs a running tokio runtime.
pub fn start_core(config: Config) -> anyhow::Result<Arc<Registry>> {
    let registry = Arc::new(Registry::open_full(config.data_dir, config.bin, config.mcp_bin, config.extract, config.propose, config.sched)?);
    if let Some(gate) = registry.gate() {
        match mcp_host::listen(&gate) {
            Ok(listener) => {
                tokio::spawn(mcp_host::serve(listener, gate, registry.clone()));
            }
            Err(error) => tracing::warn!("the MCP socket is off: {error}"),
        }
    }
    // A named chat's `claude` is started now, not at its first message: its name is how other sessions find it.
    let warm = registry.clone();
    tokio::spawn(async move { warm.warm_named().await });
    Ok(registry)
}

pub async fn start_with(config: Config, terminal: bool) -> anyhow::Result<Running> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], config.port))).await?;
    let addr = listener.local_addr()?;
    let token = random_hex(32);
    let registry = start_core(config)?;
    let state = AppState { registry: registry.clone(), token: Arc::new(token.clone()), port: addr.port(), terminal, term_slots: Default::default() };
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

const ASSETS: [(&str, &str, &[u8]); 13] = [
    asset!("index.html", "text/html; charset=utf-8"),
    asset!("favicon.png", "image/png"),
    asset!("app.js", "text/javascript; charset=utf-8"),
    asset!("theme.css", "text/css; charset=utf-8"),
    asset!("util.js", "text/javascript; charset=utf-8"),
    asset!("files.js", "text/javascript; charset=utf-8"),
    asset!("term.js", "text/javascript; charset=utf-8"),
    asset!("wt.js", "text/javascript; charset=utf-8"),
    asset!("vendor/marked.js", "text/javascript; charset=utf-8"),
    asset!("vendor/purify.js", "text/javascript; charset=utf-8"),
    asset!("vendor/fonts/pretendard-400.otf", "font/otf"),
    asset!("vendor/fonts/pretendard-600.otf", "font/otf"),
    asset!("vendor/fonts/jetbrains-mono-400.woff2", "font/woff2"),
];

async fn static_file(uri: axum::http::Uri) -> Response {
    let wanted = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        other => other,
    };
    let Some((name, kind, bytes)) = ASSETS.iter().find(|(name, _, _)| *name == wanted) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // TENREC_UI_DIR serves the UI from disk, so editing it needs no rebuild while developing.
    let body: Vec<u8> = match std::env::var_os("TENREC_UI_DIR") {
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
    // The same project check as the editor's (not the app's data folder or a place that holds it, not the home folder or a place that
    // keeps credentials), then the image rules; the disk work runs off the async threads.
    let (project, data_dir, home, wanted) = (info.meta.cwd.clone(), state.registry.dir().to_path_buf(), crate::platform::home_dir(), query.path.clone());
    let found = tokio::task::spawn_blocking(move || {
        let root = fs_ops::check_project(&project, &data_dir, home.as_deref(), false).ok()?;
        let real = files::resolve(&root, &wanted)?;
        let kind = files::image_type(&real)?;
        Some((kind, files::read_image(&real)?))
    })
    .await
    .ok()
    .flatten();
    let Some((kind, bytes)) = found else { return StatusCode::NOT_FOUND.into_response() };
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

/// The realm a message names: `"account": "<id>"` for an extra account, `"account": null` for the default one. A message without the key
/// is refused, so no page acts on an account it did not choose, and an id that is not open is refused rather than answered from the
/// default account.
pub(crate) fn realm_of(state: &AppState, op: &Value) -> Result<std::sync::Arc<crate::realm::Realm>, String> {
    match op.get("account") {
        None => Err("account is required".to_owned()),
        Some(Value::Null) => state.registry.realm(None),
        Some(Value::String(id)) => state.registry.realm(Some(id)),
        Some(_) => Err("account must be an id or null".to_owned()),
    }
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
    // Terminal output rides a small bounded channel of its own: a page that reads slowly makes the shell wait, nothing piles up.
    let (bin_tx, mut bin_rx) = mpsc::channel::<Vec<u8>>(32);
    let writer = tokio::spawn(async move {
        loop {
            let message = tokio::select! {
                text = out_rx.recv() => match text { Some(text) => Message::Text(text), None => break },
                frame = bin_rx.recv() => match frame { Some(frame) => Message::Binary(frame), None => break },
            };
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });

    // Subscribed before hello, so nothing said in between is missed.
    let mut conn = Conn::open(state.clone(), out_tx.clone(), true);
    let mut terms = term_ops::Terms::default();
    let sessions = serde_json::to_value(state.registry.list().await).unwrap_or(Value::Null);
    let _ = out_tx.send(plain("hello", None, json!({"sessions": sessions, "terminal": state.terminal})));

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
        match op["op"].as_str() {
            Some(name) if name.starts_with("term_") => term_ops::handle(&state, name, &op, &out_tx, &bin_tx, &mut terms).await,
            _ => conn.dispatch(&op).await,
        }
    }
    // Every shell this page opened ends with it.
    drop(terms);
    drop(conn);
    writer.abort();
    // A closed page does not stop the sessions or answer what waits: permissions stay pending until a person answers.
}

/// One page's link to the app: where its replies go, the chats it follows and the app-wide events forwarded to it. Shared by
/// the WebSocket and the desktop window's in-process link, so both are answered by the same code.
struct Conn {
    state: AppState,
    out: mpsc::UnboundedSender<String>,
    /// None: this link does not follow chats (the desktop window reads them from the registry itself).
    attached: Option<std::collections::HashMap<String, JoinHandle<()>>>,
    forward: JoinHandle<()>,
}

impl Conn {
    fn open(state: AppState, out: mpsc::UnboundedSender<String>, follows: bool) -> Self {
        // What every page should hear (a note changed, an auto-save happened) goes to this one too.
        let mut heard = state.registry.events.subscribe();
        let forward = {
            let out = out.clone();
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
        Conn { state, out, attached: follows.then(Default::default), forward }
    }

    async fn dispatch(&mut self, op: &Value) {
        match op["op"].as_str() {
            Some(name) if name.starts_with("wt_") => wt_ops::handle(&self.state, name, op, &self.out),
            _ => handle_op(&self.state, op, &self.out, self.attached.as_mut()).await,
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        for (_, task) in self.attached.take().unwrap_or_default() {
            task.abort();
        }
        self.forward.abort();
    }
}

/// The desktop window's link to the app: the same operations and replies as a page's WebSocket, with no socket, token or
/// terminal, and no chat following (the window reads chats from the registry). It ends when every sender is dropped. Needs a
/// running tokio runtime.
pub fn local(registry: Arc<Registry>) -> (mpsc::UnboundedSender<Value>, mpsc::UnboundedReceiver<String>) {
    let (op_tx, out_rx, _bin) = local_with_terminal(registry, false);
    (op_tx, out_rx)
}

/// `local`, and the terminal's output frames (see `term_ops`) when `terminal` is true. The caller decides that from the start
/// options only (TENREC_TERMINAL=1), never from anything a chat or a file says; with it off every `term_*` operation is refused.
/// The shells end when the operation sender is dropped.
pub fn local_with_terminal(registry: Arc<Registry>, terminal: bool) -> (mpsc::UnboundedSender<Value>, mpsc::UnboundedReceiver<String>, mpsc::Receiver<Vec<u8>>) {
    let (op_tx, mut op_rx) = mpsc::unbounded_channel::<Value>();
    let (out_tx, out_rx) = mpsc::unbounded_channel::<String>();
    // The same small bounded channel the socket uses: a window that reads slowly makes the shell wait.
    let (bin_tx, bin_rx) = mpsc::channel::<Vec<u8>>(32);
    // The token never meets HTTP here; a random one rather than an empty one all the same.
    let state = AppState { registry, token: Arc::new(random_hex(32)), port: 0, terminal, term_slots: Default::default() };
    tokio::spawn(async move {
        let mut conn = Conn::open(state.clone(), out_tx.clone(), false);
        let mut terms = term_ops::Terms::default();
        while let Some(op) = op_rx.recv().await {
            match op["op"].as_str() {
                Some(name) if name.starts_with("term_") => term_ops::handle(&state, name, &op, &out_tx, &bin_tx, &mut terms).await,
                _ => conn.dispatch(&op).await,
            }
        }
    });
    (op_tx, out_rx, bin_rx)
}

async fn handle_op(state: &AppState, op: &Value, out: &mpsc::UnboundedSender<String>, attached: Option<&mut std::collections::HashMap<String, JoinHandle<()>>>) {
    let name = op["op"].as_str().unwrap_or_default();
    let sid = op["sid"].as_str().unwrap_or_default().to_owned();
    let fail = |message: &str| {
        let _ = out.send(plain("error", Some(&sid), json!({"message": message})));
    };
    match name {
        "list" => {
            let sessions = serde_json::to_value(state.registry.list().await).unwrap_or(Value::Null);
            let _ = out.send(plain("hello", None, json!({"sessions": sessions, "terminal": state.terminal})));
        }
        "new" => {
            let cwd = op["cwd"].as_str().unwrap_or_default();
            let model = op["model"].as_str().map(str::to_owned);
            let provider = op["provider"].as_str().filter(|id| !id.is_empty());
            let container = op["container"].as_bool() == Some(true);
            let isolated = op["worktree"].as_bool() == Some(true);
            let account = op["account"].as_str().filter(|id| !id.is_empty());
            // The permission mode: absent is the default; present, it is one of the six names exactly (a client cannot make another
            // one up), and skipping every check also needs the request to say it was confirmed, so a bare message cannot choose it.
            let mode = match op.get("permission") {
                None | Some(Value::Null) => Ok(crate::claude::perm::PermMode::Default),
                Some(Value::String(name)) => match crate::claude::perm::PermMode::parse(name) {
                    Some(crate::claude::perm::PermMode::Bypass) if op["confirm_bypass"].as_bool() != Some(true) => Err("skipping every permission check needs confirm_bypass".to_owned()),
                    Some(mode) => Ok(mode),
                    None => Err("unknown permission mode".to_owned()),
                },
                Some(_) => Err("unknown permission mode".to_owned()),
            };
            let chat_name = op["name"].as_str().map(str::to_owned);
            let made = match mode {
                Err(message) => Err(message),
                Ok(_) if isolated && container => Err("a chat in a worktree cannot also run in a container".to_owned()),
                Ok(mode) if isolated => state.registry.create_isolated_mode(cwd, model, provider, account, mode).await,
                Ok(mode) => state.registry.create_in_mode(cwd, model, provider, container, account, mode).map(|handle| (handle, None)),
            };
            match made {
                Ok((handle, notice)) => {
                    // A name given when the chat is made: a first message does not replace it.
                    if let Some(name) = chat_name.filter(|n| !n.trim().is_empty()) {
                        let _ = handle.rename(name).await;
                    }
                    let info = handle.info().await;
                    if let Some(info) = &info {
                        state.registry.remember_defaults(&info.meta);
                    }
                    let mut data = serde_json::to_value(info).unwrap_or(Value::Null);
                    if let (Some(map), Some(notice)) = (data.as_object_mut(), notice) {
                        map.insert("notice".into(), json!(notice));
                    }
                    let _ = out.send(plain("created", Some(&handle.sid), data));
                    if let Some(attached) = attached {
                        attach(&handle, 0, out, attached).await;
                    }
                }
                Err(message) => fail(&message),
            }
        }
        // What the new-chat form starts with for a folder (the answer says which folder it is for, so a late one can be told).
        "project_defaults" => {
            let cwd = op["cwd"].as_str().unwrap_or_default();
            let _ = out.send(plain("project_defaults", None, state.registry.project_defaults(cwd)));
        }
        "attach" => match (state.registry.get(&sid), attached) {
            (_, None) => fail("this connection does not follow chats"),
            (Some(handle), Some(attached)) => attach(&handle, op["after_seq"].as_u64().unwrap_or(0), out, attached).await,
            (None, _) => fail("no such session"),
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
        "rename" => match state.registry.get(&sid) {
            Some(handle) => {
                if let Err(message) = handle.rename(op["title"].as_str().unwrap_or_default().to_owned()).await {
                    fail(&message);
                }
            }
            None => fail("no such session"),
        },
        "delete" => match state.registry.delete(&sid).await {
            Ok(()) => {
                let _ = out.send(plain("deleted", Some(&sid), json!({})));
            }
            Err(message) => fail(&message),
        },
        "set_model" => match state.registry.get(&sid) {
            Some(handle) => {
                let name = op["model"].as_str().unwrap_or_default();
                // Blank or "default" is the CLI's default; anything else must be a plain model name.
                match (crate::registry::clean_model(name), name.trim() == "default" || name.trim().is_empty()) {
                    (None, false) => fail("not a model name"),
                    (model, _) => {
                        if let Err(message) = handle.set_model(model).await {
                            fail(&message);
                        }
                    }
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
                // What to ask Claude Code to remember besides this call: one of two names or nothing; anything else is refused and
                // changes nothing (the request stays open). The rule itself is never sent by the page: the host builds it.
                let grant = match op.get("grant") {
                    None | Some(Value::Null) if remember => Ok(crate::session::Grant::Remember),
                    None | Some(Value::Null) => Ok(crate::session::Grant::None),
                    Some(Value::String(name)) if name == "session" => Ok(crate::session::Grant::Session),
                    Some(Value::String(name)) if name == "always" => Ok(crate::session::Grant::Always),
                    Some(_) => Err("unknown grant"),
                };
                match grant {
                    Err(message) => fail(message),
                    Ok(grant) => {
                        if let Err(message) = handle.answer(request_id, allow, message, grant).await {
                            fail(&message);
                        }
                    }
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
            // The account whose history is searched is named by the page; a chat named in the query must be of that account.
            let realm = match realm_of(state, op) {
                Ok(realm) => realm,
                Err(error) => return drop(out.send(plain("search_results", None, json!({"req": req, "hits": [], "error": error})))),
            };
            let (out, registry) = (out.clone(), state.registry.clone());
            // A search can take a moment: it runs beside the socket's other messages.
            tokio::spawn(async move {
                if let Some(sid) = &query.sid {
                    let of_account = match registry.get(sid) {
                        Some(handle) => handle.info().await.map(|i| i.meta.account.as_ref().map(|a| a.id.clone()) == realm.account && i.meta.provider.is_none()),
                        None => None,
                    };
                    if of_account != Some(true) {
                        return drop(out.send(plain("search_results", None, json!({"req": req, "hits": [], "error": "that chat belongs to another account"}))));
                    }
                }
                let found = match realm.index.clone() {
                    Some(index) => tokio::task::spawn_blocking(move || index.search(&query)).await.ok().and_then(Result::ok),
                    None => None,
                };
                let data = match found {
                    Some(hits) => {
                        // Titles only of this account's chats: a hit never names a chat of another account.
                        let titles: std::collections::HashMap<String, String> = registry.list().await.into_iter().filter(|i| i.meta.account.as_ref().map(|a| &a.id) == realm.account.as_ref()).map(|i| (i.meta.sid, i.meta.title)).collect();
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
        other if other.starts_with("fs_") => fs_ops::handle(state, other, op, out),
        other if other.starts_with("memory_") => memory_ops::handle(state, other, op, out),
        other if other.starts_with("skill_") => skill_ops::handle(state, other, op, out),
        other if other.starts_with("sched_") => sched_ops::handle(state, other, op, out),
        other if other.starts_with("provider_") => provider_ops::handle(state, other, op, out),
        other if other.starts_with("account_") => account_ops::handle(state, other, op, out),
        other if other.starts_with("cost_") => cost_ops::handle(state, other, op, out),
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
