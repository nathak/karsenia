//! The Accounts panel's operations on the WebSocket. Accounts are made, renamed and removed only here, by the person. Logging in is
//! not one of them: the person runs `claude auth login` themselves (the panel shows the command), so no token or code passes through
//! this app.
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::{plain, AppState};

fn result(out: &UnboundedSender<String>, rid: &Value, ok: bool, data: Value) {
    let mut body = data;
    body["ok"] = Value::Bool(ok);
    body["rid"] = rid.clone();
    let _ = out.send(plain("account_result", None, body));
}

/// Where the default account's `claude` runs: the folder it keeps its settings in, else the home folder.
fn default_dir() -> std::path::PathBuf {
    let home = crate::platform::home_dir();
    crate::accounts::default_config_dir(std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from).as_deref(), home.as_deref()).filter(|d| d.is_dir()).or(home).unwrap_or_else(|| "/".into())
}

fn changed(state: &AppState) {
    let _ = state.registry.events.send(json!({"v": 1, "kind": "accounts_changed", "data": {}}));
}

/// A path as one word for a shell: in single quotes, with each quote inside written as `'\''`.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// What the person types in their own terminal to log this account in.
pub fn login_command(config_dir: &std::path::Path) -> String {
    if cfg!(windows) {
        // PowerShell: a single-quoted string, a quote inside written twice.
        return format!("$env:CLAUDE_CONFIG_DIR = '{}'; claude auth login", config_dir.to_string_lossy().replace('\'', "''"));
    }
    format!("CLAUDE_CONFIG_DIR={} claude auth login", shell_quote(&config_dir.to_string_lossy()))
}

fn listing(state: &AppState) -> Value {
    let registry = &state.registry;
    let accounts: Vec<Value> = registry
        .accounts()
        .list()
        .into_iter()
        .map(|a| {
            let open = registry.realm(Some(&a.id)).is_ok();
            json!({"id": a.id, "name": a.name, "created": a.created, "open": open, "login": login_command(&crate::accounts::config_dir(registry.dir(), &a.id)), "dir": crate::accounts::config_dir(registry.dir(), &a.id).display().to_string()})
        })
        .collect();
    json!({"accounts": accounts, "notice": registry.accounts_notice})
}

pub fn handle(state: &AppState, name: &str, op: &Value, out: &UnboundedSender<String>) {
    let rid = op["rid"].clone();
    let text = |key: &str| op[key].as_str().unwrap_or_default().to_owned();
    match name {
        "account_usage" => {
            let _ = out.send(plain("account_usage", None, json!({"accounts": state.registry.usage().snapshot()})));
        }
        "account_list" => {
            let _ = out.send(plain("accounts", None, listing(state)));
        }
        "account_add" => match state.registry.add_account(&text("name")) {
            Ok(account) => {
                changed(state);
                let login = login_command(&crate::accounts::config_dir(state.registry.dir(), &account.id));
                result(out, &rid, true, json!({"account": account, "login": login}));
            }
            Err(error) => result(out, &rid, false, json!({"error": error})),
        },
        "account_rename" => match state.registry.accounts().rename(&text("id"), &text("name"), crate::now_ms()) {
            Ok(()) => {
                changed(state);
                result(out, &rid, true, json!({}));
            }
            Err(error) => result(out, &rid, false, json!({"error": error})),
        },
        "account_delete" => {
            // The notes, skills and history of the account go with it: the person must have said so.
            if op["confirm"].as_bool() != Some(true) {
                return result(out, &rid, false, json!({"error": "removing an account deletes its notes, skills and history: confirm it", "needs_confirm": true}));
            }
            let (state, out, id) = (state.clone(), out.clone(), text("id"));
            // Stopping chats and signing out can take a moment: it runs beside the socket's other messages.
            tokio::spawn(async move {
                match state.registry.delete_account(&id).await {
                    Ok(warnings) => {
                        changed(&state);
                        let _ = state.registry.events.send(json!({"v": 1, "kind": "memory_changed", "data": {"account": id, "gone": true}}));
                        result(&out, &rid, true, json!({"warnings": warnings}));
                    }
                    Err(error) => result(&out, &rid, false, json!({"error": error})),
                }
            });
        }
        // What the default account has that this account lacks (the offer), and what this account has.
        "account_plugins" => {
            let req = op["req"].clone();
            let (state, out, id) = (state.clone(), out.clone(), text("id"));
            tokio::spawn(async move {
                let bin = state.registry.bin().to_owned();
                let answer = async {
                    let (dir, env) = state.registry.account_login(&id)?;
                    let default_dir = default_dir();
                    let mine = crate::plugins::list(&bin, &dir, &env).await?;
                    let theirs = crate::plugins::list(&bin, &default_dir, &Default::default()).await?;
                    let offer: Vec<Value> = theirs.iter().filter(|p| !mine.iter().any(|m| m.id == p.id)).map(|p| json!({"id": p.id, "version": p.version})).collect();
                    Ok::<_, String>(json!({"offer": offer, "installed": mine.iter().map(|p| p.id.clone()).collect::<Vec<_>>()}))
                }
                .await;
                let mut data = match answer {
                    Ok(data) => data,
                    Err(error) => json!({"offer": [], "installed": [], "error": error}),
                };
                data["req"] = req;
                data["account"] = Value::String(id);
                let _ = out.send(plain("account_plugins", None, data));
            });
        }
        // A person asked for one plugin of the default account to be installed into this account.
        "account_plugin_install" => {
            let (state, out, id, plugin) = (state.clone(), out.clone(), text("id"), text("plugin"));
            tokio::spawn(async move {
                let bin = state.registry.bin().to_owned();
                let done = match state.registry.account_login(&id) {
                    Ok((dir, env)) => crate::plugins::install(&bin, &default_dir(), &dir, &env, &plugin).await,
                    Err(error) => Err(error),
                };
                match done {
                    Ok(()) => result(&out, &rid, true, json!({"plugin": plugin, "account": id})),
                    Err(error) => result(&out, &rid, false, json!({"error": error, "plugin": plugin, "account": id})),
                }
            });
        }
        "account_status" => {
            let req = op["req"].clone();
            let (state, out) = (state.clone(), out.clone());
            let id = op["id"].as_str().map(str::to_owned);
            tokio::spawn(async move {
                let bin = state.registry.bin().to_owned();
                let answer = match &id {
                    Some(id) => match state.registry.account_login(id) {
                        Ok((dir, env)) => crate::accounts::status(&bin, &dir, &env, Some(&dir)).await.map_err(|e| e.message()),
                        Err(error) => Err(error),
                    },
                    // The default account: the program as it already runs, in the folder it keeps its settings in.
                    None => {
                        let home = crate::platform::home_dir();
                        let dir = crate::accounts::default_config_dir(std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from).as_deref(), home.as_deref()).filter(|d| d.is_dir()).or(home).unwrap_or_else(|| "/".into());
                        crate::accounts::status(&bin, &dir, &Default::default(), None).await.map_err(|e| e.message())
                    }
                };
                let data = match answer {
                    Ok(status) => json!({"req": req, "id": id, "status": status}),
                    Err(error) => json!({"req": req, "id": id, "error": error}),
                };
                let _ = out.send(plain("account_status", None, data));
            });
        }
        _ => result(out, &rid, false, json!({"error": "unknown account operation"})),
    }
}
