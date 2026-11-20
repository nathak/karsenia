//! Model providers: other Anthropic-compatible endpoints a chat can be made against, kept in `<data>/providers.json` (0600).
//!
//! Only the person makes or changes them (WebSocket `provider_*` operations; no MCP tool). A token is never handed out again:
//! the page gets `ProviderView` (`has_token`, the host), the chat's meta gets `ProviderRef` (id, name, host), and nothing here
//! prints a token in an error message or in `Debug`.
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const MAX_PROVIDERS: usize = 20;
const MAX_TOKEN: usize = 4096;
const MAX_URL: usize = 300;

/// How the token is presented to the endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Auth {
    /// `Authorization: Bearer <token>` (ANTHROPIC_AUTH_TOKEN): gateways and proxies.
    #[default]
    Bearer,
    /// `x-api-key: <token>` (ANTHROPIC_API_KEY): an Anthropic-style API key.
    XApiKey,
    /// The key is sent only where the provider's header lines say `{key}`: `Authorization` carries a placeholder, not the key (a gateway
    /// that takes its key in a header of its own and may refuse an `Authorization` it does not know).
    Headers,
    /// The CLI is given the provider's subscription token (`CLAUDE_CODE_OAUTH_TOKEN`, made by `claude setup-token`) as its own login, and no
    /// `ANTHROPIC_AUTH_TOKEN`: for a gateway that passes a Claude subscription through. That token is sent to the provider's address.
    OauthToken,
    /// The chat's `claude` runs in the folder of one of the person's accounts and uses that account's own login (as it would without any
    /// gateway variable): the login token is sent to the provider's address. No key or token of the provider's own is used for the login.
    AccountLogin,
}

impl Auth {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "bearer" => Some(Self::Bearer),
            "x_api_key" => Some(Self::XApiKey),
            "headers" => Some(Self::Headers),
            "oauth_token" => Some(Self::OauthToken),
            "account_login" => Some(Self::AccountLogin),
            _ => None,
        }
    }
}

/// Names the endpoint's own models are given under (the three tiers Claude Code asks for by family).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Aliases {
    #[serde(default)]
    pub opus: Option<String>,
    #[serde(default)]
    pub sonnet: Option<String>,
    #[serde(default)]
    pub haiku: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default)]
    pub token: Option<String>,
    /// The subscription token of `Auth::OauthToken` (a separate secret from `token`, which is what `{key}` stands for).
    #[serde(default)]
    pub oauth_token: Option<String>,
    /// For `Auth::AccountLogin`: the id of the account whose login is used; None is the default account.
    #[serde(default)]
    pub login_account: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub models: Aliases,
    /// Extra request headers, one `Name: value` per line, where `{key}` stands for the key (a gateway that wants its key in a header of its
    /// own). The key is never in this text itself: only where it goes.
    #[serde(default)]
    pub headers: Option<String>,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub updated: u64,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider").field("id", &self.id).field("name", &self.name).field("host", &host_of(&self.base_url)).field("token", &self.token.as_ref().map(|_| "[hidden]")).finish()
    }
}

/// What the page reads: never the token.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub host: String,
    pub auth: Auth,
    pub has_token: bool,
    pub has_oauth: bool,
    pub login_account: Option<String>,
    pub model: Option<String>,
    pub models: Aliases,
    /// The header lines as the person wrote them (with `{key}`, never the key).
    pub headers: String,
}

/// What a chat remembers of its provider (no token, no full address).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRef {
    pub id: String,
    pub name: String,
    pub host: String,
}

impl Provider {
    pub fn view(&self) -> ProviderView {
        ProviderView { id: self.id.clone(), name: self.name.clone(), base_url: self.base_url.clone(), host: host_of(&self.base_url), auth: self.auth, has_token: self.token.is_some(), has_oauth: self.oauth_token.is_some(), login_account: self.login_account.clone(), model: self.model.clone(), models: self.models.clone(), headers: self.headers.clone().unwrap_or_default() }
    }

    pub fn reference(&self) -> ProviderRef {
        ProviderRef { id: self.id.clone(), name: self.name.clone(), host: host_of(&self.base_url) }
    }
}

