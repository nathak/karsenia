//! A worktree chat's header chip and card: the merge check, the merge back into the original folder, the clean-up, and accepting
//! changed git settings. Every operation names the chat; the host decides what is allowed (it checks the worktree is one this app
//! made, and again when it acts). What it says on an error is shown as it is.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use serde_json::{json, Value};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tenrec::view::i18n::{fill, tr};
use tenrec::view::wt::{merge_prompt, remove_prompts, status_view};

use crate::{App, AppWindow, Wt, WtLine, WtRow};

#[derive(Clone, Copy, PartialEq)]
enum Purpose {
    Check,
    Remove,
}

struct State {
    /// The number of the last operation sent: an answer to an older one is dropped.
    req: u64,
    purpose: Purpose,
    facts: Option<Value>,
    /// The hash of the git settings to accept, from the host's last refusal.
    hash: String,
}

thread_local! {
    static ST: RefCell<State> = const { RefCell::new(State { req: 0, purpose: Purpose::Check, facts: None, hash: String::new() }) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

fn set_busy(ui: &AppWindow, busy: bool) {
    BUSY.with(|b| b.set(busy));
    ui.global::<Wt>().set_busy(busy);
}

/// The chat on screen changed: nothing of the old one stays.
pub fn reset(ui: &AppWindow) {
    ST.with(|s| *s.borrow_mut() = State { req: 0, purpose: Purpose::Check, facts: None, hash: String::new() });
    let w = ui.global::<Wt>();
    w.set_show(false);
    w.set_open(false);
    w.set_checked(false);
    w.set_cfg(false);
    w.set_lines(ModelRc::default());
    w.set_summary(SharedString::new());
    w.set_can_merge(false);
    set_busy(ui, false);
}

/// What the chat on screen has of a worktree: (branch, base branch, original folder, cleaned up).
pub type Info = Option<(String, String, String, bool)>;

pub fn set_chat(ui: &AppWindow, app: &App, info: Info) {
    let w = ui.global::<Wt>();
    match info {
        Some((branch, base, repo, false)) => {
            w.set_show(true);
            let banner = fill(&tr(app.lang, "wt_banner"), &[("branch", branch), ("base", base), ("repo", repo)]);
            if w.get_banner() != banner.as_str() {
                w.set_banner(banner.into());
            }
        }
        _ => {
            w.set_show(false);
            w.set_open(false);
        }
    }
}

/// The project's working folders for the panel; the list is only replaced when it changed.
pub fn set_list(ui: &AppWindow, app: &App, rows: &[tenrec::view::git::WtEntry]) {
    let list: Vec<WtRow> = rows
        .iter()
        .map(|r| {
            let tag = if r.missing {
                tr(app.lang, "wt_tag_gone")
            } else if r.current {
                tr(app.lang, "wt_tag_here")
            } else if r.main {
                tr(app.lang, "wt_tag_main")
            } else {
                String::new()
            };
            WtRow { path: r.path.display().to_string().into(), branch: r.branch.clone().into(), tag: tag.into(), missing: r.missing, current: r.current }
        })
        .collect();
    let w = ui.global::<Wt>();
    if !w.get_list().iter().eq(list.iter().cloned()) {
        w.set_list(ModelRc::from(Rc::new(VecModel::from(list))));
    }
}

/// The chat on screen, to check again when a question is answered: it may have changed meanwhile.
fn chat_now(app: &App) -> Option<String> {
    app.current.lock().expect("current").sid.clone()
}

fn send(ui: &AppWindow, app: &App, purpose: Purpose, mut op: Value) {
    let Some(sid) = chat_now(app) else { return };
    op["sid"] = json!(sid);
    ST.with(|s| {
        let mut s = s.borrow_mut();
        s.purpose = purpose;
        s.req = app.bus.send(op);
    });
    set_busy(ui, true);
}

fn check(ui: &AppWindow, app: &App, purpose: Purpose) {
    send(ui, app, purpose, json!({"op": "wt_status"}));
}

pub fn on_reply(ui: &AppWindow, app: &Arc<App>, kind: &str, data: Value) {
    let lang = app.lang;
    // A clean-up changes the chat list whichever chat is on screen.
    if kind == "wt_removed" {
        let a = app.clone();
        tokio::spawn(async move { crate::refresh_sessions(&a).await });
    }
    if data["req"].as_u64() != Some(ST.with(|s| s.borrow().req)) {
        return;
    }
    set_busy(ui, false);
    let w = ui.global::<Wt>();
    match kind {
        "wt_status" => {
            let purpose = ST.with(|s| s.borrow().purpose);
            ST.with(|s| {
                let mut s = s.borrow_mut();
                s.facts = Some(data.clone());
                s.purpose = Purpose::Check;
            });
            w.set_cfg(false);
            let view = status_view(&data, lang);
            w.set_summary(view.summary.into());
            let lines: Vec<WtLine> = view.lines.into_iter().map(|(text, bad)| WtLine { text: text.into(), bad }).collect();
            w.set_lines(ModelRc::from(Rc::new(VecModel::from(lines))));
            w.set_can_merge(view.can_merge);
            w.set_checked(true);
            if purpose == Purpose::Remove {
                ask_remove(ui, app, &data);
            }
        }
        "wt_merged" => {
            crate::toast(ui, tr(lang, "wt_merged"));
            check(ui, app, Purpose::Check);
        }
        "wt_removed" => {
            let mut text = tr(lang, "wt_done");
            if data["branch_kept"] == true {
                text = format!("{text} · {}", fill(&tr(lang, "wt_kept"), &[("branch", data["branch"].as_str().unwrap_or_default().to_owned())]));
            }
            crate::toast(ui, text);
            w.set_show(false);
            w.set_open(false);
        }
        "wt_trusted" => {
            crate::toast(ui, tr(lang, "wt_cfg_done"));
            w.set_cfg(false);
            check(ui, app, Purpose::Check);
        }
        "wt_error" => {
            if data["code"] == "config_changed" {
                let entries: Vec<SharedString> = data["entries"].as_array().map(|e| e.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string()).chars().filter(|c| !c.is_control()).take(300).collect::<String>().into()).collect()).unwrap_or_default();
                ST.with(|s| s.borrow_mut().hash = data["hash"].as_str().unwrap_or_default().to_owned());
                w.set_cfg_entries(ModelRc::from(Rc::new(VecModel::from(entries))));
                w.set_cfg(true);
                return;
            }
            if let Some(message) = data["message"].as_str() {
                crate::toast(ui, message.to_owned());
            }
            // What the host refused is stale or in the way: look again.
            if ["stale", "blocked", "conflict", "uncommitted"].contains(&data["code"].as_str().unwrap_or_default()) {
                check(ui, app, Purpose::Check);
            }
        }
        _ => {}
    }
}

