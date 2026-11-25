//! The version the `claude` program says it is (the `claude_code_version` of its `init` event), kept per program (a path, or a container
//! image) for the rest of this run. It is only used to write the program's own User-Agent back (see `env::with_user_agent`): no extra
//! process is started to ask for it, so the first start of a program has none yet.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::container::Spec;

static KNOWN: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

/// A version that is safe to put into a header line: 1 to 32 characters of letters, digits, `.`, `+` and `-`, and nothing else (no space,
/// no line break, no `{}` or `:`), so a program cannot make the line say anything but a version.
pub fn clean(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty() && text.len() <= 32 && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))).then(|| text.to_owned())
}

/// The version an `init` event reports, if it is a clean one.
pub fn from_init(raw: &serde_json::Value) -> Option<String> {
    raw["claude_code_version"].as_str().and_then(clean)
}

/// Which program a chat runs: the container's image (its own program, inside it), or the host's `claude` path.
pub fn key(bin: &str, container: Option<&Spec>) -> String {
    match container {
        Some(spec) => format!("container:{}", spec.runtime.image),
        None => bin.to_owned(),
    }
}

pub fn remember(key: &str, version: String) {
    if let Ok(mut map) = KNOWN.get_or_init(Default::default).lock() {
        map.insert(key.to_owned(), version);
    }
}

pub fn known(key: &str) -> Option<String> {
    KNOWN.get_or_init(Default::default).lock().ok()?.get(key).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_plain_version_is_taken() {
        for good in ["2.1.326", "2.1.326-beta+3", " 2.1.326\n"] {
            assert!(clean(good).is_some(), "{good:?}");
        }
        assert_eq!(clean(" 2.1.326\n").as_deref(), Some("2.1.326"));
        for bad in ["", "   ", "2.1.326\r\nX-Evil: 1", "2.1.326 (x)", "{key}", "1:2", "２.１", &"1".repeat(33)] {
            assert!(clean(bad).is_none(), "{bad:?}");
        }
        assert!(clean(env!("CARGO_PKG_VERSION")).is_some());
    }

    #[test]
    fn the_version_is_read_from_the_init_event() {
        assert_eq!(from_init(&json!({"claude_code_version": "2.1.326"})).as_deref(), Some("2.1.326"));
        assert_eq!(from_init(&json!({"type": "system"})), None);
        assert_eq!(from_init(&json!({"claude_code_version": 2})), None);
        assert_eq!(from_init(&json!({"claude_code_version": "1\r\nHost: x"})), None);
    }

    #[test]
    fn a_version_is_kept_for_its_own_program_only() {
        remember("/cli-version-test/claude", "9.9.9".into());
        assert_eq!(known("/cli-version-test/claude").as_deref(), Some("9.9.9"));
        assert_eq!(known("container:cli-version-test-image"), None);
        assert_eq!(key("/cli-version-test/claude", None), "/cli-version-test/claude");
    }
}
