//! The host's end of the MCP socket: a stdio server (`smart-workbench-app mcp`) of one chat asks here, with that chat's secret.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::index::SearchQuery;
use crate::mcp::{format_excerpts, format_notes, McpGate};
use crate::memory::{Added, NewMem};
use crate::registry::Registry;

const MAX_REQUEST: u64 = 64 * 1024;

pub fn listen(gate: &McpGate) -> Result<UnixListener> {
    let _ = std::fs::remove_file(&gate.sock);
    let listener = UnixListener::bind(&gate.sock)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gate.sock, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(listener)
}

pub async fn serve(listener: UnixListener, gate: Arc<McpGate>, registry: Arc<Registry>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let (gate, registry) = (gate.clone(), registry.clone());
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(20), one(stream, gate, registry)).await;
                });
            }
            // A failed accept (too many open files, say) is not the end of the socket.
            Err(error) => {
                tracing::warn!("the MCP socket could not accept: {error}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn one(stream: UnixStream, gate: Arc<McpGate>, registry: Arc<Registry>) {
    let mut reader = BufReader::new(stream).take(MAX_REQUEST);
    let mut line = String::new();
    if reader.read_line(&mut line).await.is_err() {
        return;
    }
    let answer = match serde_json::from_str::<Value>(&line) {
        Ok(request) => answer(&request, &gate, &registry).await,
        Err(_) => json!({"ok": false, "error": "unreadable request"}),
    };
    let mut stream = reader.into_inner().into_inner();
    let _ = stream.write_all(format!("{answer}\n").as_bytes()).await;
}

async fn answer(request: &Value, gate: &McpGate, registry: &Registry) -> Value {
    let fail = |reason: &str| json!({"ok": false, "error": reason});
    // The chat is told by the secret it was started with, never by anything the model says.
    let Some(sid) = request["secret"].as_str().and_then(|secret| gate.sid_of(secret)) else { return fail("not allowed") };
    let Some(info) = (match registry.get(&sid) {
        Some(handle) => handle.info().await,
        None => None,
    }) else {
        return fail("the chat is gone");
    };
    let cwd = info.meta.cwd.to_string_lossy().into_owned();
    let params = &request["params"];
    match request["method"].as_str().unwrap_or_default() {
        crate::mcp::TOOL_NAME => search_history(params, &cwd, registry).await,
        crate::mcp::MEMORY_SEARCH => memory_search(params, &cwd, registry),
        crate::mcp::MEMORY_SAVE => memory_save(params, &sid, &cwd, gate, registry),
        _ => fail("unknown method"),
    }
}

async fn search_history(params: &Value, cwd: &str, registry: &Registry) -> Value {
    let fail = |reason: &str| json!({"ok": false, "error": reason});
    let Some(index) = registry.index() else { return fail("the search index is not available") };
    let words = params["query"].as_str().unwrap_or_default().trim().to_owned();
    if words.is_empty() {
        return fail("query is required");
    }
    let since = params["since_days"].as_u64().filter(|d| *d > 0).map(|d| now_ms().saturating_sub(d.saturating_mul(86_400_000)));
    let limit = params["limit"].as_u64().unwrap_or(5).clamp(1, 6) as usize;
    // Always this chat's project: a rule that allows the tool once must not also allow a wider search, so there is no wider one here.
    let query = SearchQuery { q: words, cwd: Some(cwd.to_owned()), sid: None, since, limit };

    let hits = match tokio::task::spawn_blocking(move || index.search(&query)).await {
        Ok(Ok(hits)) => hits,
        _ => return fail("the search failed"),
    };
    let titles: HashMap<String, String> = registry.list().await.into_iter().map(|i| (i.meta.sid, i.meta.title)).collect();
    json!({"ok": true, "text": format_excerpts(&hits, &titles)})
}

fn memory_search(params: &Value, cwd: &str, registry: &Registry) -> Value {
    let fail = |reason: &str| json!({"ok": false, "error": reason});
    let Some(store) = registry.memory() else { return fail("memory is not available") };
    let words = params["query"].as_str().unwrap_or_default().trim().to_owned();
    if words.is_empty() {
        return fail("query is required");
    }
    let limit = params["limit"].as_u64().unwrap_or(5).clamp(1, 8) as usize;
    json!({"ok": true, "text": format_notes(&store.search(&words, cwd, limit))})
}

fn memory_save(params: &Value, sid: &str, cwd: &str, gate: &McpGate, registry: &Registry) -> Value {
    let fail = |reason: &str| json!({"ok": false, "error": reason});
    let Some(store) = registry.memory() else { return fail("memory is not available") };
    if !gate.allow_save(sid, now_ms(), 5) {
        return fail("too many saves in a minute; try again later");
    }
    let text = params["text"].as_str().unwrap_or_default().to_owned();
    let kind = match params["kind"].as_str() {
        Some(k @ ("preference" | "fact" | "project")) => k.to_owned(),
        _ => "fact".to_owned(),
    };
    let scope: &'static str = if params["scope"].as_str() == Some("user") { "user" } else { "project" };
    // The person saw this exact text in the permission dialog if a ticket exists for it; a note about the person always waits.
    let approved = gate.take_ticket(sid, &params.to_string());
    let status = if approved && scope == "project" { "active" } else { "pending" };
    let note = NewMem { scope, cwd: (scope == "project").then(|| cwd.to_owned()), kind, text, source: "claude", sid: Some(sid.to_owned()), seq: None, status, instr: false, allow_secret: false };
    match store.add(note) {
        Added::Saved(mem) => {
            let _ = registry.events.send(json!({"v": 1, "kind": "memory_changed", "data": {}}));
            let text = if mem.status == "active" { "Saved.".to_owned() } else { "Saved; it waits for the person's approval in the Memory panel and is not used until then.".to_owned() };
            json!({"ok": true, "text": text})
        }
        Added::Duplicate(_) => json!({"ok": true, "text": "That note is already saved."}),
        Added::Refused(reason) => fail(&format!("Not saved: {reason}")),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