/// `host[:port]` of an address that passed `validate_base_url`.
pub fn host_of(base_url: &str) -> String {
    let rest = base_url.split_once("://").map(|(_, r)| r).unwrap_or(base_url);
    rest.split('/').next().unwrap_or_default().to_owned()
}

/// True when `host[:port]` names this computer itself.
pub fn is_local_host(host: &str) -> bool {
    let name = if host.starts_with('[') { host.split_once(']').map_or(host, |(h, _)| h).to_owned() + "]" } else { host.split(':').next().unwrap_or_default().to_owned() };
    matches!(name.as_str(), "localhost" | "127.0.0.1" | "[::1]")
}

/// Checks and normalises an address; returns it without a trailing slash. The error never repeats the input.
///
/// `https` anywhere; plain `http` only to this machine (a local gateway). No user name, query or fragment: the address is put
/// into a settings flag and an environment variable, and the person is shown its host.
pub fn validate_base_url(text: &str) -> Result<String, &'static str> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_URL {
        return Err("the address is empty or too long");
    }
    if text.chars().any(|c| c.is_control() || c.is_whitespace() || "@?#\\\"'`<>{}|^".contains(c)) {
        return Err("the address holds a character that is not allowed");
    }
    let (scheme, rest) = text.split_once("://").ok_or("the address must start with https://")?;
    let scheme = scheme.to_ascii_lowercase();
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        let (h, after) = inner.split_once(']').ok_or("the host is not valid")?;
        (format!("[{h}]"), after.strip_prefix(':'))
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h.to_owned(), Some(p)),
            None => (authority.to_owned(), None),
        }
    };
    let host = host.to_ascii_lowercase();
    let host_ok = host == "[::1]" || (!host.is_empty() && host.len() <= 253 && host.split('.').all(|label| !label.is_empty() && label.len() <= 63 && !label.starts_with('-') && !label.ends_with('-') && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')));
    if !host_ok {
        return Err("the host is not valid");
    }
    if let Some(port) = port {
        if port.is_empty() || port.len() > 5 || !port.chars().all(|c| c.is_ascii_digit()) || port.parse::<u32>().map_or(true, |p| p == 0 || p > 65535) {
            return Err("the port is not valid");
        }
    }
    let local = is_local_host(&host);
    match scheme.as_str() {
        "https" => {}
        "http" if local => {}
        "http" => return Err("an address outside this machine must use https://"),
        _ => return Err("the address must start with https://"),
    }
    if !path.chars().all(|c| c.is_ascii_alphanumeric() || "-._~/".contains(c)) || path.contains("//") || path.split('/').any(|s| s == "..") {
        return Err("the path of the address is not valid");
    }
    let authority = match port {
        Some(p) => format!("{host}:{p}"),
        None => host,
    };
    Ok(format!("{scheme}://{authority}{}", path.trim_end_matches('/')))
}

pub fn validate_name(text: &str) -> Result<String, &'static str> {
    let name = text.trim();
    if name.is_empty() || name.chars().count() > 40 || name.chars().any(|c| c.is_control()) {
        return Err("the name must be 1 to 40 characters");
    }
    Ok(name.to_owned())
}

pub fn validate_token(text: &str) -> Result<String, &'static str> {
    let token = text.trim();
    // Under 8 characters is not a key, and a chat's output is scrubbed of a key only from 8 characters on.
    if token.len() < 8 || token.len() > MAX_TOKEN || !token.chars().all(|c| c.is_ascii_graphic()) {
        return Err("the key must be 8 to 4096 characters, with no space or character that is not plain text");
    }
    Ok(token.to_owned())
}

const MAX_HEADER_LINES: usize = 8;
const MAX_HEADER_VALUE: usize = 500;
/// Headers the app sets itself or that would break the request if set twice.
const RESERVED_HEADERS: [&str; 8] = ["host", "content-length", "content-type", "transfer-encoding", "connection", "upgrade", "authorization", "x-api-key"];

