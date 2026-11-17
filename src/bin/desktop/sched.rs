//! The Schedule panel: the list of tasks and recent runs, the task form, a run's result in an overlay, a toast when a run ends,
//! and the count of unread runs on the sidebar. Operations are the web page's `sched_*` ones (same checks, limits and messages);
//! the rows come from `tenrec::view::sched`.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use serde_json::{json, Value};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tenrec::schedule::SystemClock;
use tenrec::view::i18n::tr;
use tenrec::view::sched::{form_from_task, run_detail, sched_op, sched_view, status_label, SchedForm};

use crate::{App, AppWindow, Sched, SchedHeadV, SchedRun, SchedTask};

thread_local! {
    static DATA: RefCell<Option<Value>> = const { RefCell::new(None) };
    // A save was sent and its answer has not come: a second click does nothing meanwhile.
    static SAVING: Cell<bool> = const { Cell::new(false) };
    // The open run's result, for the copy button.
    static RUN_RESULT: RefCell<String> = const { RefCell::new(String::new()) };
}

fn render(ui: &AppWindow, app: &App) {
    let Some(data) = DATA.with(|d| d.borrow().clone()) else { return };
    let now = tenrec::now_ms() as i64;
    let (head, tasks, runs) = sched_view(&data, app.lang, now, &SystemClock);
    let sched = ui.global::<Sched>();
    sched.set_head(SchedHeadV { available: head.available, on: head.on, notice: head.notice.into(), unseen: head.unseen, count: head.count, full: head.full });
    let tasks: Vec<SchedTask> = tasks
        .into_iter()
        .map(|t| SchedTask {
            id: t.id as i32,
            name: t.name.into(),
            profile: t.profile.into(),
            summary: t.summary.into(),
            state: t.state.into(),
            detail: t.detail.into(),
            tools: t.tools.into(),
            next: t.next.into(),
            enabled: t.enabled,
            running: t.running,
            paused_note: t.paused_note.into(),
        })
        .collect();
    crate::panels::set_rows(sched.get_tasks(), tasks, |m| sched.set_tasks(m));
    let runs: Vec<SchedRun> = runs.into_iter().map(|r| SchedRun { id: r.id as i32, status: r.status.into(), tone: r.tone, name: r.name.into(), when: r.when.into(), unseen: r.unseen }).collect();
    crate::panels::set_rows(sched.get_runs(), runs, |m| sched.set_runs(m));
}

fn fill_form(ui: &AppWindow, f: &SchedForm) {
    let sched = ui.global::<Sched>();
    sched.set_f_name(f.name.as_str().into());
    sched.set_f_prompt(f.prompt.as_str().into());
    sched.set_f_web(f.web);
    sched.set_f_cwd(f.cwd.as_str().into());
    sched.set_f_sites(f.sites.as_str().into());
    sched.set_f_daily(f.daily);
    sched.set_f_time(f.time.as_str().into());
    sched.set_f_days(ModelRc::from(Rc::new(VecModel::from(f.days.to_vec()))));
    sched.set_f_every(f.every.as_str().into());
    sched.set_f_model(f.model);
    sched.set_f_turns(f.turns.as_str().into());
    sched.set_f_budget(f.budget.as_str().into());
    sched.set_f_timeout(f.timeout.as_str().into());
}

fn read_form(ui: &AppWindow) -> SchedForm {
    let sched = ui.global::<Sched>();
    let mut days = [false; 7];
    for (i, day) in days.iter_mut().enumerate() {
        *day = sched.get_f_days().row_data(i).unwrap_or(false);
    }
    SchedForm {
        id: match sched.get_editing() {
            id if id > 0 => i64::from(id),
            _ => 0,
        },
        name: sched.get_f_name().to_string(),
        prompt: sched.get_f_prompt().to_string(),
        web: sched.get_f_web(),
        cwd: sched.get_f_cwd().to_string(),
        sites: sched.get_f_sites().to_string(),
        daily: sched.get_f_daily(),
        time: sched.get_f_time().to_string(),
        days,
        every: sched.get_f_every().to_string(),
        model: sched.get_f_model(),
        turns: sched.get_f_turns().to_string(),
        budget: sched.get_f_budget().to_string(),
        timeout: sched.get_f_timeout().to_string(),
    }
}

