//! The live view's web server: a page and a feed, read only. It binds 127.0.0.1, answers only GET, wants its own token on every `/api` call
//! (in a header: a token in an address would be kept in the history), and has no route to a file, to the data folder or to anything that
//! sends a message. The main app's server (`server::start`) is a different one with a different token.
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::derive::{Deriver, GuardFn};
use super::feed::{GuardMark, Prompts, Tab};
use super::mask::mask_text;
use crate::registry::Registry;
use crate::server::auth::{bearer_ok, cross_site, host_ok, origin_absent_or_own};
use crate::workbench::risk::Policy;
use crate::workbench::{hook, store, Profile};

/// The port tried first, and how many after it are tried when it is taken.
pub const DEFAULT_PORT: u16 = 4317;
const PORT_TRIES: u16 = 20;
/// The newest items a page is given when it opens (the rest of a long chat is read to keep the state right and not sent).
const KEEP: usize = 2000;

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

#[derive(Clone)]
struct LiveState {
    registry: Arc<Registry>,
    token: Arc<String>,
    port: u16,
    prompts: Arc<AtomicU8>,
    shutdown: watch::Receiver<bool>,
}

impl LiveState {
    fn prompts(&self) -> Prompts {
        if self.prompts.load(Ordering::Relaxed) == 1 { Prompts::Short } else { Prompts::Full }
    }
}

pub struct LiveServer {
    pub addr: SocketAddr,
    token: String,
    prompts: Arc<AtomicU8>,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl LiveServer {
    /// The address to open: the token rides in the fragment, which is never sent over HTTP (the page moves it out of the address bar).
    pub fn url(&self, sid: Option<&str>, lang: &str) -> String {
        let mut url = format!("http://127.0.0.1:{}/#t={}&lang={}", self.addr.port(), self.token, lang);
        if let Some(sid) = sid {
            url.push_str("&s=");
            url.push_str(sid);
        }
        url
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn prompts(&self) -> Prompts {
        if self.prompts.load(Ordering::Relaxed) == 1 { Prompts::Short } else { Prompts::Full }
    }

    /// What the pages that connect from now on are given of a prompt.
    pub fn set_prompts(&self, prompts: Prompts) {
        self.prompts.store(u8::from(prompts == Prompts::Short), Ordering::Relaxed);
    }

    /// Ends the server: the port is closed and the open feeds end.
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for LiveServer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        self.task.abort();
    }
}

async fn bind(port: u16) -> anyhow::Result<tokio::net::TcpListener> {
    if port == 0 {
        return Ok(tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?);
    }
    let mut last = None;
    for p in port..port.saturating_add(PORT_TRIES) {
        match tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], p))).await {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => last = Some(error),
            Err(error) => return Err(error.into()),
        }
    }
    Err(anyhow::anyhow!("no free port from {port} (the last: {})", last.map(|e| e.to_string()).unwrap_or_default()))
}

/// Starts the server on `port` (or the next free one; 0 for any). Needs a running tokio runtime.
pub async fn start(registry: Arc<Registry>, port: u16, prompts: Prompts) -> anyhow::Result<LiveServer> {
    let listener = bind(port).await?;
    let addr = listener.local_addr()?;
    let token = crate::random_hex(32);
    let (shutdown, watch) = watch::channel(false);
    let prompts_flag = Arc::new(AtomicU8::new(u8::from(prompts == Prompts::Short)));
    let state = LiveState { registry, token: Arc::new(token.clone()), port: addr.port(), prompts: prompts_flag.clone(), shutdown: watch };
    let app = Router::new()
        .route("/api/info", get(info))
        .route("/api/chats", get(chats))
        .route("/api/stream", get(stream))
        .fallback(fallback)
        .layer(middleware::from_fn_with_state(state.clone(), gate))
        .with_state(state);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(LiveServer { addr, token, prompts: prompts_flag, shutdown, task })
}