/// Checks the header lines of the form: `Name: value` one to a line, `{key}` where the provider's key goes. Returns them tidied (one line
/// each, joined by a newline), or None when there are none. The error never repeats the input.
pub fn validate_headers(text: &str, has_key: bool) -> Result<Option<String>, &'static str> {
    let mut lines = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for line in text.split('\n').map(str::trim).filter(|l| !l.is_empty()) {
        if lines.len() >= MAX_HEADER_LINES {
            return Err("there are too many header lines");
        }
        let (name, value) = line.split_once(':').ok_or("a header line must be written Name: value")?;
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err("a header name has a character that is not allowed");
        }
        let lower = name.to_ascii_lowercase();
        if RESERVED_HEADERS.contains(&lower.as_str()) {
            return Err("that header is set by the app and cannot be changed here");
        }
        if seen.contains(&lower) {
            return Err("a header name is written twice");
        }
        seen.push(lower);
        if value.is_empty() || value.len() > MAX_HEADER_VALUE || !value.chars().all(|c| c == ' ' || c.is_ascii_graphic()) {
            return Err("a header value is empty, too long, or has a character that is not plain text");
        }
        let rest = value.replace("{key}", "");
        if rest.contains('{') || rest.contains('}') {
            return Err("a header value may hold braces only as {key}");
        }
        if value.contains("{key}") && !has_key {
            return Err("a header uses {key} but the provider has no key");
        }
        lines.push(format!("{name}: {value}"));
    }
    Ok((!lines.is_empty()).then(|| lines.join("\n")))
}

impl Provider {
    /// Every secret of this provider that a chat must never show or store (what a chat's events are scrubbed of): the key and the
    /// subscription token, those long enough to be told apart from other text.
    pub fn secrets(&self) -> Vec<String> {
        [&self.token, &self.oauth_token].into_iter().flatten().filter(|t| t.len() >= 8).cloned().collect()
    }

    /// The value of `ANTHROPIC_CUSTOM_HEADERS` for a chat: the lines with the key put where `{key}` stands. None when there are no lines.
    pub fn custom_headers(&self) -> Option<String> {
        let template = self.headers.as_ref()?;
        Some(template.replace("{key}", self.token.as_deref().unwrap_or_default()))
    }
}

/// A model name as Claude Code takes it (the same characters a chat's own model may use).
pub fn validate_model(text: &str) -> Result<Option<String>, &'static str> {
    let model = text.trim();
    if model.is_empty() {
        return Ok(None);
    }
    if model.len() > 100 || model.starts_with('-') || !model.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/[]".contains(c)) {
        return Err("a model name holds a character that is not allowed");
    }
    Ok(Some(model.to_owned()))
}

/// What the person typed in the form.
#[derive(Debug, Default)]
pub struct Form {
    pub id: Option<String>,
    pub name: String,
    pub base_url: String,
    pub auth: Auth,
    /// None keeps the key already saved.
    pub token: Option<String>,
    pub clear_token: bool,
    /// The subscription token; None keeps the one already saved.
    pub oauth_token: Option<String>,
    pub clear_oauth: bool,
    /// For `Auth::AccountLogin`: the account whose login is used (None: the default account).
    pub account: Option<String>,
    pub model: String,
    pub models: [String; 3],
    /// The header lines; empty for none.
    pub headers: String,
}

pub struct ProviderStore {
    path: PathBuf,
    inner: Mutex<Vec<Provider>>,
}

impl ProviderStore {
    /// An unreadable file opens as an empty list and is left where it is (never overwritten by opening); the notice says so.
    pub fn open(path: PathBuf) -> (Self, Option<String>) {
        let (list, notice) = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<File>(&text) {
                Ok(file) => (file.providers, None),
                Err(_) => (Vec::new(), Some("providers.json could not be read; no provider is available until it is fixed or removed".to_owned())),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None),
            Err(_) => (Vec::new(), Some("providers.json could not be read".to_owned())),
        };
        (Self { path, inner: Mutex::new(list) }, notice)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Provider>> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn list_views(&self) -> Vec<ProviderView> {
        self.lock().iter().map(Provider::view).collect()
    }

    pub fn get(&self, id: &str) -> Option<Provider> {
        self.lock().iter().find(|p| p.id == id).cloned()
    }