pub fn on_reply(ui: &AppWindow, app: &Arc<App>, kind: &str, data: Value) {
    let lang = app.lang;
    match kind {
        "sched" => {
            DATA.with(|d| *d.borrow_mut() = Some(data));
            render(ui, app);
        }
        // Always asked for again, whichever panel is open: the sidebar's count of unread runs needs it.
        "sched_changed" => app.bus.post_quiet(json!({"op": "sched_list"})),
        "sched_result" => {
            let op = data["rid"].as_u64().and_then(|rid| app.bus.take(rid)).unwrap_or(Value::Null);
            if data["ok"] != true {
                SAVING.with(|s| s.set(false));
                if let Some(error) = data["error"].as_str().filter(|e| !e.is_empty()) {
                    crate::toast(ui, error.to_owned());
                }
                return;
            }
            match op["op"].as_str() {
                Some("sched_save") => {
                    SAVING.with(|s| s.set(false));
                    // The form is emptied only if it still holds what was sent: text typed or a task opened since stays.
                    let sent = (op["name"].as_str().unwrap_or_default().to_owned(), op["prompt"].as_str().unwrap_or_default().to_owned());
                    let f = ui.global::<Sched>();
                    if (f.get_f_name().trim().to_owned(), f.get_f_prompt().trim().to_owned()) == sent {
                        ensure_form(ui, true);
                    }
                    crate::toast(ui, tr(lang, "sc_saved"));
                }
                Some("sched_run_now") => crate::toast(ui, tr(lang, "sc_started")),
                _ => {}
            }
        }
        "sched_run" => {
            // A run ended: the list shows it, and the task is not "running" any more.
            app.bus.post_quiet(json!({"op": "sched_list"}));
            // A run that was held back by the day's limit has no run to look at.
            if data["status"] == "cap" {
                return;
            }
            let mut text = format!("{}: {}", data["name"].as_str().unwrap_or_default(), status_label(data["status"].as_str().unwrap_or_default(), lang));
            if data["paused"] == true {
                text = format!("{text} ({})", tr(lang, "sc_paused_toast"));
            }
            let action = data["run_id"].as_i64().map(|run_id| {
                let a = app.clone();
                (tr(lang, "sc_view"), Box::new(move || a.bus.post_quiet(json!({"op": "sched_run", "id": run_id}))) as Box<dyn FnOnce()>)
            });
            crate::panels::toast_with(ui, text, action, 10_000);
        }
        "sched_run_detail" => {
            let d = run_detail(&data["run"], lang, &SystemClock);
            let sched = ui.global::<Sched>();
            sched.set_run_title(d.title.into());
            sched.set_run_meta(d.meta.into());
            sched.set_run_note(d.note.into());
            sched.set_run_external(d.external);
            sched.set_run_denied(d.denied.into());
            RUN_RESULT.with(|r| *r.borrow_mut() = d.result.clone());
            sched.set_run_result(d.result.into());
            sched.set_run_def(d.def.into());
            ui.set_new_open(false);
            ui.set_side(3);
            sched.set_run_open(true);
        }
        _ => {}
    }
}