/// The questions before a clean-up: the first always, the second when commits would be left on no other branch.
fn ask_remove(ui: &AppWindow, app: &Arc<App>, facts: &Value) {
    let (force, first, second) = remove_prompts(facts, app.lang);
    let tip = facts["tip"].as_str().unwrap_or_default().to_owned();
    let (a, weak) = (app.clone(), ui.as_weak());
    let asked_in = chat_now(app);
    let go = move |discard: bool| {
        if chat_now(&a) != asked_in {
            return;
        }
        if let Some(ui) = weak.upgrade() {
            send(&ui, &a, Purpose::Check, json!({"op": "wt_remove", "force": force, "discard": discard, "tip": tip}));
        }
    };
    let (b, weak) = (app.clone(), ui.as_weak());
    crate::panels::ask(ui, first, tr(app.lang, "wt_remove"), true, Box::new(move |ok| {
        if !ok {
            return;
        }
        match (second, weak.upgrade()) {
            // Cancel on the second question keeps the branch; OK deletes it too.
            (Some(question), Some(ui)) => crate::panels::ask(&ui, question, tr(b.lang, "wt_remove_branch"), true, Box::new(go)),
            _ => go(false),
        }
    }));
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let w = ui.global::<Wt>();
    let (a, weak) = (app.clone(), ui.as_weak());
    w.on_check(move || {
        if let Some(ui) = weak.upgrade() {
            if !BUSY.with(Cell::get) {
                check(&ui, &a, Purpose::Check);
            }
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    w.on_merge(move || {
        let Some(ui) = weak.upgrade() else { return };
        let Some(facts) = ST.with(|s| s.borrow().facts.clone()) else { return };
        let b = a.clone();
        let asked_in = chat_now(&a);
        let (tip, main_head) = (facts["tip"].as_str().unwrap_or_default().to_owned(), facts["main_head"].as_str().unwrap_or_default().to_owned());
        crate::panels::ask(&ui, merge_prompt(&facts, a.lang), tr(a.lang, "wt_merge"), false, Box::new(move |ok| {
            if let (true, Some(ui)) = (ok && chat_now(&b) == asked_in, b.weak.upgrade()) {
                send(&ui, &b, Purpose::Check, json!({"op": "wt_merge", "tip": tip, "main_head": main_head}));
            }
        }));
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    w.on_remove(move || {
        // Looked at again first: the questions are about the worktree as it is now.
        if let Some(ui) = weak.upgrade() {
            if !BUSY.with(Cell::get) {
                check(&ui, &a, Purpose::Remove);
            }
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    w.on_accept(move || {
        let Some(ui) = weak.upgrade() else { return };
        let hash = ST.with(|s| s.borrow().hash.clone());
        let b = a.clone();
        let asked_in = chat_now(&a);
        crate::panels::ask(&ui, tr(a.lang, "wt_cfg_confirm"), tr(a.lang, "wt_cfg_accept"), false, Box::new(move |ok| {
            if let (true, Some(ui)) = (ok && chat_now(&b) == asked_in, b.weak.upgrade()) {
                send(&ui, &b, Purpose::Check, json!({"op": "wt_trust", "hash": hash}));
            }
        }));
    });
}