    fn write(&self, list: &[Provider]) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&File { v: 1, providers: list.to_vec() }).map_err(|e| e.to_string())?;
        crate::private::write(&self.path, &text).map_err(|e| format!("could not save: {e}"))
    }

    /// Makes a provider, or changes the one named by `form.id`. Returns its id.
    pub fn save(&self, form: Form, now: u64) -> Result<String, String> {
        let name = validate_name(&form.name)?;
        let base_url = validate_base_url(&form.base_url)?;
        let model = validate_model(&form.model)?;
        let models = Aliases { opus: validate_model(&form.models[0])?, sonnet: validate_model(&form.models[1])?, haiku: validate_model(&form.models[2])? };
        let mut guard = self.lock();
        // Changed on a copy and kept only once the file is written: a save that fails changes nothing that a chat can start with.
        let mut list = guard.clone();
        let existing = match &form.id {
            Some(id) => Some(list.iter().position(|p| &p.id == id).ok_or("that provider is gone")?),
            None => None,
        };
        let token = match (&form.token, form.clear_token) {
            (Some(text), _) => Some(validate_token(text)?),
            (None, true) => None,
            (None, false) => existing.and_then(|i| list[i].token.clone()),
        };
        if form.auth == Auth::XApiKey && token.is_none() {
            return Err("an API key address needs the key".into());
        }
        let oauth_token = match (&form.oauth_token, form.clear_oauth) {
            (Some(text), _) => Some(validate_token(text).map_err(|_| "the subscription token must be 8 to 4096 characters, with no space or character that is not plain text")?),
            (None, true) => None,
            (None, false) => existing.and_then(|i| list[i].oauth_token.clone()),
        };
        let login_account = match (form.auth, &form.account) {
            (Auth::AccountLogin, Some(id)) if !crate::accounts::valid_id(id) => return Err("that account is not valid".into()),
            (Auth::AccountLogin, account) => account.clone(),
            _ => None,
        };
        if form.auth == Auth::OauthToken && oauth_token.is_none() {
            return Err("a subscription token address needs the token".into());
        }
        let headers = validate_headers(&form.headers, token.is_some())?;
        if list.iter().enumerate().any(|(i, p)| p.name == name && Some(i) != existing) {
            return Err("another provider has this name".into());
        }
        let id = match existing {
            Some(i) => {
                let p = &mut list[i];
                (p.name, p.base_url, p.auth, p.token, p.oauth_token, p.login_account, p.model, p.models, p.headers, p.updated) = (name, base_url, form.auth, token, oauth_token, login_account, model, models, headers, now);
                p.id.clone()
            }
            None => {
                if list.len() >= MAX_PROVIDERS {
                    return Err("there are too many providers".into());
                }
                let id = crate::random_hex(6);
                list.push(Provider { id: id.clone(), name, base_url, auth: form.auth, token, oauth_token, login_account, model, models, headers, created: now, updated: now });
                id
            }
        };
        self.write(&list)?;
        *guard = list;
        Ok(id)
    }

    /// Ok(false) when there was no such provider; Err when the file could not be written (the provider stays, key and all).
    pub fn delete(&self, id: &str) -> Result<bool, String> {
        let mut guard = self.lock();
        let mut list = guard.clone();
        list.retain(|p| p.id != id);
        if list.len() == guard.len() {
            return Ok(false);
        }
        self.write(&list)?;
        *guard = list;
        Ok(true)
    }
}

#[derive(Serialize, Deserialize)]
struct File {
    #[serde(default)]
    v: u8,
    #[serde(default)]
    providers: Vec<Provider>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(name: &str) -> Form {
        Form { name: name.into(), base_url: "https://gateway.example.com/anthropic/".into(), token: Some("sk-secret-1234".into()), ..Default::default() }
    }

    #[test]
    fn addresses_are_checked_and_normalised() {
        for (given, wanted) in [
            ("https://gateway.example.com", "https://gateway.example.com"),
            ("HTTPS://Gateway.Example.com/anthropic/", "https://gateway.example.com/anthropic"),
            ("https://example.com:8443/a/b", "https://example.com:8443/a/b"),
            ("http://127.0.0.1:8080", "http://127.0.0.1:8080"),
            ("http://localhost:11434/", "http://localhost:11434"),
            ("http://[::1]:9000", "http://[::1]:9000"),
        ] {
            assert_eq!(validate_base_url(given).as_deref(), Ok(wanted), "{given}");
        }
        for bad in [
            "",
            "example.com",
            "http://example.com",
            "http://192.168.0.5:8080",
            "ftp://example.com",
            "https://user@example.com",
            "https://user:pw@example.com",
            "https://example.com?x=1",
            "https://example.com/#frag",
            "https://exa mple.com",
            "https://example.com:0",
            "https://example.com:99999",
            "https://example.com:",
            "https://",
            "https://-.example.com",
            "https://example..com",
            "https://example.com/a/../b",
            "https://example.com//x",
            "https://example.com/\"quote",
            "https://exa\nmple.com",
            &format!("https://example.com/{}", "a".repeat(300)),
        ] {
            assert!(validate_base_url(bad).is_err(), "{bad:?} must be refused");
        }
        let error = validate_base_url("https://user:hunter2@example.com").unwrap_err();
        assert!(!error.contains("hunter2"), "an error never repeats what was typed");
    }

