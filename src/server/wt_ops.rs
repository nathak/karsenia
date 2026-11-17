//! `wt_*` messages: check, merge and remove the git worktree a chat works in (see `worktree`). Every one starts by proving that the
//! chat's worktree is one this app made (`worktree::verify`); what the page sends is only the chat's id and what it was shown.
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::{plain, AppState};
use crate::worktree;

pub fn handle(state: &AppState, name: &str, op: &Value, out: &UnboundedSender<String>) {
    let sid = op["sid"].as_str().unwrap_or_default().to_owned();
    let req = op["req"].clone();
    let name = name.to_owned();
    let (registry, out) = (state.registry.clone(), out.clone());
    let tip = op["tip"].as_str().unwrap_or_default().to_owned();
    let main_head = op["main_head"].as_str().unwrap_or_default().to_owned();
    let hash = op["hash"].as_str().unwrap_or_default().to_owned();
    let (force, discard) = (op["force"].as_bool() == Some(true), op["discard"].as_bool() == Some(true));
    // git work runs beside the socket's other messages and never on its own task.
    tokio::spawn(async move {
        let fail = |code: &str, message: String, extra: Value| {
            let mut data = json!({"req": req, "code": code, "message": message});
            if let (Some(map), Some(more)) = (data.as_object_mut(), extra.as_object()) {
                map.extend(more.clone());
            }
            let _ = out.send(plain("wt_error", Some(&sid), data));
        };
        let Some(handle) = registry.get(&sid) else { return fail("no_session", "no such session".to_owned(), Value::Null) };
        let Some(first) = handle.info().await else { return fail("no_session", "no such session".to_owned(), Value::Null) };
        let Some(repo) = first.meta.worktree.as_ref().map(|w| w.repo.clone()) else { return fail("no_worktree", "this chat has no worktree".to_owned(), Value::Null) };
        // Merging and removing of one project run one after another; looking does not wait. What is read is read again after the wait.
        let lock = worktree::repo_lock(&repo);
        let _guard = if name == "wt_status" { None } else { Some(lock.lock().await) };
        let Some(info) = handle.info().await else { return fail("no_session", "no such session".to_owned(), Value::Null) };
        let Some(wt) = info.meta.worktree.clone() else { return fail("no_worktree", "this chat has no worktree".to_owned(), Value::Null) };
        if wt.removed {
            return fail("removed", "this chat's worktree was already removed".to_owned(), Value::Null);
        }
        let root = match registry.worktree_root(&wt.repo, wt.root.as_deref()) {
            Ok(root) => root,
            Err(message) => return fail("root", message, Value::Null),
        };
        // Accepting the project's git settings as they are now is the one operation that does not require them to be unchanged.
        let owned = match worktree::verify(&info.meta, &root, name != "wt_trust").await {
            Ok(owned) => owned,
            Err(code) if code == "not_owned" => {
                // Nothing left on disk or in git's list: nothing to remove, so the chat may just be marked.
                if name == "wt_remove" && worktree::can_forget(&info.meta, &root).await && handle.retire().await.is_ok() {
                    handle.retired(true);
                    let _ = out.send(plain("wt_removed", Some(&sid), json!({"req": req, "branch_kept": false, "branch": wt.branch, "warning": "forgotten"})));
                    return;
                }
                return fail("not_owned", "this chat's worktree is not one this app made; nothing was touched".to_owned(), Value::Null);
            }
            Err(code) if code == "config_changed" => {
                let now = worktree::fingerprint(&wt.repo, &info.meta.cwd).await.ok();
                let extra = json!({"hash": now.as_ref().map(|f| f.hash.clone()), "entries": now.map(|f| f.entries).unwrap_or_default()});
                return fail("config_changed", "the project's git settings changed since this chat was made".to_owned(), extra);
            }
            Err(message) => return fail("git", message, Value::Null),
        };
        match name.as_str() {
            "wt_status" => match worktree::facts(&owned).await {
                Ok(facts) => {
                    let mut data = serde_json::to_value(&facts).unwrap_or(Value::Null);
                    if let Some(map) = data.as_object_mut() {
                        map.insert("req".into(), req.clone());
                        map.insert("branch".into(), json!(owned.branch));
                        map.insert("repo".into(), json!(owned.repo));
                        map.insert("path".into(), json!(owned.path));
                    }
                    let _ = out.send(plain("wt_status", Some(&sid), data));
                }
                Err(message) => fail("git", message, Value::Null),
            },
            "wt_trust" => match worktree::fingerprint(&owned.repo, &owned.path).await {
                // Only what the person was shown (its hash) is accepted: settings changed again since are not.
                Ok(now) if now.hash == hash => {
                    handle.set_config_hash(now.hash);
                    let _ = out.send(plain("wt_trusted", Some(&sid), json!({"req": req})));
                }
                Ok(now) => fail("config_changed", "the settings changed again; look at them again".to_owned(), json!({"hash": now.hash, "entries": now.entries})),
                Err(message) => fail("git", message, Value::Null),
            },
            "wt_merge" => match worktree::merge(&owned, &tip, &main_head).await {
                Ok(commit) => {
                    let _ = out.send(plain("wt_merged", Some(&sid), json!({"req": req, "commit": commit})));
                }
                Err(refused) => fail(refused.code, refused.message, json!({"conflicts": refused.conflicts, "blockers": refused.blockers})),
            },
            "wt_remove" => {
                if let Ok(facts) = worktree::facts(&owned).await {
                    if facts.uncommitted > 0 && !force {
                        return fail("uncommitted", "the worktree has changes that are not committed".to_owned(), Value::Null);
                    }
                    if !tip.is_empty() && facts.tip != tip {
                        return fail("stale", "the branch changed since it was checked; check again".to_owned(), Value::Null);
                    }
                }
                // The process must be gone before its folder is: a dying claude would write into it.
                match tokio::time::timeout(Duration::from_secs(10), handle.retire()).await {
                    Ok(Ok(())) => {}
                    Ok(Err(message)) => {
                        handle.retired(false);
                        return fail(if message == "busy" { "busy" } else { "git" }, if message == "busy" { "a turn is running; stop it first".to_owned() } else { message }, Value::Null);
                    }
                    Err(_) => {
                        handle.retired(false);
                        return fail("busy", "claude did not end in time; try again".to_owned(), Value::Null);
                    }
                }
                match worktree::remove(&owned, force, discard, (!tip.is_empty()).then_some(tip.as_str())).await {
                    Ok(done) => {
                        handle.retired(true);
                        let _ = out.send(plain("wt_removed", Some(&sid), json!({"req": req, "branch_kept": done.branch_kept, "branch": owned.branch, "warning": done.warning})));
                    }
                    Err(message) => {
                        handle.retired(false);
                        let code = if message == "uncommitted" || message == "stale" { message.as_str() } else { "git" };
                        fail(code, message.clone(), Value::Null);
                    }
                }
            }
            _ => fail("unknown", "unknown worktree operation".to_owned(), Value::Null),
        }
    });
}
