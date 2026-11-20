//! The text of a chat's debug view: what the chat is, how its `claude` was last started, the last events (as they came, one line each)
//! and the end of the CLI's own debug log when that is on. It answers "why does nothing happen?": an error the CLI printed, an endpoint that
//! was tried again, a process that ended. Every piece of text from outside goes through `mask_text` (secrets hidden, invisible characters out).
use serde_json::Value;

use super::i18n::{fill, tr, Lang};
use crate::live::mask::mask_text;
use crate::schedule::{Clock, SystemClock};
use crate::session::Envelope;

/// The most events shown (the newest ones); older ones are said to be left out.
pub const MAX_EVENTS: usize = 300;
/// The longest one event is shown.
const LINE: usize = 400;
/// The longest one argument or environment value is shown.
const ITEM: usize = 600;
/// The most of the CLI's debug log shown (its end).
const LOG_LINES: usize = 400;

/// What the head of the view says of the chat.
pub struct DebugHead {
    pub sid: String,
    pub state: String,
    pub model: Option<String>,
    /// "→ host" for a provider chat, else the account's name or empty.
    pub endpoint: String,
    /// Whether chats started from now on write the CLI's debug log.
    pub api_log: bool,
}

fn clock(ts: u64) -> String {
    if ts == 0 {
        return "--:--:--".to_owned();
    }
    let l = SystemClock.local(ts as i64);
    format!("{:02}:{:02}:{:02}", l.hour, l.minute, (ts / 1000) % 60)
}

fn text_of(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())
}

/// One event in a few words: what a person scanning for the failure needs, not the whole of it.
fn summary(e: &Envelope) -> String {
    let d = &e.data;
    let s = |key: &str| d.get(key).map(text_of).unwrap_or_default();
    match e.kind.as_str() {
        "user" => format!("text: {}", s("text")),
        "error" => format!("message: {}", s("message")),
        "stderr" => format!("text: {}", s("text")),
        "state" => {
            let mut text = s("state");
            if let Some(code) = d.get("code").filter(|c| !c.is_null()) {
                text.push_str(&format!(" (exit code {code})"));
            }
            text
        }
        "permission" => format!("{} {}", s("tool_name"), s("request_id")),
        "permission_resolved" => format!("{} {}", s("outcome"), s("request_id")),
        "spawn" => "claude started (see the section above)".to_owned(),
        "claude" => claude_summary(d),
        _ => d.to_string(),
    }
}

fn claude_summary(d: &Value) -> String {
    let kind = d.get("type").and_then(Value::as_str).unwrap_or("?");
    let sub = d.get("subtype").and_then(Value::as_str);
    let head = match sub {
        Some(sub) => format!("{kind}/{sub}"),
        None => kind.to_owned(),
    };
    match (kind, sub) {
        ("result", _) => {
            let mut parts = vec![format!("is_error={}", d.get("is_error").map(text_of).unwrap_or_else(|| "?".into()))];
            for key in ["api_error_status", "num_turns", "total_cost_usd", "duration_ms"] {
                if let Some(v) = d.get(key).filter(|v| !v.is_null()) {
                    parts.push(format!("{key}={}", text_of(v)));
                }
            }
            if let Some(text) = d.get("result").and_then(Value::as_str) {
                parts.push(format!("result: {text}"));
            }
            format!("{head} {}", parts.join(" "))
        }
        ("system", Some("api_retry")) => {
            let pick = |keys: &[&str]| keys.iter().filter_map(|k| d.get(*k).filter(|v| !v.is_null()).map(|v| format!("{k}={}", text_of(v)))).collect::<Vec<_>>().join(" ");
            format!("{head} {}", pick(&["attempt", "max_retries", "retry_delay_ms", "error_status", "error"]))
        }
        ("system", Some("init")) => format!("{head} model={} tools={}", d.get("model").map(text_of).unwrap_or_default(), d.get("tools").and_then(Value::as_array).map_or(0, Vec::len)),
        ("assistant", _) => {
            let blocks = d.pointer("/message/content").and_then(Value::as_array).cloned().unwrap_or_default();
            let parts: Vec<String> = blocks
                .iter()
                .map(|b| match b.get("type").and_then(Value::as_str) {
                    Some("text") => format!("text: {}", b.get("text").map(text_of).unwrap_or_default()),
                    Some("tool_use") => format!("tool_use {}", b.get("name").map(text_of).unwrap_or_default()),
                    Some(other) => other.to_owned(),
                    None => "?".to_owned(),
                })
                .collect();
            format!("{head} {}", parts.join(" | "))
        }
        _ => format!("{head} {}", d.to_string().chars().take(200).collect::<String>()),
    }
}

