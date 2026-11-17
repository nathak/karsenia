//! The Skills tab's operations on the WebSocket. A person decides everything here: the page asks, the store checks and recomputes the
//! risk report on the text that would be written.
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::{plain, realm_of, AppState};
use crate::realm::Realm;
use crate::skills::Edit;

/// Every page hears which account's skills changed, and redraws only when it is showing that account.
fn changed(state: &AppState, realm: &Realm) {
    let _ = state.registry.events.send(json!({"v": 1, "kind": "skills_changed", "data": {"account": realm.account}}));
}

fn result(out: &UnboundedSender<String>, rid: &Value, ok: bool, data: Value) {
    let mut body = data;
    body["ok"] = Value::Bool(ok);
    body["rid"] = rid.clone();
    let _ = out.send(plain("skill_result", None, body));
}

fn listing(state: &AppState, realm: &Realm) -> Value {
    let Some(store) = realm.skills.clone() else {
        return json!({"available": false, "notice": realm.skills_notice, "account": realm.account});
    };
    let settings = state.registry.settings().get();
    json!({"available": true, "overview": store.overview(), "auto": settings.skills_auto, "notice": realm.skills_notice, "account": realm.account})
}

fn text(op: &Value, key: &str) -> Option<String> {
    op[key].as_str().map(str::to_owned)
}

pub fn handle(state: &AppState, name: &str, op: &Value, out: &UnboundedSender<String>) {
    let rid = op["rid"].clone();
    // The setting is the person's, for every account: it names no account.
    if name == "skill_setting" {
        if let Some(on) = op["auto"].as_bool() {
            state.registry.settings().update(|s| s.skills_auto = on);
        }
        let _ = state.registry.events.send(json!({"v": 1, "kind": "skills_changed", "data": {"account": Value::Null, "all": true}}));
        return result(out, &rid, true, json!({}));
    }
    // Every other operation names the account whose skills it means ("account": an id, or null for the default one).
    let realm = match realm_of(state, op) {
        Ok(realm) => realm,
        Err(error) if name == "skill_list" => return drop(out.send(plain("skills", None, json!({"available": false, "notice": error})))),
        Err(error) => return result(out, &rid, false, json!({"error": error})),
    };
    let Some(store) = realm.skills.clone() else {
        let _ = out.send(plain("skills", None, listing(state, &realm)));
        return;
    };
    let id = op["id"].as_i64().unwrap_or(0);
    match name {
        "skill_list" => {
            let _ = out.send(plain("skills", None, listing(state, &realm)));
        }
        "skill_propose" => {
            // A proposal takes a model run: it is made in the background and announced when it is ready.
            let (Some(ctx), Some(handle)) = (realm.propose.clone(), op["sid"].as_str().and_then(|sid| state.registry.get(sid))) else {
                return result(out, &rid, false, json!({"error": "open a chat first"}));
            };
            let scope: &'static str = if op["scope"].as_str() == Some("user") { "user" } else { "project" };
            let (hint, target) = (text(op, "hint"), text(op, "target").filter(|t| !t.is_empty()));
            let events = state.registry.events.clone();
            let account = realm.account.clone();
            tokio::spawn(async move {
                if let Err(reason) = ctx.manual(&handle, hint, target, scope).await {
                    let _ = events.send(json!({"v": 1, "kind": "skill_notice", "data": {"text": reason, "account": account}}));
                }
            });
            result(out, &rid, true, json!({"started": true}));
        }
        "skill_approve" => {
            let edit = match op.get("edit") {
                Some(e) if e.is_object() => Edit { name: text(e, "name"), description: text(e, "description"), body: text(e, "body") },
                _ => Edit::default(),
            };
            let report = store.final_report(id, &edit);
            match store.approve(id, edit, op["ack_risk"].as_bool() == Some(true)) {
                Ok(skill) => {
                    changed(state, &realm);
                    result(out, &rid, true, json!({"skill": skill}));
                }
                // The findings of the text that WOULD have been written, so the person sees what the refusal is about.
                Err(reason) => result(out, &rid, false, json!({"error": reason, "id": id, "risks": report})),
            }
        }
        "skill_reject" => {
            let done = store.reject(id);
            if done {
                changed(state, &realm);
            }
            result(out, &rid, done, json!({}));
        }
        "skill_enable" => {
            let done = store.set_enabled(id, op["on"].as_bool() == Some(true));
            if done {
                changed(state, &realm);
            }
            result(out, &rid, done, json!({}));
        }
        "skill_delete" => {
            let done = store.delete(id);
            if done {
                changed(state, &realm);
            }
            result(out, &rid, done, json!({"undo": id}));
        }
        "skill_undo" => {
            let done = store.undo_delete(id);
            if done {
                changed(state, &realm);
            }
            result(out, &rid, done, json!({}));
        }
        "skill_versions" => {
            let _ = out.send(plain("skill_versions", None, json!({"id": id, "versions": store.versions(id)})));
        }
        "skill_rollback" => match store.rollback(id, op["n"].as_i64().unwrap_or(0)) {
            Ok(skill) => {
                changed(state, &realm);
                result(out, &rid, true, json!({"skill": skill}));
            }
            Err(reason) => result(out, &rid, false, json!({"error": reason})),
        },
        _ => result(out, &rid, false, json!({"error": "unknown skills operation"})),
    }
}
