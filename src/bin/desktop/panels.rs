//! The Memory, Skills and Settings panels, the sidebar search, the question dialog and the toast's button. Operations go out on
//! the in-process link (`bus`); every reply comes back through `read`, which hands it to the UI thread. The rows are made by
//! `tenrec::view::panels`, the same data the web page gets.
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use slint::{ComponentHandle, Model, ModelRc, SharedString, StyledText, VecModel};
use tenrec::view::i18n::tr;
use tenrec::view::panels::{hits_view, keep_matching, memory_view, risk_lines, skills_view, versions_view};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::{open_chat, App, AppWindow, Dialog, HitRow, Know, Mem, MemHead, MemRow, Search, SkHead, Sk, SkVerV, SkillRow};

/// What the dialog's OK (true) or Cancel (false) does.
type Answer = Box<dyn FnOnce(bool)>;
/// What the toast's button does.
type Action = Box<dyn FnOnce()>;

thread_local! {
    // The last listing of each panel, to open or close a section without asking again.
    static MEM_DATA: RefCell<Option<Value>> = const { RefCell::new(None) };
    static SK_DATA: RefCell<Option<Value>> = const { RefCell::new(None) };
    // Sections the person opened or closed (against each section's default).
    static MEM_TOGGLED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static SK_TOGGLED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    // The search box and the "suggested" chip of the memory and skills pages, and what each list held before they cut it:
    // (notes, waiting notes) and (skills, waiting proposals).
    static FILTER: RefCell<String> = const { RefCell::new(String::new()) };
    static ONLY_PENDING: Cell<bool> = const { Cell::new(false) };
    static MEM_COUNTS: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
    static SK_COUNTS: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
    // What the dialog's OK does, and what the toast's button does.
    static ON_ANSWER: RefCell<Option<Answer>> = const { RefCell::new(None) };
    static ON_TOAST: RefCell<Option<Action>> = const { RefCell::new(None) };
    // Each toast has a number: the timer of an older one never hides a newer one.
    static TOAST_GEN: Cell<u64> = const { Cell::new(0) };
    // The newest search asked for; older answers are dropped.
    static SEARCH_REQ: Cell<u64> = const { Cell::new(0) };
    static SEARCH_TYPED: Cell<u64> = const { Cell::new(0) };
}

/// Shows a toast, with a button when `action` is given; it goes after `ms`.
pub fn toast_with(ui: &AppWindow, text: String, action: Option<(String, Box<dyn FnOnce()>)>, ms: u64) {
    let n = TOAST_GEN.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    ui.set_toast_text(text.into());
    match action {
        Some((label, f)) => {
            ui.set_toast_action(label.into());
            ON_TOAST.with(|t| *t.borrow_mut() = Some(f));
        }
        None => {
            ui.set_toast_action(SharedString::new());
            ON_TOAST.with(|t| *t.borrow_mut() = None);
        }
    }
    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_millis(ms), move || {
        if TOAST_GEN.with(Cell::get) != n {
            return;
        }
        if let Some(ui) = weak.upgrade() {
            ui.set_toast_text(SharedString::new());
            ui.set_toast_action(SharedString::new());
            ON_TOAST.with(|t| *t.borrow_mut() = None);
        }
    });
}

/// Asks a question; `then(true)` runs on OK, `then(false)` on Cancel or Esc.
pub fn ask(ui: &AppWindow, body: String, ok: String, danger: bool, then: Box<dyn FnOnce(bool)>) {
    let d = ui.global::<Dialog>();
    d.set_title(SharedString::new());
    d.set_body(body.into());
    d.set_ok(ok.into());
    d.set_danger(danger);
    d.set_visible(true);
    // A question still open is answered "no" first: its callback must run to give back what it holds.
    let older = ON_ANSWER.with(|a| a.borrow_mut().replace(then));
    if let Some(older) = older {
        older(false);
    }
}

fn id_num(id: &str) -> i64 {
    id.parse().unwrap_or(0)
}

