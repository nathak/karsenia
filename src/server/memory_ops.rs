//! The Memory panel's operations on the WebSocket. Everything here is the person's own hand: the page asks, the store checks.
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::{plain, AppState};
use crate::memory::{Added, NewMem};
use crate::settings::Auto;

fn changed(state: &AppState) {
    let _ = state.registry.events.send(json!({"v": 1, "kind": "memory_changed", "data": {}}));
}

fn result(out: &UnboundedSender<String>, ok: bool, data: Value) {
    let mut body = data;
    body["ok"] = Value::Bool(ok);
    let _ = out.send(plain("memory_result", None, body));
}

fn listing(state: &AppState, cwd: Option<&str>) -> Value {
    let Some(store) = state.registry.memory() else {
        return json!({"available": false, "notice": state.registry.memory_notice});
    };
    let (active, pending) = store.counts();
    json!({
        "available": true,
        "overview": store.overview(cwd),
        "settings": state.registry.settings().get(),
        "counts": {"active": active, "pending": pending},
        "notice": state.registry.memory_notice,
        "cwd": cwd,
    })
}

pub fn handle(state: &AppState, name: &str, op: &Value, out: &UnboundedSender<String>) {
    let Some(store) = state.registry.memory() else {
        let _ = out.send(plain("memory", None, listing(state, None)));
        return;
    };
    let id = op["id"].as_i64().unwrap_or(0);
    let text = op["text"].as_str().unwrap_or_default();
    let confirm = op["confirm"].as_bool() == Some(true);
    match name {
        "memory_list" => {
            let _ = out.send(plain("memory", None, listing(state, op["cwd"].as_str())));
        }
        "memory_add" => {
            let scope: &'static str = if op["scope"].as_str() == Some("user") { "user" } else { "project" };
            let cwd = op["cwd"].as_str().map(str::to_owned);
            if scope == "project" && cwd.is_none() {
                return result(out, false, json!({"error": "open a chat first: a project note belongs to its folder"}));
            }
            let kind = match op["kind"].as_str() {
                Some(k @ ("preference" | "fact" | "project")) => k.to_owned(),
                _ => "fact".to_owned(),
            };
            let note = NewMem { scope, cwd: if scope == "project" { cwd } else { None }, kind, text: text.to_owned(), source: "manual", sid: None, seq: None, status: "active", instr: false, allow_secret: confirm };
            match store.add(note) {
                Added::Saved(mem) => {
                    changed(state);
                    result(out, true, json!({"saved": mem}));
                }
                Added::Duplicate(id) => result(out, false, json!({"error": "the same note is already saved", "duplicate": id})),
                Added::Refused(reason) => result(out, false, json!({"error": reason, "needs_confirm": reason.starts_with("it looks like")})),
            }
        }
        "memory_edit" => match store.edit(id, text, confirm) {
            Ok(()) => {
                changed(state);
                result(out, true, json!({}));
            }
            Err(reason) => result(out, false, json!({"error": reason, "needs_confirm": reason.starts_with("it looks like")})),
        },
        "memory_delete" | "memory_reject" => {
            let done = store.delete(id);
            if done {
                changed(state);
            }
            result(out, done, json!({"undo": id}));
        }
        "memory_undo" => {
            let done = store.undo_delete(id);
            if done {
                changed(state);
            }
            result(out, done, if done { json!({}) } else { json!({"error": "the same words are saved again, or it is gone"}) });
        }
        "memory_approve" => {
            let done = if op["instruction"].as_bool() == Some(true) { store.approve_instruction(id) } else { store.approve(id) };
            if done {
                changed(state);
            }
            result(out, done, json!({}));
        }
        "memory_pin" => {
            let done = store.set_pinned(id, op["pinned"].as_bool() == Some(true));
            if done {
                changed(state);
            }
            result(out, done, json!({}));
        }
        "memory_setting" => {
            let settings = state.registry.settings();
            if let Some(auto) = op["auto"].as_str().and_then(Auto::parse) {
                settings.update(|s| s.memory_auto = auto);
            }
            if op["card_seen"].as_bool() == Some(true) {
                settings.update(|s| s.memory_card_seen = true);
            }
            changed(state);
            result(out, true, json!({}));
        }
        "memory_export" => {
            let _ = out.send(plain("memory_export", None, store.export()));
        }
        _ => result(out, false, json!({"error": "unknown memory operation"})),
    }
}