/// The whole text of the view.
pub fn debug_text(head: &DebugHead, events: &[Envelope], log_tail: Option<&str>, lang: Lang) -> String {
    let mut out = String::new();
    let one_line = |text: &str, max: usize| mask_text(&text.replace(['\n', '\r'], " "), max);
    out.push_str(&format!("== {} ==\n", tr(lang, "dbg_chat")));
    out.push_str(&format!("sid: {}\nstate: {}\nmodel: {}\n", head.sid, head.state, head.model.as_deref().unwrap_or("-")));
    if !head.endpoint.is_empty() {
        out.push_str(&format!("{}: {}\n", tr(lang, "dbg_endpoint"), mask_text(&head.endpoint, ITEM)));
    }
    out.push_str(&format!("{}: {}\n\n", tr(lang, "dbg_api_log_state"), tr(lang, if head.api_log { "dbg_on" } else { "dbg_off" })));

    out.push_str(&format!("== {} ==\n", tr(lang, "dbg_spawn")));
    match events.iter().rev().find(|e| e.kind == "spawn") {
        Some(e) => {
            let d = &e.data;
            out.push_str(&format!("{}  bin: {}\ncwd: {}\n", clock(e.ts), one_line(&text_of(&d["bin"]), ITEM), one_line(&text_of(&d["cwd"]), ITEM)));
            out.push_str("args:\n");
            for a in d["args"].as_array().into_iter().flatten() {
                out.push_str(&format!("  {}\n", one_line(&text_of(a), ITEM)));
            }
            out.push_str("env:\n");
            for pair in d["env"].as_array().into_iter().flatten() {
                out.push_str(&format!("  {} = {}\n", one_line(&text_of(&pair[0]), 100), one_line(&text_of(&pair[1]), ITEM)));
            }
            let removed: Vec<String> = d["env_removed"].as_array().into_iter().flatten().map(text_of).collect();
            if !removed.is_empty() {
                out.push_str(&format!("env removed: {}\n", one_line(&removed.join(", "), ITEM)));
            }
        }
        None => out.push_str(&format!("{}\n", tr(lang, "dbg_no_spawn"))),
    }
    out.push('\n');

    let shown: Vec<&Envelope> = events.iter().collect();
    let cut = shown.len().saturating_sub(MAX_EVENTS);
    out.push_str(&format!("== {} ==\n", tr(lang, "dbg_events")));
    if cut > 0 {
        out.push_str(&format!("{}\n", fill(&tr(lang, "dbg_events_cut"), &[("n", cut.to_string())])));
    }
    for e in &shown[cut..] {
        out.push_str(&format!("{} #{} {}  {}\n", clock(e.ts), e.seq, e.kind, one_line(&summary(e), LINE)));
    }
    out.push('\n');

    out.push_str(&format!("== {} ==\n", tr(lang, "dbg_api_log")));
    match log_tail {
        Some(text) => {
            let lines: Vec<&str> = text.lines().collect();
            let from = lines.len().saturating_sub(LOG_LINES);
            if from > 0 {
                out.push_str(&format!("{}\n", fill(&tr(lang, "dbg_events_cut"), &[("n", from.to_string())])));
            }
            for line in &lines[from..] {
                out.push_str(&mask_text(line, LINE * 2));
                out.push('\n');
            }
        }
        None => out.push_str(&format!("{}\n", tr(lang, if head.api_log { "dbg_log_empty" } else { "dbg_log_off" }))),
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn ev(seq: u64, kind: &str, data: Value) -> Envelope {
        Envelope { v: 1, sid: "s1".into(), seq, ts: 1_700_000_000_000 + seq * 1000, kind: kind.into(), data }
    }

    fn head(api_log: bool) -> DebugHead {
        DebugHead { sid: "s1".into(), state: "idle".into(), model: Some("sonnet".into()), endpoint: "→ gw.example.com".into(), api_log }
    }

    #[test]
    fn the_view_shows_the_start_the_events_in_order_and_what_went_wrong() {
        let events = vec![
            ev(1, "spawn", json!({"bin": "claude", "cwd": "/p", "args": ["-p", "--resume", "abc"], "env": [["ANTHROPIC_BASE_URL", "https://gw.example.com/x"], ["ANTHROPIC_AUTH_TOKEN", "(set, 13 characters)"]], "env_removed": ["ANTHROPIC_API_KEY"], "container": false})),
            ev(2, "user", json!({"text": "hello there"})),
            ev(3, "claude", json!({"type": "system", "subtype": "api_retry", "attempt": 1, "max_retries": 10, "error_status": 401, "error": "authentication_failed"})),
            ev(4, "claude", json!({"type": "result", "subtype": "success", "is_error": true, "result": "API Error: 401 invalid key", "total_cost_usd": 0.0})),
            ev(5, "stderr", json!({"text": "connection refused"})),
            ev(6, "state", json!({"state": "exited", "code": 1})),
        ];
        let text = debug_text(&head(false), &events, None, Lang::En);
        for want in ["sid: s1", "→ gw.example.com", "args:", "--resume", "ANTHROPIC_BASE_URL = https://gw.example.com/x", "ANTHROPIC_AUTH_TOKEN = (set, 13 characters)", "env removed: ANTHROPIC_API_KEY", "#2 user  text: hello there", "system/api_retry attempt=1 max_retries=10 error_status=401 error=authentication_failed", "result/success is_error=true", "API Error: 401 invalid key", "text: connection refused", "exited (exit code 1)"] {
            assert!(text.contains(want), "{want:?} missing in:\n{text}");
        }
        let at = |needle: &str| text.find(needle).unwrap_or(usize::MAX);
        assert!(at("#2 user") < at("api_retry") && at("api_retry") < at("#5 stderr"), "events in the order they came");
    }

    #[test]
    fn a_chat_that_never_started_and_a_missing_log_are_said_plainly() {
        let text = debug_text(&head(false), &[ev(1, "user", json!({"text": "hi"}))], None, Lang::En);
        assert!(text.contains(&tr(Lang::En, "dbg_no_spawn")) && text.contains(&tr(Lang::En, "dbg_log_off")), "{text}");
        let on = debug_text(&head(true), &[], None, Lang::En);
        assert!(on.contains(&tr(Lang::En, "dbg_log_empty")), "{on}");
    }

    #[test]
    fn secrets_and_hidden_characters_never_get_through_and_only_the_newest_events_are_shown() {
        let key = format!("sk-ant-{}", "a1b2c3d4e5".repeat(6));
        let mut events = vec![ev(1, "user", json!({"text": format!("my key is {key} and\u{202E} more")}))];
        for i in 0..(MAX_EVENTS as u64 + 20) {
            events.push(ev(i + 2, "state", json!({"state": "running"})));
        }
        let log = format!("request id=1\nAuthorization: Bearer abc.def.ghi123456\n{key}\n");
        let text = debug_text(&head(true), &events, Some(&log), Lang::En);
        assert!(!text.contains("a1b2c3d4e5") && !text.contains("ghi123456") && !text.contains('\u{202E}'), "{text}");
        assert!(text.contains("request id=1"), "the log's own lines stay");
        assert!(!text.contains("#1 user"), "the oldest were left out");
        assert!(text.contains(&format!("#{} state", MAX_EVENTS + 21)), "the newest are in");
        assert!(text.contains(&fill(&tr(Lang::En, "dbg_events_cut"), &[("n", "21".into())])), "and it says how many were left out");
    }

    #[test]
    fn a_long_event_is_cut_to_one_line() {
        let long = "x ".repeat(2000);
        let text = debug_text(&head(false), &[ev(1, "error", json!({"message": format!("first\nsecond {long}")}))], None, Lang::En);
        let line = text.lines().find(|l| l.contains("#1 error")).expect("the line");
        assert!(line.contains("first second") && line.chars().count() < 460, "{}", line.chars().count());
    }
}