/// Replaces a model's rows only when something changed, so an unchanged panel keeps its state.
pub fn set_rows<T: Clone + PartialEq + 'static>(current: ModelRc<T>, rows: Vec<T>, set: impl FnOnce(ModelRc<T>)) {
    if current.row_count() == rows.len() && current.iter().zip(rows.iter()).all(|(a, b)| &a == b) {
        return;
    }
    set(ModelRc::from(Rc::new(VecModel::from(rows))));
}

// ---- memory ----

fn render_memory(ui: &AppWindow, app: &App) {
    let mem = ui.global::<Mem>();
    // A note being edited would lose its text: the new listing waits until the edit is saved or cancelled.
    if !mem.get_editing().is_empty() {
        return;
    }
    let Some(data) = MEM_DATA.with(|d| d.borrow().clone()) else { return };
    let (head, rows) = MEM_TOGGLED.with(|t| memory_view(&data, app.lang, tenrec::now_ms(), &t.borrow()));
    let notes = rows.iter().filter(|r| r.kind == "note" && r.section != "deleted").count() as i32;
    let waiting = rows.iter().filter(|r| r.kind == "note" && r.section == "pending").count() as i32;
    MEM_COUNTS.with(|c| c.set((notes, waiting)));
    let rows = FILTER.with(|f| keep_matching(&rows, &f.borrow(), ONLY_PENDING.with(Cell::get), |r| r.kind.as_str(), |r| format!("{} {}", r.text, r.meta), |r| r.section == "pending"));
    set_counts(ui);
    mem.set_head(MemHead { available: head.available, notice: head.notice.into(), auto: head.auto, has_cwd: head.has_cwd, card: head.card });
    let rows: Vec<MemRow> = rows
        .into_iter()
        .map(|r| MemRow { kind: r.kind.into(), key: r.key.into(), section: r.section.into(), id: r.id.into(), text: r.text.into(), meta: r.meta.into(), pinned: r.pinned, instr: r.instr, open: r.open, count: r.count })
        .collect();
    set_rows(mem.get_rows(), rows, |m| mem.set_rows(m));
}

/// The two counts on the tabs and the number waiting for a decision.
fn set_counts(ui: &AppWindow) {
    let k = ui.global::<Know>();
    let (notes, waiting_notes) = MEM_COUNTS.with(Cell::get);
    let (skills, proposals) = SK_COUNTS.with(Cell::get);
    k.set_memo_count(notes);
    k.set_skill_count(skills);
    k.set_pending_count(waiting_notes + proposals);
}

fn list_memory(app: &App) {
    let cwd = app.cwd.lock().expect("cwd").clone();
    app.bus.send(json!({"op": "memory_list", "cwd": cwd}));
}

fn memory_result(ui: &AppWindow, app: &Arc<App>, data: &Value) {
    let op = data["rid"].as_u64().and_then(|rid| app.bus.take(rid));
    // A refused operation can still carry other fields (a failed delete names its id): nothing but the refusal is shown.
    if data["ok"] != true {
        if data["needs_confirm"] == true {
            if let Some(op) = op {
                let a = app.clone();
                ask(ui, tr(app.lang, "mem_secret_confirm"), tr(app.lang, "mem_save"), false, Box::new(move |ok| {
                    if ok {
                        let mut op = op;
                        op["confirm"] = json!(true);
                        a.bus.ask(op);
                    }
                }));
                return;
            }
        }
        if let Some(error) = data["error"].as_str() {
            crate::toast(ui, error.to_owned());
        }
        return;
    }
    // A note added: the field is emptied only now, so a refused or cancelled note keeps its text.
    if op.as_ref().is_some_and(|op| op["op"] == "memory_add") {
        ui.global::<Mem>().set_note(SharedString::new());
    }
    if let Some(id) = data["undo"].as_i64() {
        let a = app.clone();
        toast_with(ui, tr(app.lang, "mem_removed"), Some((tr(app.lang, "mem_undo"), Box::new(move || a.bus.post(json!({"op": "memory_undo", "id": id}))))), 10_000);
    }
}

