//! The Memory panel's operations on the WebSocket. Everything here is the person's own hand: the page asks, the store checks.
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::{plain, realm_of, AppState};
use crate::memory::{Added, NewMem};
use crate::realm::Realm;
use crate::settings::Auto;

/// Every page hears which account's notes changed, and redraws only when it is showing that account.
fn changed(state: &AppState, realm: &Realm) {
    let _ = state.registry.events.send(json!({"v": 1, "kind": "memory_changed", "data": {"account": realm.account}}));
}

fn result(out: &UnboundedSender<String>, rid: &Value, ok: bool, data: Value) {
    let mut body = data;
    body["ok"] = Value::Bool(ok);
    body["rid"] = rid.clone();
    let _ = out.send(plain("memory_result", None, body));
}

fn listing(state: &AppState, realm: &Realm, cwd: Option<&str>) -> Value {
    let Some(store) = realm.memory.clone() else {
        return json!({"available": false, "notice": realm.memory_notice, "account": realm.account});
    };
    let (active, pending) = store.counts();
    json!({
        "available": true,
        "overview": store.overview(cwd),
        "settings": state.registry.settings().get(),
        "counts": {"active": active, "pending": pending},
        "notice": realm.memory_notice,
        "cwd": cwd,
        "account": realm.account,
    })
}

pub fn handle(state: &AppState, name: &str, op: &Value, out: &UnboundedSender<String>) {
    let rid = op["rid"].clone();
    // The settings are the person's, for every account: they name no account.
    if name == "memory_setting" {
        let settings = state.registry.settings();
        if let Some(auto) = op["auto"].as_str().and_then(Auto::parse) {
            settings.update(|s| s.memory_auto = auto);
        }
        if op["card_seen"].as_bool() == Some(true) {
            settings.update(|s| s.memory_card_seen = true);
        }
        let _ = state.registry.events.send(json!({"v": 1, "kind": "memory_changed", "data": {"account": Value::Null, "all": true}}));
        return result(out, &rid, true, json!({}));
    }
    // Every other operation names the account whose notes it means ("account": an id, or null for the default one): a message without
    // the key is refused, so no page can act on an account it did not choose.
    let realm = match realm_of(state, op) {
        Ok(realm) => realm,
        Err(error) if name == "memory_list" => return drop(out.send(plain("memory", None, json!({"available": false, "notice": error})))),
        Err(error) => return result(out, &rid, false, json!({"error": error})),
    };
    let Some(store) = realm.memory.clone() else {
        let _ = out.send(plain("memory", None, listing(state, &realm, None)));
        return;
    };
    let id = op["id"].as_i64().unwrap_or(0);
    let text = op["text"].as_str().unwrap_or_default();
    let confirm = op["confirm"].as_bool() == Some(true);
    match name {
        "memory_list" => {
            let _ = out.send(plain("memory", None, listing(state, &realm, op["cwd"].as_str().map(crate::platform::strip_verbatim))));
        }
        "memory_add" => {
            let scope: &'static str = if op["scope"].as_str() == Some("user") { "user" } else { "project" };
            // The page may send the folder with the `\\?\` Windows puts before it; notes are filed without it.
            let cwd = op["cwd"].as_str().map(|c| crate::platform::strip_verbatim(c).to_owned());
            if scope == "project" && cwd.is_none() {
                return result(out, &rid, false, json!({"error": "open a chat first: a project note belongs to its folder"}));
            }
            let kind = match op["kind"].as_str() {
                Some(k @ ("preference" | "fact" | "project")) => k.to_owned(),
                _ => "fact".to_owned(),
            };
            let note = NewMem { scope, cwd: if scope == "project" { cwd } else { None }, kind, text: text.to_owned(), source: "manual", sid: None, seq: None, status: "active", instr: false, allow_secret: confirm };
            match store.add(note) {
                Added::Saved(mem) => {
                    changed(state, &realm);
                    result(out, &rid, true, json!({"saved": mem}));
                }
                Added::Duplicate(id) => result(out, &rid, false, json!({"error": "the same note is already saved", "duplicate": id})),
                Added::Refused(reason) => result(out, &rid, false, json!({"error": reason, "needs_confirm": reason.starts_with("it looks like")})),
            }
        }
        "memory_edit" => match store.edit(id, text, confirm) {
            Ok(()) => {
                changed(state, &realm);
                result(out, &rid, true, json!({}));
            }
            Err(reason) => result(out, &rid, false, json!({"error": reason, "needs_confirm": reason.starts_with("it looks like")})),
        },
        "memory_delete" | "memory_reject" => {
            let done = store.delete(id);
            if done {
                changed(state, &realm);
            }
            result(out, &rid, done, json!({"undo": id}));
        }
        "memory_undo" => {
            let done = store.undo_delete(id);
            if done {
                changed(state, &realm);
            }
            result(out, &rid, done, if done { json!({}) } else { json!({"error": "the same words are saved again, or it is gone"}) });
        }
        "memory_approve" => {
            let done = if op["instruction"].as_bool() == Some(true) { store.approve_instruction(id) } else { store.approve(id) };
            if done {
                changed(state, &realm);
            }
            result(out, &rid, done, json!({}));
        }
        "memory_pin" => {
            let done = store.set_pinned(id, op["pinned"].as_bool() == Some(true));
            if done {
                changed(state, &realm);
            }
            result(out, &rid, done, json!({}));
        }
        "memory_export" => {
            let _ = out.send(plain("memory_export", None, store.export()));
        }
        _ => result(out, &rid, false, json!({"error": "unknown memory operation"})),
    }
}
