//! The host's end of the MCP socket (a named pipe on Windows): a stdio server (`tenrec mcp`) of one chat asks here, with that
//! chat's secret.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixListener;

use crate::index::SearchQuery;
use crate::mcp::{format_excerpts, format_notes, McpGate};
use crate::memory::{Added, NewMem};
use crate::realm::Realm;
use crate::registry::Registry;

const MAX_REQUEST: u64 = 64 * 1024;

#[cfg(unix)]
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

#[cfg(unix)]
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

/// Windows: the first instance of the pipe; it fails when another program already holds the name.
#[cfg(windows)]
pub fn listen(gate: &McpGate) -> Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    Ok(pipe::instance(gate.sock.as_os_str(), true)?)
}

/// Windows: one connection per pipe instance; the next instance is made before the connected one is handed on, so a caller
/// never finds the name missing.
#[cfg(windows)]
pub async fn serve(mut server: tokio::net::windows::named_pipe::NamedPipeServer, gate: Arc<McpGate>, registry: Arc<Registry>) {
    loop {
        if let Err(error) = server.connect().await {
            tracing::warn!("the MCP pipe could not accept: {error}");
            tokio::time::sleep(Duration::from_millis(100)).await;
            if let Ok(fresh) = pipe::instance(gate.sock.as_os_str(), false) {
                server = fresh;
            }
            continue;
        }
        let next = loop {
            match pipe::instance(gate.sock.as_os_str(), false) {
                Ok(next) => break next,
                Err(error) => {
                    tracing::warn!("the MCP pipe could not make its next instance: {error}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        };
        let connected = std::mem::replace(&mut server, next);
        let (gate, registry) = (gate.clone(), registry.clone());
        tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(20), one(connected, gate, registry)).await;
        });
    }
}

#[cfg(windows)]
mod pipe {
    use std::ffi::{c_void, OsStr};
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::sync::OnceLock;

    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// One instance of the pipe, open only to this user and the system, and never to another machine.
    pub fn instance(name: &OsStr, first: bool) -> io::Result<NamedPipeServer> {
        let attributes = attributes()?;
        // SAFETY: `attributes` points at a SECURITY_ATTRIBUTES that lives for the whole program (made once, never freed).
        unsafe { ServerOptions::new().first_pipe_instance(first).reject_remote_clients(true).create_with_security_attributes_raw(name, attributes as *mut c_void) }
    }

    /// The access list `D:P(A;;GA;;;<this user>)(A;;GA;;;SY)`, made once. The default one would let everyone read.
    fn attributes() -> io::Result<usize> {
        static MADE: OnceLock<Result<usize, String>> = OnceLock::new();
        MADE.get_or_init(|| make().map_err(|e| e.to_string())).clone().map_err(io::Error::other)
    }

    fn make() -> io::Result<usize> {
        let sddl: Vec<u16> = OsStr::new(&format!("D:P(A;;GA;;;{})(A;;GA;;;SY)", user_sid()?)).encode_wide().chain(Some(0)).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: a NUL-ended string in, a descriptor out that is kept for the whole program.
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let attributes = Box::new(SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: descriptor, bInheritHandle: 0 });
        Ok(Box::into_raw(attributes) as usize)
    }

    /// The SID of the user this program runs as, as text (`S-1-5-21-...`).
    fn user_sid() -> io::Result<String> {
        // SAFETY: plain Win32 calls; the token handle and the string are released here.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
            // u64 words, so the TOKEN_USER read from the start is aligned.
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut text: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut end = 0;
            while *text.add(end) != 0 {
                end += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, end));
            LocalFree(text.cast());
            Ok(sid)
        }
    }
}

async fn one<S: AsyncRead + AsyncWrite + Unpin>(stream: S, gate: Arc<McpGate>, registry: Arc<Registry>) {
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
    let Some(grant) = request["secret"].as_str().and_then(|secret| gate.grant_of(secret)) else { return fail("not allowed") };
    let sid = grant.sid.clone();
    let Some(info) = (match registry.get(&sid) {
        Some(handle) => handle.info().await,
        None => None,
    }) else {
        return fail("the chat is gone");
    };
    // What the secret was given for and what the chat is now must agree, and only the realm of that account is reachable from here.
    if info.meta.account.as_ref().map(|a| &a.id) != grant.account.as_ref() || info.meta.provider.is_some() {
        return fail("not allowed");
    }
    let Ok(realm) = registry.realm(grant.account.as_deref()) else { return fail("this account is not available") };
    if realm.is_retired() {
        return fail("this account is not available");
    }
    let cwd = info.meta.cwd.to_string_lossy().into_owned();
    let params = &request["params"];
    match request["method"].as_str().unwrap_or_default() {
        crate::mcp::TOOL_NAME => search_history(params, &cwd, &realm, registry).await,
        crate::mcp::MEMORY_SEARCH => memory_search(params, &cwd, &realm),
        crate::mcp::MEMORY_SAVE => memory_save(params, &sid, &cwd, gate, &realm, registry),
        _ => fail("unknown method"),
    }
}

async fn search_history(params: &Value, cwd: &str, realm: &Realm, registry: &Registry) -> Value {
    let fail = |reason: &str| json!({"ok": false, "error": reason});
    let Some(index) = realm.index.clone() else { return fail("the search index is not available") };
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
    // Titles only of this account's chats: a hit never names a chat of another account.
    let titles: HashMap<String, String> = registry.list().await.into_iter().filter(|i| i.meta.account.as_ref().map(|a| &a.id) == realm.account.as_ref()).map(|i| (i.meta.sid, i.meta.title)).collect();
    json!({"ok": true, "text": format_excerpts(&hits, &titles)})
}

fn memory_search(params: &Value, cwd: &str, realm: &Realm) -> Value {
    let fail = |reason: &str| json!({"ok": false, "error": reason});
    let Some(store) = realm.memory.clone() else { return fail("memory is not available") };
    let words = params["query"].as_str().unwrap_or_default().trim().to_owned();
    if words.is_empty() {
        return fail("query is required");
    }
    let limit = params["limit"].as_u64().unwrap_or(5).clamp(1, 8) as usize;
    json!({"ok": true, "text": format_notes(&store.search(&words, cwd, limit))})
}

fn memory_save(params: &Value, sid: &str, cwd: &str, gate: &McpGate, realm: &Realm, registry: &Registry) -> Value {
    let fail = |reason: &str| json!({"ok": false, "error": reason});
    let Some(store) = realm.memory.clone() else { return fail("memory is not available") };
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
            let _ = registry.events.send(json!({"v": 1, "kind": "memory_changed", "data": {"account": realm.account}}));
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
