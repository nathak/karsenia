//! The Schedule tab's operations on the WebSocket. Tasks are made and changed only here, by the person: there is no MCP tool for it,
//! so a chat (or anything Claude read) cannot schedule a run of its own.
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::{plain, AppState};
use crate::sched_store::{Profile, TaskDef};
use crate::schedule::Schedule;
use crate::scheduler::folder_allowed;

fn changed(state: &AppState) {
    let _ = state.registry.events.send(json!({"v": 1, "kind": "sched_changed", "data": {}}));
}

fn result(out: &UnboundedSender<String>, rid: &Value, ok: bool, data: Value) {
    let mut body = data;
    body["ok"] = Value::Bool(ok);
    body["rid"] = rid.clone();
    let _ = out.send(plain("sched_result", None, body));
}

fn listing(state: &AppState) -> Value {
    let Some((store, scheduler)) = state.registry.sched() else {
        return json!({"available": false, "notice": state.registry.sched_notice});
    };
    let runs: Vec<Value> = store.runs(None, 100).iter().map(|r| r.to_json(false)).collect();
    let tasks: Vec<Value> = store.tasks().iter().map(|t| t.to_json()).collect();
    json!({"available": true, "on": state.registry.settings().get().sched_on, "tasks": tasks, "runs": runs, "unseen": store.unseen(), "busy": scheduler.busy(), "running": store.running_tasks(), "notice": state.registry.sched_notice})
}

fn text(op: &Value, key: &str) -> String {
    op[key].as_str().unwrap_or_default().to_owned()
}

/// A task as the form sent it. The folder is made canonical and checked here; the rest is checked by `TaskDef::check`.
fn parse_def(state: &AppState, op: &Value) -> Result<TaskDef, String> {
    let profile = Profile::parse(&text(op, "profile")).ok_or("choose what the run may use: files or the web")?;
    let schedule = match op["kind"].as_str() {
        Some("every") => Schedule::Every { minutes: op["every_min"].as_u64().unwrap_or(0).min(u64::from(u32::MAX)) as u32 },
        Some("daily") => Schedule::Daily { minute_of_day: op["minute_of_day"].as_u64().unwrap_or(u64::MAX).min(9999) as u16, weekdays: op["weekdays"].as_u64().unwrap_or(0).min(255) as u8 },
        _ => return Err("choose a schedule".into()),
    };
    let cwd = match profile {
        // Files and the web never mix: a web task that names a folder is refused, not quietly stripped of it.
        Profile::Web if !text(op, "cwd").is_empty() => return Err("a web task has no project folder: files and the web are not mixed".into()),
        Profile::Web => String::new(),
        Profile::Folder => {
            let given = text(op, "cwd");
            folder_allowed(std::path::Path::new(&given), state.registry.dir())?.display().to_string()
        }
    };
    let def = TaskDef {
        name: text(op, "name"),
        prompt: text(op, "prompt"),
        cwd,
        profile,
        model: if text(op, "model").is_empty() { "sonnet".into() } else { text(op, "model") },
        schedule,
        max_turns: op["max_turns"].as_u64().unwrap_or(20).min(1000) as u32,
        budget_usd: op["budget_usd"].as_f64().unwrap_or(0.5),
        timeout_s: op["timeout_s"].as_u64().unwrap_or(600).min(100_000) as u32,
    };
    def.check()?;
    Ok(def)
}

pub fn handle(state: &AppState, name: &str, op: &Value, out: &UnboundedSender<String>) {
    let rid = op["rid"].clone();
    let Some((store, scheduler)) = state.registry.sched() else {
        let _ = out.send(plain("sched", None, listing(state)));
        return;
    };
    let id = op["id"].as_i64().unwrap_or(0);
    match name {
        "sched_list" => {
            let _ = out.send(plain("sched", None, listing(state)));
        }
        "sched_save" => match parse_def(state, op) {
            Err(reason) => result(out, &rid, false, json!({"error": reason})),
            Ok(def) => {
                let next = scheduler.first_due(&def);
                let now = crate::now_ms();
                let saved = if id > 0 { store.update_task(id, &def, next, now).map(|()| id) } else { store.create_task(&def, next, now) };
                match saved {
                    Ok(id) => {
                        changed(state);
                        result(out, &rid, true, json!({"id": id}));
                    }
                    Err(reason) => result(out, &rid, false, json!({"error": reason})),
                }
            }
        },
        "sched_enable" => {
            let on = op["on"].as_bool() == Some(true);
            let next = store.task(id).map(|t| scheduler.first_due(&t.def)).unwrap_or(0);
            let done = store.set_enabled(id, on, next);
            if done {
                changed(state);
            }
            result(out, &rid, done, json!({}));
        }
        "sched_delete" => {
            let done = store.delete_task(id);
            if done {
                changed(state);
            }
            result(out, &rid, done, json!({}));
        }
        "sched_run_now" => match scheduler.run_now(id) {
            Ok(()) => result(out, &rid, true, json!({})),
            Err(reason) => result(out, &rid, false, json!({"error": reason})),
        },
        "sched_stop" => {
            let done = scheduler.stop(id);
            changed(state);
            result(out, &rid, done, json!({"error": if done { "" } else { "nothing of this task is running or waiting" }}));
        }
        "sched_switch" => {
            let on = op["on"].as_bool() == Some(true);
            let was = state.registry.settings().get().sched_on;
            state.registry.settings().update(|s| s.sched_on = on);
            if on && !was {
                // What came due while it was off does not run the moment it is switched on.
                scheduler.reset_times();
            }
            changed(state);
            result(out, &rid, true, json!({}));
        }
        "sched_run" => match store.run(id) {
            Some(run) => {
                store.mark_seen(Some(id));
                changed(state);
                let _ = out.send(plain("sched_run_detail", None, json!({"run": run.to_json(true)})));
            }
            None => result(out, &rid, false, json!({"error": "no such run"})),
        },
        "sched_seen" => {
            store.mark_seen(None);
            changed(state);
            result(out, &rid, true, json!({}));
        }
        _ => result(out, &rid, false, json!({"error": "unknown schedule operation"})),
    }
}
