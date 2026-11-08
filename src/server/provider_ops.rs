//! The Providers panel's operations on the WebSocket. Providers are made and changed only here, by the person: there is no MCP
//! tool for it. A key goes in and is never sent back: the page gets `has_token` and the host.
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::{plain, AppState};
use crate::providers::{Auth, Form};

fn result(out: &UnboundedSender<String>, rid: &Value, ok: bool, data: Value) {
    let mut body = data;
    body["ok"] = Value::Bool(ok);
    body["rid"] = rid.clone();
    let _ = out.send(plain("provider_result", None, body));
}

fn text(op: &Value, key: &str) -> String {
    op[key].as_str().unwrap_or_default().to_owned()
}

fn form(op: &Value) -> Result<Form, String> {
    let auth = if op["auth"].is_null() { Auth::Bearer } else { Auth::parse(&text(op, "auth")).ok_or("choose how the key is sent")? };
    Ok(Form {
        id: op["id"].as_str().filter(|id| !id.is_empty()).map(str::to_owned),
        name: text(op, "name"),
        base_url: text(op, "base_url"),
        auth,
        // A key field left empty keeps the saved key; only an explicit "clear" removes it.
        token: op["token"].as_str().filter(|t| !t.is_empty()).map(str::to_owned),
        clear_token: op["clear_token"].as_bool() == Some(true),
        model: text(op, "model"),
        models: [text(&op["models"], "opus"), text(&op["models"], "sonnet"), text(&op["models"], "haiku")],
    })
}

pub fn handle(state: &AppState, name: &str, op: &Value, out: &UnboundedSender<String>) {
    let rid = op["rid"].clone();
    let store = state.registry.providers();
    match name {
        "provider_list" => {
            let registry = state.registry.clone();
            let out = out.clone();
            tokio::spawn(async move {
                let mut bound = std::collections::HashMap::<String, usize>::new();
                for info in registry.list().await {
                    if let Some(p) = info.meta.provider {
                        *bound.entry(p.id).or_default() += 1;
                    }
                }
                let providers: Vec<Value> = registry
                    .providers()
                    .list_views()
                    .into_iter()
                    .map(|v| {
                        let chats = bound.get(&v.id).copied().unwrap_or(0);
                        let mut value = serde_json::to_value(&v).unwrap_or(Value::Null);
                        value["chats"] = json!(chats);
                        value
                    })
                    .collect();
                let _ = out.send(plain("providers", None, json!({"providers": providers, "notice": registry.providers_notice})));
            });
        }
        "provider_save" => match form(op).and_then(|f| store.save(f, crate::now_ms())) {
            Ok(id) => {
                let _ = state.registry.events.send(json!({"v": 1, "kind": "providers_changed", "data": {}}));
                result(out, &rid, true, json!({"id": id}));
            }
            Err(reason) => result(out, &rid, false, json!({"error": reason})),
        },
        "provider_delete" => {
            let done = store.delete(op["id"].as_str().unwrap_or_default());
            if done {
                let _ = state.registry.events.send(json!({"v": 1, "kind": "providers_changed", "data": {}}));
            }
            result(out, &rid, done, json!({}));
        }
        _ => result(out, &rid, false, json!({"error": "unknown provider operation"})),
    }
}