    #[test]
    fn this_computer_is_told_from_other_hosts() {
        for local in ["localhost", "localhost:11434", "127.0.0.1", "127.0.0.1:8080", "[::1]", "[::1]:9000"] {
            assert!(is_local_host(local), "{local}");
        }
        for other in ["gw.example.com", "gw.example.com:443", "localhost.evil.com", "127.0.0.2", "10.0.0.1:80", "[::2]"] {
            assert!(!is_local_host(other), "{other}");
        }
    }

    #[test]
    fn the_view_the_reference_and_debug_never_hold_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let (store, notice) = ProviderStore::open(dir.path().join("providers.json"));
        assert!(notice.is_none());
        let id = store.save(form("Gateway"), 5).unwrap();
        let provider = store.get(&id).unwrap();
        for text in [serde_json::to_string(&provider.view()).unwrap(), serde_json::to_string(&provider.reference()).unwrap(), format!("{provider:?}"), serde_json::to_string(&store.list_views()).unwrap()] {
            assert!(!text.contains("sk-secret-1234"), "{text}");
        }
        assert!(provider.view().has_token);
        assert_eq!(provider.reference().host, "gateway.example.com");
    }

    #[test]
    fn saving_keeps_or_clears_the_key_and_the_file_is_private_and_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.json");
        let (store, _) = ProviderStore::open(path.clone());
        let id = store.save(form("Gateway"), 1).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

        store.save(Form { id: Some(id.clone()), name: "Gateway 2".into(), base_url: "https://gateway.example.com".into(), ..Default::default() }, 2).unwrap();
        assert_eq!(store.get(&id).unwrap().token.as_deref(), Some("sk-secret-1234"), "no new key typed: the old one stays");
        store.save(Form { id: Some(id.clone()), name: "Gateway 2".into(), base_url: "https://gateway.example.com".into(), token: None, clear_token: true, ..Default::default() }, 3).unwrap();
        assert_eq!(store.get(&id).unwrap().token, None);

        let (again, _) = ProviderStore::open(path);
        assert_eq!(again.get(&id).unwrap().name, "Gateway 2");
        assert_eq!(again.get(&id).unwrap().updated, 3);
        assert_eq!(again.delete(&id), Ok(true));
        assert_eq!(again.delete(&id), Ok(false));
    }

    #[test]
    fn a_save_or_delete_that_cannot_be_written_changes_nothing_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("providers.json");
        std::fs::create_dir(path.parent().unwrap()).unwrap();
        let (store, _) = ProviderStore::open(path.clone());
        let id = store.save(form("A"), 1).unwrap();
        // The folder goes away: nothing can be written.
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert!(store.save(Form { id: Some(id.clone()), name: "Renamed".into(), base_url: "https://other.example.com".into(), ..Default::default() }, 2).is_err());
        assert_eq!(store.get(&id).unwrap().name, "A", "a rejected edit changed what chats start with");
        assert!(store.save(form("B"), 2).is_err());
        assert_eq!(store.list_views().len(), 1, "a rejected new provider is in the list");
        assert!(store.delete(&id).is_err());
        assert!(store.get(&id).is_some(), "a delete that failed to persist removed it from memory");
    }

    #[test]
    fn bad_forms_are_refused_and_a_broken_file_is_not_overwritten_by_opening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.json");
        let (store, _) = ProviderStore::open(path.clone());
        store.save(form("A"), 1).unwrap();
        assert!(store.save(form("A"), 1).is_err(), "names are unique");
        assert!(store.save(Form { auth: Auth::XApiKey, token: None, ..form("B") }, 1).is_err(), "an API key address needs the key");
        assert!(store.save(Form { token: Some("has space".into()), ..form("C") }, 1).is_err());
        assert!(store.save(Form { token: Some("short".into()), ..form("C2") }, 1).is_err(), "a key under 8 characters would not be scrubbed from the chat");
        assert!(store.save(Form { model: "--flag".into(), ..form("D") }, 1).is_err());
        assert!(store.save(Form { models: ["".into(), "bad name".into(), "".into()], ..form("E") }, 1).is_err());
        assert!(store.save(Form { base_url: "http://example.com".into(), ..form("F") }, 1).is_err());
        assert!(store.save(Form { id: Some("nope".into()), ..form("G") }, 1).is_err());
        assert_eq!(store.list_views().len(), 1, "nothing was made by the refused forms");

        std::fs::write(&path, "not json").unwrap();
        let (broken, notice) = ProviderStore::open(path.clone());
        assert!(notice.is_some() && broken.list_views().is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json", "opening leaves the file as it is");
    }

    #[test]
    fn header_lines_are_checked_and_tidied() {
        let ok = validate_headers("  AIR-Api-Key:   {key}  \n\nAIR-Coding-Agent-Mode: subscription\n", true).unwrap();
        assert_eq!(ok.as_deref(), Some("AIR-Api-Key: {key}\nAIR-Coding-Agent-Mode: subscription"));
        assert_eq!(validate_headers("  \n ", false), Ok(None), "no lines is no headers");
        for bad in [
            "no colon here",
            ": value",
            "Bad Name: v",
            "Name: ",
            "Authorization: x",
            "X-API-KEY: x",
            "Host: example.com",
            "A: 1\na: 2",
            "A: {oops}",
            "A: {key",
            "A: tab\there",
            "A: caf\u{e9}",
        ] {
            assert!(validate_headers(bad, true).is_err(), "{bad:?}");
        }
        assert!(validate_headers("A: {key}", false).is_err(), "{{key}} with no key saved");
        assert!(validate_headers("A: plain", false).is_ok(), "a header with no key in it needs none");
        assert!(validate_headers(&(0..9).map(|i| format!("H{i}: v")).collect::<Vec<_>>().join("\n"), true).is_err(), "too many lines");
        assert!(validate_headers(&format!("A: {}", "x".repeat(501)), true).is_err());
    }

    #[test]
    fn headers_are_saved_shown_without_the_key_kept_on_edit_and_rendered_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = ProviderStore::open(dir.path().join("providers.json"));
        let mut f = form("Air");
        f.token = Some("airkey-123456".into());
        f.headers = "AIR-Api-Key: {key}\nAIR-Coding-Agent-Mode: subscription".into();
        let id = store.save(f, 1).unwrap();
        let view = store.list_views().remove(0);
        assert_eq!(view.headers, "AIR-Api-Key: {key}\nAIR-Coding-Agent-Mode: subscription");
        assert!(!serde_json::to_string(&view).unwrap().contains("airkey-123456"), "the page never gets the key");
        let p = store.get(&id).unwrap();
        assert_eq!(p.custom_headers().as_deref(), Some("AIR-Api-Key: airkey-123456\nAIR-Coding-Agent-Mode: subscription"));
        assert!(!format!("{p:?}").contains("airkey-123456"), "Debug hides the key");
        // An edit with no new key keeps the saved one for {key}; clearing the key while a header uses {key} is refused.
        let keep = Form { id: Some(id.clone()), name: "Air".into(), base_url: "https://gateway.example.com/anthropic".into(), headers: "AIR-Api-Key: {key}".into(), ..Default::default() };
        store.save(keep, 2).unwrap();
        assert_eq!(store.get(&id).unwrap().custom_headers().as_deref(), Some("AIR-Api-Key: airkey-123456"));
        let clear = Form { id: Some(id.clone()), name: "Air".into(), base_url: "https://gateway.example.com/anthropic".into(), clear_token: true, headers: "AIR-Api-Key: {key}".into(), ..Default::default() };
        assert!(store.save(clear, 3).is_err());
        assert!(store.get(&id).unwrap().token.is_some(), "a refused save changes nothing");
        // Empty lines remove the headers.
        let none = Form { id: Some(id.clone()), name: "Air".into(), base_url: "https://gateway.example.com/anthropic".into(), ..Default::default() };
        store.save(none, 4).unwrap();
        assert!(store.get(&id).unwrap().custom_headers().is_none());
        // A file written before headers existed has none.
        let old: Provider = serde_json::from_str(r#"{"id":"x","name":"N","base_url":"https://a.example"}"#).unwrap();
        assert!(old.headers.is_none() && old.custom_headers().is_none());
    }

    #[test]
    fn a_subscription_token_is_kept_apart_from_the_key_never_shown_and_needed_for_its_mode() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = ProviderStore::open(dir.path().join("providers.json"));
        let base = || Form { name: "Air".into(), base_url: "https://gateway.example.com/anthropic".into(), auth: Auth::OauthToken, ..Default::default() };
        assert!(store.save(base(), 1).is_err(), "the mode needs the token");
        assert!(store.save(Form { oauth_token: Some("short".into()), ..base() }, 1).is_err(), "too short to be a token");
        let id = store.save(Form { oauth_token: Some("sk-ant-oat01-subscription-token".into()), token: Some("airkey-123456".into()), headers: "AIR-Api-Key: {key}".into(), ..base() }, 1).unwrap();
        let view = store.list_views().remove(0);
        assert!(view.has_oauth && view.has_token);
        let shown = serde_json::to_string(&view).unwrap();
        assert!(!shown.contains("sk-ant-oat01") && !shown.contains("airkey-123456"), "{shown}");
        let p = store.get(&id).unwrap();
        assert_eq!(p.custom_headers().as_deref(), Some("AIR-Api-Key: airkey-123456"), "{{key}} is the key, not the subscription token");
        assert_eq!(p.secrets().len(), 2);
        assert!(!format!("{p:?}").contains("sk-ant-oat01"));
        // An edit with no new token keeps it; clearing it while the mode needs it is refused and changes nothing.
        store.save(Form { id: Some(id.clone()), ..base() }, 2).unwrap();
        assert_eq!(store.get(&id).unwrap().oauth_token.as_deref(), Some("sk-ant-oat01-subscription-token"));
        assert!(store.save(Form { id: Some(id.clone()), clear_oauth: true, ..base() }, 3).is_err());
        assert!(store.get(&id).unwrap().oauth_token.is_some());
        // Another mode may drop it.
        store.save(Form { id: Some(id.clone()), auth: Auth::Bearer, clear_oauth: true, ..base() }, 4).unwrap();
        assert!(store.get(&id).unwrap().oauth_token.is_none());
        let old: Provider = serde_json::from_str(r#"{"id":"x","name":"N","base_url":"https://a.example"}"#).unwrap();
        assert!(old.oauth_token.is_none() && old.secrets().is_empty());
    }

    #[test]
    fn an_account_login_provider_names_an_account_or_the_default_and_nothing_else_keeps_one() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = ProviderStore::open(dir.path().join("providers.json"));
        let base = || Form { name: "Air".into(), base_url: "https://gateway.example.com/anthropic".into(), auth: Auth::AccountLogin, ..Default::default() };
        let id = store.save(base(), 1).unwrap();
        assert_eq!(store.get(&id).unwrap().login_account, None, "the default account");
        store.save(Form { id: Some(id.clone()), account: Some("0123456789abcdef".into()), ..base() }, 2).unwrap();
        assert_eq!(store.list_views().remove(0).login_account.as_deref(), Some("0123456789abcdef"));
        for bad in ["", "short", "../../etc/passwd0", "0123456789abcdeg", "0123456789ABCDEF"] {
            assert!(store.save(Form { id: Some(id.clone()), account: Some(bad.into()), ..base() }, 3).is_err(), "{bad:?}");
        }
        assert_eq!(store.get(&id).unwrap().login_account.as_deref(), Some("0123456789abcdef"), "a refused save changes nothing");
        // Another mode forgets the account, and no token of the provider's own is needed for the login.
        store.save(Form { id: Some(id.clone()), auth: Auth::Bearer, account: Some("0123456789abcdef".into()), ..base() }, 4).unwrap();
        assert_eq!(store.get(&id).unwrap().login_account, None);
        assert_eq!(Auth::parse("account_login"), Some(Auth::AccountLogin));
    }
}
