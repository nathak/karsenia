//! The Skills tab's operations on the WebSocket. A person decides everything here: the page asks, the store checks and recomputes the
//! risk report on the text that would be written.
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::{plain, AppState};
use crate::skills::Edit;

fn changed(state: &AppState) {
    let _ = state.registry.events.send(json!({"v": 1, "kind": "skills_changed", "data": {}}));
}

fn result(out: &UnboundedSender<String>, rid: &Value, ok: bool, data: Value) {
    let mut body = data;
    body["ok"] = Value::Bool(ok);
    body["rid"] = rid.clone();
    let _ = out.send(plain("skill_result", None, body));
}

fn listing(state: &AppState) -> Value {
    let Some(store) = state.registry.skills() else {
        return json!({"available": false, "notice": state.registry.skills_notice});
    };
    let settings = state.registry.settings().get();
    json!({"available": true, "overview": store.overview(), "auto": settings.skills_auto, "notice": state.registry.skills_notice})
}

fn text(op: &Value, key: &str) -> Option<String> {
    op[key].as_str().map(str::to_owned)
}

pub fn handle(state: &AppState, name: &str, op: &Value, out: &UnboundedSender<String>) {
    let rid = op["rid"].clone();
    let Some(store) = state.registry.skills() else {
        let _ = out.send(plain("skills", None, listing(state)));
        return;
    };
    let id = op["id"].as_i64().unwrap_or(0);
    match name {
        "skill_list" => {
            let _ = out.send(plain("skills", None, listing(state)));
        }
        "skill_propose" => {
            // A proposal takes a model run: it is made in the background and announced when it is ready.
            let (Some(ctx), Some(handle)) = (state.registry.propose_ctx(), op["sid"].as_str().and_then(|sid| state.registry.get(sid))) else {
                return result(out, &rid, false, json!({"error": "open a chat first"}));
            };
            let scope: &'static str = if op["scope"].as_str() == Some("user") { "user" } else { "project" };
            let (hint, target) = (text(op, "hint"), text(op, "target").filter(|t| !t.is_empty()));
            let events = state.registry.events.clone();
            tokio::spawn(async move {
                if let Err(reason) = ctx.manual(&handle, hint, target, scope).await {
                    let _ = events.send(json!({"v": 1, "kind": "skill_notice", "data": {"text": reason}}));
                }
            });
            result(out, &rid, true, json!({"started": true}));
        }
        "skill_approve" => {
            let edit = match op.get("edit") {
                Some(e) if e.is_object() => Edit { name: text(e, "name"), description: text(e, "description"), body: text(e, "body") },
                _ => Edit::default(),
            };
            match store.approve(id, edit, op["ack_risk"].as_bool() == Some(true)) {
                Ok(skill) => {
                    changed(state);
                    result(out, &rid, true, json!({"skill": skill}));
                }
                Err(reason) => result(out, &rid, false, json!({"error": reason})),
            }
        }
        "skill_reject" => {
            let done = store.reject(id);
            if done {
                changed(state);
            }
            result(out, &rid, done, json!({}));
        }
        "skill_enable" => {
            let done = store.set_enabled(id, op["on"].as_bool() == Some(true));
            if done {
                changed(state);
            }
            result(out, &rid, done, json!({}));
        }
        "skill_delete" => {
            let done = store.delete(id);
            if done {
                changed(state);
            }
            result(out, &rid, done, json!({"undo": id}));
        }
        "skill_undo" => {
            let done = store.undo_delete(id);
            if done {
                changed(state);
            }
            result(out, &rid, done, json!({}));
        }
        "skill_versions" => {
            let _ = out.send(plain("skill_versions", None, json!({"id": id, "versions": store.versions(id)})));
        }
        "skill_rollback" => match store.rollback(id, op["n"].as_i64().unwrap_or(0)) {
            Ok(skill) => {
                changed(state);
                result(out, &rid, true, json!({"skill": skill}));
            }
            Err(reason) => result(out, &rid, false, json!({"error": reason})),
        },
        "skill_setting" => {
            if let Some(on) = op["auto"].as_bool() {
                state.registry.settings().update(|s| s.skills_auto = on);
            }
            changed(state);
            result(out, &rid, true, json!({}));
        }
        _ => result(out, &rid, false, json!({"error": "unknown skills operation"})),
    }
}