// ---- skills ----

fn render_skills(ui: &AppWindow, app: &App) {
    let sk = ui.global::<Sk>();
    sk.set_can_propose(app.current.lock().expect("current").sid.is_some());
    let Some(data) = SK_DATA.with(|d| d.borrow().clone()) else { return };
    let (head, rows) = SK_TOGGLED.with(|t| skills_view(&data, app.lang, tenrec::now_ms(), &t.borrow()));
    let skills = rows.iter().filter(|r| (r.kind == "skill" || r.kind == "proposal") && r.section != "deleted").count() as i32;
    let proposals = rows.iter().filter(|r| r.kind == "proposal").count() as i32;
    SK_COUNTS.with(|c| c.set((skills, proposals)));
    let rows = FILTER.with(|f| keep_matching(&rows, &f.borrow(), ONLY_PENDING.with(Cell::get), |r| r.kind.as_str(), |r| format!("{} {} {}", r.name, r.description, r.meta), |r| r.kind == "proposal"));
    set_counts(ui);
    sk.set_head(SkHead { available: head.available, notice: head.notice.into(), auto: head.auto });
    let rows: Vec<SkillRow> = rows
        .into_iter()
        .map(|r| SkillRow {
            kind: r.kind.into(),
            key: r.key.into(),
            section: r.section.into(),
            id: r.id.into(),
            name: r.name.into(),
            meta: r.meta.into(),
            description: r.description.into(),
            body: r.body.into(),
            risks: r.risks.into(),
            has_block: r.has_block,
            has_warn: r.has_warn,
            enabled: r.enabled,
            improve: r.improve,
            version: r.version,
            open: r.open,
            count: r.count,
        })
        .collect();
    set_rows(sk.get_rows(), rows, |m| sk.set_rows(m));
    // A proposal that is gone (approved, rejected elsewhere) takes its editor with it.
    let editing = sk.get_editing().to_string();
    if !editing.is_empty() && !data["overview"]["proposals"].as_array().is_some_and(|p| p.iter().any(|x| x["id"].as_i64().map(|i| i.to_string()).as_deref() == Some(editing.as_str()))) {
        sk.set_editing(SharedString::new());
        sk.set_edit_risks(SharedString::new());
    }
}

fn skill_result(ui: &AppWindow, app: &Arc<App>, data: &Value) {
    let op = data["rid"].as_u64().and_then(|rid| app.bus.take(rid)).unwrap_or(Value::Null);
    let sk = ui.global::<Sk>();
    if data["ok"] != true {
        if let Some(error) = data["error"].as_str() {
            crate::toast(ui, error.to_owned());
        }
        // An edited text refused for its findings: the editor stays, with what the refusal is about.
        if op.get("edit").is_some() {
            let risks: Vec<Value> = data["risks"].as_array().cloned().unwrap_or_default();
            sk.set_edit_risks(risk_lines(&risks, app.lang).into());
            return;
        }
        // A proposal refused for its risks: show the report as it stands now.
        if data.get("risks").is_some() {
            app.bus.post_quiet(json!({"op": "skill_list"}));
        }
        return;
    }
    if op.get("edit").is_some() {
        sk.set_editing(SharedString::new());
        sk.set_edit_risks(SharedString::new());
    }
    if op["op"] == "skill_rollback" {
        sk.set_history_id(SharedString::new());
        crate::toast(ui, tr(app.lang, "sk_rolled_back"));
    }
    if data["started"] == true {
        crate::toast(ui, tr(app.lang, "sk_started"));
        return;
    }
    if let Some(id) = data["undo"].as_i64() {
        let a = app.clone();
        toast_with(ui, tr(app.lang, "mem_removed"), Some((tr(app.lang, "mem_undo"), Box::new(move || a.bus.post(json!({"op": "skill_undo", "id": id}))))), 10_000);
    }
}

// ---- replies ----

