//! Who may talk to this server: only a page opened from the address printed at start, carrying the run's token.
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::AppState;

/// Equal without stopping at the first difference.
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

/// The Host header must be this server's own address (a page on another name pointed at 127.0.0.1 is refused: DNS rebinding).
pub fn host_ok(headers: &HeaderMap, port: u16) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else { return false };
    let host = host.to_ascii_lowercase();
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

/// A WebSocket must come from a page served by this server: the Origin is exactly one of its own addresses.
pub fn origin_ok(headers: &HeaderMap, port: u16) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else { return false };
    origin == format!("http://127.0.0.1:{port}") || origin == format!("http://localhost:{port}")
}

/// A request a page of another site made (the browser says so with `Sec-Fetch-Site`): refused by the servers that only their own page may use.
pub fn cross_site(headers: &HeaderMap) -> bool {
    headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()).is_some_and(|v| v.eq_ignore_ascii_case("cross-site"))
}

/// No Origin (a plain request), or exactly one of this server's own addresses.
pub fn origin_absent_or_own(headers: &HeaderMap, port: u16) -> bool {
    headers.get(header::ORIGIN).is_none() || origin_ok(headers, port)
}

pub fn bearer_ok(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|given| ct_eq(given, token))
        .unwrap_or(false)
}

const CSP: &str = "default-src 'self'; img-src 'self' blob: data:; style-src 'self'; script-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// Runs for every request: Host check first, then the security headers on the answer.
pub async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if !host_ok(request.headers(), state.port) {
        return (StatusCode::MISDIRECTED_REQUEST, "wrong host").into_response();
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        map
    }

    #[test]
    fn host_and_origin_must_be_this_servers_own() {
        assert!(host_ok(&headers(&[("host", "127.0.0.1:4000")]), 4000));
        assert!(host_ok(&headers(&[("host", "LOCALHOST:4000")]), 4000));
        assert!(!host_ok(&headers(&[("host", "evil.test:4000")]), 4000));
        assert!(!host_ok(&headers(&[("host", "127.0.0.1:4001")]), 4000));
        assert!(!host_ok(&headers(&[]), 4000));
        assert!(origin_ok(&headers(&[("origin", "http://127.0.0.1:4000")]), 4000));
        assert!(!origin_ok(&headers(&[("origin", "http://evil.test")]), 4000));
        assert!(!origin_ok(&headers(&[("origin", "https://127.0.0.1:4000")]), 4000));
        assert!(!origin_ok(&headers(&[]), 4000), "a missing Origin is refused");
    }

    #[test]
    fn the_token_is_compared_whole() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
        assert!(!ct_eq("", "a"));
        assert!(bearer_ok(&headers(&[("authorization", "Bearer tok")]), "tok"));
        assert!(!bearer_ok(&headers(&[("authorization", "Bearer tok2")]), "tok"));
        assert!(!bearer_ok(&headers(&[("authorization", "tok")]), "tok"));
        assert!(!bearer_ok(&headers(&[]), "tok"));
    }
}