/// For every request: the Host must be this server's own, a page of another site is refused, `/api` wants the token; then the security
/// headers go on the answer. No CORS header is ever set.
async fn gate(State(state): State<LiveState>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    if !host_ok(headers, state.port) {
        return (StatusCode::MISDIRECTED_REQUEST, "wrong host").into_response();
    }
    if cross_site(headers) || !origin_absent_or_own(headers, state.port) {
        return (StatusCode::FORBIDDEN, "not from this page").into_response();
    }
    if request.uri().path().starts_with("/api/") && !bearer_ok(headers, &state.token) {
        return (StatusCode::UNAUTHORIZED, "token missing or wrong").into_response();
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

const ASSETS: [(&str, &str, &[u8]); 4] = [
    ("/", "text/html; charset=utf-8", include_bytes!("../../ui/live/index.html")),
    ("/live.js", "text/javascript; charset=utf-8", include_bytes!("../../ui/live/live.js")),
    ("/live.css", "text/css; charset=utf-8", include_bytes!("../../ui/live/live.css")),
    ("/favicon.png", "image/png", include_bytes!("../../ui/favicon.png")),
];

/// The page's own files and nothing else: any other path is 404, any other method 405 (a route of `/api` with a wrong method answers 405 on its own).
async fn fallback(method: Method, uri: Uri) -> Response {
    if method != Method::GET {
        return (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "GET")], "read only").into_response();
    }
    match ASSETS.iter().find(|(path, _, _)| *path == uri.path()) {
        Some((_, kind, bytes)) => ([(header::CONTENT_TYPE, *kind)], Bytes::from_static(bytes)).into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[derive(Serialize)]
struct Info {
    version: &'static str,
    prompts: &'static str,
}

async fn info(State(state): State<LiveState>) -> Json<Info> {
    Json(Info { version: env!("CARGO_PKG_VERSION"), prompts: state.prompts().name() })
}

async fn chats(State(state): State<LiveState>) -> Json<Vec<Tab>> {
    Json(tabs(&state.registry).await)
}

async fn tabs(registry: &Registry) -> Vec<Tab> {
    registry
        .list()
        .await
        .into_iter()
        .map(|info| {
            let state = serde_json::to_value(&info.state).ok().and_then(|v| v["state"].as_str().map(str::to_owned)).unwrap_or_default();
            let project = info.meta.cwd.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Tab { sid: info.meta.sid.clone(), title: mask_text(&info.meta.title, 80), project: mask_text(&project, 60), state, last_ts: info.meta.created, asks_open: info.pending as u32 }
        })
        .collect()
}

#[derive(Deserialize)]
struct StreamQuery {
    sid: String,
}

fn profile_name(profile: Profile) -> &'static str {
    match profile {
        Profile::Off => "off",
        Profile::Permissive => "low",
        Profile::Balanced => "medium",
        Profile::Strict => "high",
    }
}

/// What the guard would say about a call of this chat by its settings as they are when the page connects (a recomputation, marked as such).
fn guard_for(data: PathBuf, project: PathBuf, sid: &str) -> Option<GuardFn> {
    let wb = store::load(&data, sid).ok()?;
    Some(Box::new(move |tool, input| {
        let ev = hook::evaluate(&wb, &project, &project, tool, input).ok()?;
        Some(GuardMark { profile: profile_name(ev.profile), policy: ev.policy.name(), rule: ev.verdict.rule.map(Policy::name), derived: true })
    }))
}

/// One frame of a server-sent event stream (the data is one line of JSON).
fn frame(name: &str, id: Option<u64>, data: &impl Serialize) -> Option<Bytes> {
    let json = serde_json::to_string(data).ok()?;
    let id = id.map(|n| format!("id: {n}\n")).unwrap_or_default();
    Some(Bytes::from(format!("event: {name}\n{id}data: {json}\n\n")))
}

/// The feed of one chat: its newest items, then each new one as it happens, the list of chats when it changes and a keep-alive line.
/// It ends when the page goes, when the server stops, or (telling the page to start over) when the page fell too far behind.
async fn stream(State(state): State<LiveState>, Query(q): Query<StreamQuery>) -> Response {
    let Some(handle) = state.registry.get(&q.sid) else { return (StatusCode::NOT_FOUND, "no such chat").into_response() };
    let Some(info) = handle.info().await else { return (StatusCode::NOT_FOUND, "no such chat").into_response() };
    let Some((replay, mut events)) = handle.subscribe(0).await else { return (StatusCode::NOT_FOUND, "no such chat").into_response() };
    let guard = guard_for(state.registry.dir().to_path_buf(), info.meta.cwd.clone(), &q.sid);
    let mut deriver = Deriver::new(state.prompts(), guard);
    let registry = state.registry.clone();
    let mut shutdown = state.shutdown.clone();
    let (tx, out) = mpsc::channel::<Bytes>(512);
    tokio::spawn(async move {
        // Masking and the guard's recomputation of a long chat is work for a thread of its own, not for the ones the chats run on.
        let Ok((mut deriver, items)) = tokio::task::spawn_blocking(move || {
            let items: Vec<_> = replay.iter().flat_map(|e| deriver.push(e)).collect();
            (deriver, items)
        })
        .await
        else {
            return;
        };
        let skip = items.len().saturating_sub(KEEP);
        for item in &items[skip..] {
            let Some(bytes) = frame("item", Some(item.seq), item) else { continue };
            if tx.send(bytes).await.is_err() {
                return;
            }
        }
        let mut last_tabs = String::new();
        let mut tick = tokio::time::interval(Duration::from_secs(3));
        let mut keep_alive = tokio::time::interval(Duration::from_secs(15));
        let ready = Bytes::from_static(b"event: ready\ndata: {}\n\n");
        if tx.send(ready).await.is_err() {
            return;
        }
        loop {
            tokio::select! {
                got = events.recv() => match got {
                    Ok(envelope) => {
                        for item in deriver.push(&envelope) {
                            let Some(bytes) = frame("item", Some(item.seq), &item) else { continue };
                            if tx.send(bytes).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let _ = tx.send(Bytes::from_static(b"event: reset\ndata: {}\n\n")).await;
                        return;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        let _ = tx.send(Bytes::from_static(b"event: closed\ndata: {}\n\n")).await;
                        return;
                    }
                },
                _ = tick.tick() => {
                    let list = tabs(&registry).await;
                    let json = serde_json::to_string(&list).unwrap_or_default();
                    if json != last_tabs {
                        if let Some(bytes) = frame("tabs", None, &list) {
                            if tx.send(bytes).await.is_err() {
                                return;
                            }
                        }
                        last_tabs = json;
                    }
                }
                _ = keep_alive.tick() => {
                    if tx.send(Bytes::from_static(b": keep-alive\n\n")).await.is_err() {
                        return;
                    }
                }
                _ = shutdown.changed() => return,
                _ = tx.closed() => return,
            }
        }
    });
    let body = Body::from_stream(futures_util::stream::unfold(out, |mut rx| async move { rx.recv().await.map(|bytes| (Ok::<Bytes, Infallible>(bytes), rx)) }));
    match Response::builder().status(StatusCode::OK).header(header::CONTENT_TYPE, "text/event-stream").body(body) {
        Ok(response) => response,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