/// Reads every reply of the link and hands the ones the window uses to the UI thread.
pub async fn read(app: Arc<App>, mut rx: UnboundedReceiver<String>) {
    const USED: &[&str] = &[
        "memory", "memory_result", "memory_changed", "memory_auto", "memory_notice", "skills", "skill_result", "skills_changed", "skill_notice", "skill_proposed", "search_results",
        "accounts", "account_result", "account_status", "account_plugins", "accounts_changed", "account_usage", "usage_changed", "usage_refresh",
        "sched", "sched_result", "sched_changed", "sched_run", "sched_run_detail",
        "providers", "provider_result", "providers_changed", "skill_versions",
        "fs_listing", "fs_text", "fs_saved", "fs_conflict", "fs_needs_confirm", "fs_error", "fs_checked", "fs_created", "fs_renamed", "fs_deleted", "fs_restored", "fs_purge_summary", "fs_purged",
        "wt_status", "wt_merged", "wt_removed", "wt_trusted", "wt_error",
        "term_opened", "term_error",
    ];
    while let Some(text) = rx.recv().await {
        let Ok(mut value) = serde_json::from_str::<Value>(&text) else { continue };
        let kind = value["kind"].as_str().unwrap_or_default().to_owned();
        if !USED.contains(&kind.as_str()) {
            continue;
        }
        let data = value["data"].take();
        let a = app.clone();
        let _ = app.weak.upgrade_in_event_loop(move |ui| on_reply(&ui, &a, &kind, data));
    }
}

/// Whether a reply or an event is about the account the panels show (`all`: about every account).
fn ours(app: &App, data: &Value) -> bool {
    data["all"] == true || data["account"] == app.bus.account().map_or(Value::Null, Value::String)
}

/// The panels now show another account: nothing of the old one stays, and what is on screen is asked again.
pub fn account_changed(ui: &AppWindow, app: &App) {
    MEM_DATA.with(|d| *d.borrow_mut() = None);
    SK_DATA.with(|d| *d.borrow_mut() = None);
    // A note's number is only good inside its account's file.
    ui.global::<Mem>().set_editing(SharedString::new());
    ui.global::<Mem>().set_rows(ModelRc::default());
    ui.global::<Sk>().set_rows(ModelRc::default());
    match ui.get_side() {
        1 => list_memory(app),
        2 => app.bus.post_quiet(json!({"op": "skill_list"})),
        _ => {}
    }
    let q = ui.global::<Search>().get_text().trim().to_owned();
    SEARCH_REQ.with(|r| r.set(0));
    if !q.is_empty() {
        ask_search(app, q);
    }
}

fn ask_search(app: &App, q: String) {
    let req = app.bus.send(json!({"op": "search", "q": q, "limit": 30}));
    SEARCH_REQ.with(|r| r.set(req));
}