/// The page always carries the form for a new task: it is made again (empty) when asked or when none is open.
pub fn ensure_form(ui: &AppWindow, force: bool) {
    if force || ui.global::<Sched>().get_editing() == 0 {
        fill_form(ui, &SchedForm::new_default());
        ui.global::<Sched>().set_editing(-1);
    }
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let lang = app.lang;
    let sched = ui.global::<Sched>();
    let names: Vec<SharedString> = (0..7).map(|i| tr(lang, &format!("sc_d{i}")).into()).collect();
    sched.set_day_names(ModelRc::from(Rc::new(VecModel::from(names))));
    // Asked at the start: the sidebar shows the count of unread runs before the panel is ever opened.
    app.bus.post_quiet(json!({"op": "sched_list"}));

    let a = app.clone();
    sched.on_switch(move |on| a.bus.post(json!({"op": "sched_switch", "on": on})));
    let weak = ui.as_weak();
    sched.on_new_task(move || {
        let Some(ui) = weak.upgrade() else { return };
        fill_form(&ui, &SchedForm::new_default());
        ui.global::<Sched>().set_editing(-1);
    });
    let weak = ui.as_weak();
    sched.on_edit(move |id| {
        let Some(ui) = weak.upgrade() else { return };
        let task = DATA.with(|d| d.borrow().as_ref().and_then(|d| d["tasks"].as_array().and_then(|t| t.iter().find(|t| t["id"].as_i64() == Some(i64::from(id))).cloned())));
        if let Some(task) = task {
            fill_form(&ui, &form_from_task(&task));
            ui.global::<Sched>().set_editing(id);
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    sched.on_save(move || {
        let Some(ui) = weak.upgrade() else { return };
        // One save at a time: a second click before the answer would make the task twice.
        if SAVING.with(Cell::get) {
            return;
        }
        match sched_op(&read_form(&ui)) {
            Ok(op) => {
                SAVING.with(|s| s.set(true));
                a.bus.post(op)
            }
            Err(key) => crate::toast(&ui, tr(a.lang, key)),
        }
    });
    let weak = ui.as_weak();
    sched.on_cancel(move || {
        if let Some(ui) = weak.upgrade() {
            ensure_form(&ui, true);
        }
    });
    let weak = ui.as_weak();
    sched.on_day(move |i, on| {
        if let Some(ui) = weak.upgrade() {
            ui.global::<Sched>().get_f_days().set_row_data(i.max(0) as usize, on);
        }
    });
    let weak = ui.as_weak();
    sched.on_pick(move || {
        let Some(ui) = weak.upgrade() else { return };
        let start = ui.global::<Sched>().get_f_cwd().to_string();
        crate::pick_folder(&ui, lang, start, |ui, busy| ui.global::<Sched>().set_picking(busy), |ui, path| ui.global::<Sched>().set_f_cwd(path.into()));
    });
    let a = app.clone();
    sched.on_run_now(move |id| a.bus.post(json!({"op": "sched_run_now", "id": id})));
    let a = app.clone();
    sched.on_stop(move |id| a.bus.post(json!({"op": "sched_stop", "id": id})));
    let a = app.clone();
    sched.on_enable(move |id, on| a.bus.post(json!({"op": "sched_enable", "id": id, "on": on})));
    let (a, weak) = (app.clone(), ui.as_weak());
    sched.on_delete(move |id| {
        let Some(ui) = weak.upgrade() else { return };
        let b = a.clone();
        crate::panels::ask(&ui, tr(a.lang, "sc_delete_confirm"), tr(a.lang, "sc_delete"), true, Box::new(move |ok| {
            if ok {
                b.bus.post(json!({"op": "sched_delete", "id": id}));
            }
        }));
    });
    let a = app.clone();
    sched.on_open_run(move |id| a.bus.post_quiet(json!({"op": "sched_run", "id": id})));
    let weak = ui.as_weak();
    sched.on_close_run(move || {
        if let Some(ui) = weak.upgrade() {
            ui.global::<Sched>().set_run_open(false);
            // The overlay took focus with it.
            ui.invoke_refocus();
        }
    });
    let a = app.clone();
    sched.on_seen_all(move || a.bus.post(json!({"op": "sched_seen"})));
    let (a, weak) = (app.clone(), ui.as_weak());
    sched.on_copy_result(move || {
        let text = RUN_RESULT.with(|r| r.borrow().clone());
        if crate::copy_quiet(text) {
            if let Some(ui) = weak.upgrade() {
                crate::toast(&ui, tr(a.lang, "sc_copied"));
            }
        }
    });
}