fn on_reply(ui: &AppWindow, app: &Arc<App>, kind: &str, data: Value) {
    let lang = app.lang;
    let panel = ui.get_side();
    match kind {
        "accounts" | "account_result" | "account_status" | "account_plugins" | "accounts_changed" => crate::accounts::on_reply(ui, app, kind, data),
        "account_usage" | "usage_changed" | "usage_refresh" => crate::usage::on_reply(ui, app, kind, data),
        "sched" | "sched_result" | "sched_changed" | "sched_run" | "sched_run_detail" => crate::sched::on_reply(ui, app, kind, data),
        "providers" | "provider_result" | "providers_changed" => crate::prov::on_reply(ui, app, kind, data),
        "fs_listing" | "fs_text" | "fs_saved" | "fs_conflict" | "fs_needs_confirm" | "fs_error" | "fs_checked" | "fs_created" | "fs_renamed" | "fs_deleted" | "fs_restored" | "fs_purge_summary" | "fs_purged" => crate::files::on_reply(ui, app, kind, data),
        "wt_status" | "wt_merged" | "wt_removed" | "wt_trusted" | "wt_error" => crate::wt::on_reply(ui, app, kind, data),
        "term_opened" | "term_error" => crate::term::on_reply(ui, app, kind, data),
        // A listing of another account (asked before a switch) is not shown.
        "memory" if ours(app, &data) => {
            MEM_DATA.with(|d| *d.borrow_mut() = Some(data));
            render_memory(ui, app);
        }
        "memory_result" => memory_result(ui, app, &data),
        "memory_changed" if data["gone"] == true && ours(app, &data) => crate::accounts::set_account(ui, app, None),
        "memory_changed" if (panel == 1 || panel == 5) && ours(app, &data) => list_memory(app),
        "memory_auto" => crate::toast(ui, tr(lang, "mem_auto_saved")),
        "memory_notice" | "skill_notice" => crate::toast(ui, data["text"].as_str().unwrap_or_default().to_owned()),
        "skills" if ours(app, &data) => {
            SK_DATA.with(|d| *d.borrow_mut() = Some(data));
            render_skills(ui, app);
        }
        "skill_result" => skill_result(ui, app, &data),
        // The versions of the skill whose history is open; the reply names no account, so only the id is checked.
        "skill_versions" => {
            let sk = ui.global::<Sk>();
            if data["id"].as_i64().map(|i| i.to_string()).as_deref() == Some(sk.get_history_id().as_str()) {
                let current = SK_DATA.with(|d| d.borrow().as_ref().and_then(|d| d["overview"]["active"].as_array().and_then(|a| a.iter().find(|s| s["id"] == data["id"]).and_then(|s| s["version"].as_i64())))).unwrap_or(0);
                let rows: Vec<SkVerV> = versions_view(&data, current, lang, tenrec::now_ms()).into_iter().map(|v| SkVerV { n: v.n, meta: v.meta.into(), description: v.description.into(), current: v.current }).collect();
                sk.set_history(ModelRc::from(Rc::new(VecModel::from(rows))));
            }
        }
        "skills_changed" if (panel == 2 || panel == 5) && ours(app, &data) => app.bus.post_quiet(json!({"op": "skill_list"})),
        "skill_proposed" => {
            let weak = ui.as_weak();
            toast_with(ui, tr(lang, "sk_proposed"), Some((tr(lang, "sk_show"), Box::new(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_new_open(false);
                    ui.set_side(2);
                }
            }))), 10_000);
        }
        "search_results" => {
            if data["req"].as_u64() != Some(SEARCH_REQ.with(Cell::get)) {
                return;
            }
            let search = ui.global::<Search>();
            match hits_view(&data, lang, tenrec::now_ms()) {
                Ok(hits) => {
                    search.set_status(if hits.is_empty() { tr(lang, "srch_none").into() } else { SharedString::new() });
                    let rows: Vec<HitRow> = hits
                        .into_iter()
                        .map(|h| HitRow { sid: h.sid.into(), title: h.title.into(), meta: h.meta.into(), rich: StyledText::from_markdown(&h.md).unwrap_or_else(|_| StyledText::from_plain_text(&h.plain)), plain: h.plain.into() })
                        .collect();
                    search.set_hits(ModelRc::from(Rc::new(VecModel::from(rows))));
                }
                Err(_) => {
                    search.set_status(tr(lang, "srch_failed").into());
                    search.set_hits(ModelRc::default());
                }
            }
        }
        _ => {}
    }
}

// ---- wiring ----

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let know = ui.global::<Know>();
    let (a, weak) = (app.clone(), ui.as_weak());
    know.on_filter_edited(move |text| {
        FILTER.with(|f| *f.borrow_mut() = text.to_string());
        if let Some(ui) = weak.upgrade() {
            render_memory(&ui, &a);
            render_skills(&ui, &a);
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    know.on_chip(move |pending| {
        ONLY_PENDING.with(|p| p.set(pending));
        if let Some(ui) = weak.upgrade() {
            ui.global::<Know>().set_only_pending(pending);
            render_memory(&ui, &a);
            render_skills(&ui, &a);
        }
    });
    let weak = ui.as_weak();
    know.on_goto(move |side| {
        if let Some(ui) = weak.upgrade() {
            ui.global::<Know>().set_adding(false);
            ui.set_new_open(false);
            ui.set_side(side);
        }
    });
    let lang = app.lang;
    ui.global::<crate::About>().set_version(tenrec::view::i18n::fill(&tr(lang, "about_version"), &[("v", format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("TENREC_GIT")))]).into());

    let a = app.clone();
    ui.on_panel_open(move |side| {
        // Leaving Settings: a key typed into the provider form does not stay in the window.
        if side != 3 && side != 5 {
            if let Some(ui) = a.weak.upgrade() {
                crate::prov::leave(&ui);
            }
        }
        match side {
        1 => {
            // A panel made again has no field being edited.
            if let Some(ui) = a.weak.upgrade() {
                ui.global::<Mem>().set_editing(SharedString::new());
            }
            // Both counts are on the tabs of either page.
            a.bus.post_quiet(json!({"op": "skill_list"}));
            list_memory(&a)
        }
        2 => {
            if let Some(ui) = a.weak.upgrade() {
                ui.global::<Sk>().set_can_propose(a.current.lock().expect("current").sid.is_some());
            }
            list_memory(&a);
            a.bus.send(json!({"op": "skill_list"}));
        }
        // The schedule and the settings are one page: either way it needs the accounts, providers, tasks, notes and skills.
        3 | 5 => {
            if let Some(ui) = a.weak.upgrade() {
                crate::accounts::panel_open(&ui, &a);
                crate::sched::ensure_form(&ui, false);
            }
            a.bus.post_quiet(json!({"op": "provider_list"}));
            a.bus.post_quiet(json!({"op": "sched_list"}));
            a.bus.post_quiet(json!({"op": "memory_list", "cwd": a.cwd.lock().expect("cwd").clone()}));
            a.bus.post_quiet(json!({"op": "skill_list"}));
        }
        // The Files page: the open chat's folder is listed.
        4 => {
            if let Some(ui) = a.weak.upgrade() {
                crate::files::panel_open(&ui, &a);
            }
        }
        // The Team page: the chat's own workbench (and with it the team settings) is read.
        6 => {
            if let Some(ui) = a.weak.upgrade() {
                crate::team::open_page(&ui, &a);
            }
        }
        // The VHDB page: what the plug-in left in the chat's project is read.
        7 => {
            if let Some(ui) = a.weak.upgrade() {
                crate::vhdb::open(&ui, &a);
            }
        }
        // The new-chat page with the accounts beside it opened: each login is looked at once.
        100 => {
            if let Some(ui) = a.weak.upgrade() {
                crate::accounts::panel_open(&ui, &a);
            }
        }
        _ => {}
        }
    });
    // The toast goes first, then the action runs: a toast the action shows is not hidden by this click.
    let weak = ui.as_weak();
    ui.on_toast_act(move || {
        let f = ON_TOAST.with(|t| t.borrow_mut().take());
        TOAST_GEN.with(|g| g.set(g.get() + 1));
        if let Some(ui) = weak.upgrade() {
            ui.set_toast_text(SharedString::new());
            ui.set_toast_action(SharedString::new());
        }
        if let Some(f) = f {
            f();
        }
    });
    let weak = ui.as_weak();
    ui.global::<Dialog>().on_answer(move |ok| {
        let Some(ui) = weak.upgrade() else { return };
        ui.global::<Dialog>().set_visible(false);
        ui.invoke_refocus();
        if let Some(f) = ON_ANSWER.with(|a| a.borrow_mut().take()) {
            f(ok);
        }
    });

    // memory
    let mem = ui.global::<Mem>();
    let (a, weak) = (app.clone(), ui.as_weak());
    mem.on_toggle(move |key| {
        MEM_TOGGLED.with(|t| {
            let mut t = t.borrow_mut();
            if !t.remove(key.as_str()) {
                t.insert(key.to_string());
            }
        });
        if let Some(ui) = weak.upgrade() {
            render_memory(&ui, &a);
        }
    });
    let a = app.clone();
    mem.on_add(move |text, scope| {
        let cwd = a.cwd.lock().expect("cwd").clone();
        a.bus.ask(json!({"op": "memory_add", "text": text.as_str(), "scope": scope.as_str(), "cwd": cwd}));
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    mem.on_approve(move |id, instr| {
        let id = id_num(&id);
        if !instr {
            a.bus.ask(json!({"op": "memory_approve", "id": id}));
            return;
        }
        let Some(ui) = weak.upgrade() else { return };
        let b = a.clone();
        ask(&ui, tr(a.lang, "mem_instr_confirm"), tr(a.lang, "mem_approve"), false, Box::new(move |ok| {
            if ok {
                b.bus.ask(json!({"op": "memory_approve", "id": id, "instruction": true}));
            }
        }));
    });
    let a = app.clone();
    mem.on_reject(move |id| a.bus.post(json!({"op": "memory_reject", "id": id_num(&id)})));
    let a = app.clone();
    mem.on_pin(move |id, on| a.bus.post(json!({"op": "memory_pin", "id": id_num(&id), "pinned": on})));
    let (a, weak) = (app.clone(), ui.as_weak());
    mem.on_save(move |id, text| {
        a.bus.ask(json!({"op": "memory_edit", "id": id_num(&id), "text": text.as_str()}));
        if let Some(ui) = weak.upgrade() {
            ui.global::<Mem>().set_editing(SharedString::new());
            render_memory(&ui, &a);
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    mem.on_cancel_edit(move || {
        if let Some(ui) = weak.upgrade() {
            ui.global::<Mem>().set_editing(SharedString::new());
            render_memory(&ui, &a);
        }
    });
    let a = app.clone();
    mem.on_delete(move |id| a.bus.post(json!({"op": "memory_delete", "id": id_num(&id)})));
    let a = app.clone();
    mem.on_undo(move |id| a.bus.post(json!({"op": "memory_undo", "id": id_num(&id)})));
    let a = app.clone();
    mem.on_auto(move |i| {
        let auto = match i {
            1 => "project",
            2 => "project_user",
            _ => "off",
        };
        a.bus.ask(json!({"op": "memory_setting", "auto": auto, "card_seen": true}));
    });
    let a = app.clone();
    mem.on_card_later(move || a.bus.post(json!({"op": "memory_setting", "card_seen": true})));

    // skills
    let sk = ui.global::<Sk>();
    let (a, weak) = (app.clone(), ui.as_weak());
    sk.on_toggle(move |key| {
        SK_TOGGLED.with(|t| {
            let mut t = t.borrow_mut();
            if !t.remove(key.as_str()) {
                t.insert(key.to_string());
            }
        });
        if let Some(ui) = weak.upgrade() {
            render_skills(&ui, &a);
        }
    });
    let a = app.clone();
    sk.on_approve(move |id, ack| a.bus.post(json!({"op": "skill_approve", "id": id_num(&id), "ack_risk": ack})));
    let weak = ui.as_weak();
    sk.on_edit_open(move |id| {
        let Some(ui) = weak.upgrade() else { return };
        let one = SK_DATA.with(|d| d.borrow().as_ref().and_then(|d| d["overview"]["proposals"].as_array().and_then(|p| p.iter().find(|x| x["id"].as_i64().map(|i| i.to_string()).as_deref() == Some(id.as_str())).cloned())));
        let Some(one) = one else { return };
        let text = |k: &str| SharedString::from(one[k].as_str().unwrap_or_default());
        let sk = ui.global::<Sk>();
        sk.set_edit_name(text("name"));
        sk.set_edit_desc(text("description"));
        sk.set_edit_body(text("body"));
        sk.set_edit_ack(false);
        sk.set_edit_risks(SharedString::new());
        sk.set_edit_improve(one["target"].as_str().is_some_and(|t| !t.is_empty()));
        sk.set_editing(id);
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    sk.on_edit_save(move || {
        let Some(ui) = weak.upgrade() else { return };
        let sk = ui.global::<Sk>();
        let desc = sk.get_edit_desc().split_whitespace().collect::<Vec<_>>().join(" ");
        // `ask`, not `post`: the reply must find the operation again to tell an edited text's refusal from another's.
        a.bus.ask(json!({"op": "skill_approve", "id": id_num(&sk.get_editing()), "ack_risk": sk.get_edit_ack(),
            "edit": {"name": sk.get_edit_name().trim(), "description": desc, "body": sk.get_edit_body().as_str()}}));
    });
    let weak = ui.as_weak();
    sk.on_edit_cancel(move || {
        if let Some(ui) = weak.upgrade() {
            ui.global::<Sk>().set_editing(SharedString::new());
            ui.global::<Sk>().set_edit_risks(SharedString::new());
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    sk.on_history_open(move |id| {
        let Some(ui) = weak.upgrade() else { return };
        let sk = ui.global::<Sk>();
        sk.set_history(ModelRc::default());
        if sk.get_history_id() == id {
            sk.set_history_id(SharedString::new());
            return;
        }
        sk.set_history_id(id.clone());
        a.bus.post_quiet(json!({"op": "skill_versions", "id": id_num(&id)}));
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    sk.on_rollback(move |id, n| {
        let Some(ui) = weak.upgrade() else { return };
        let b = a.clone();
        let id = id_num(&id);
        ask(&ui, tr(a.lang, "sk_rollback_confirm"), tr(a.lang, "sk_rollback"), false, Box::new(move |ok| {
            if ok {
                b.bus.ask(json!({"op": "skill_rollback", "id": id, "n": n}));
            }
        }));
    });
    let a = app.clone();
    sk.on_reject(move |id| a.bus.post(json!({"op": "skill_reject", "id": id_num(&id)})));
    let a = app.clone();
    sk.on_enable(move |id, on| a.bus.post(json!({"op": "skill_enable", "id": id_num(&id), "on": on})));
    let a = app.clone();
    sk.on_delete(move |id| a.bus.post(json!({"op": "skill_delete", "id": id_num(&id)})));
    let a = app.clone();
    sk.on_undo(move |id| a.bus.post(json!({"op": "skill_undo", "id": id_num(&id)})));
    let a = app.clone();
    sk.on_auto(move |on| a.bus.post(json!({"op": "skill_setting", "auto": on})));
    let a = app.clone();
    sk.on_propose(move |hint, scope| {
        let Some(sid) = a.current.lock().expect("current").sid.clone() else { return };
        // A proposal is made from the chat, so it belongs to the chat's account, whichever the panel shows.
        let account = a.chat_acct.lock().expect("chat account").clone();
        a.bus.ask(json!({"op": "skill_propose", "sid": sid, "scope": scope.as_str(), "hint": hint.as_str(), "target": "", "account": account}));
    });

    // search: asked 250 ms after the last key, so typing a word asks once
    let search = ui.global::<Search>();
    let (a, weak) = (app.clone(), ui.as_weak());
    search.on_changed(move |text| {
        let typed = SEARCH_TYPED.with(|t| {
            t.set(t.get() + 1);
            t.get()
        });
        let q = text.trim().to_owned();
        if q.is_empty() {
            SEARCH_REQ.with(|r| r.set(0));
            if let Some(ui) = weak.upgrade() {
                ui.global::<Search>().set_hits(ModelRc::default());
                ui.global::<Search>().set_status(SharedString::new());
            }
            return;
        }
        let b = a.clone();
        slint::Timer::single_shot(Duration::from_millis(250), move || {
            if SEARCH_TYPED.with(Cell::get) != typed {
                return;
            }
            ask_search(&b, q);
        });
    });
    let a = app.clone();
    let weak = ui.as_weak();
    search.on_open(move |sid| {
        let Some(ui) = weak.upgrade() else { return };
        let (b, sid) = (a.clone(), sid.to_string());
        crate::files::guard_dirty(&ui, &a, move |_| open_chat(&b, sid));
    });
}
