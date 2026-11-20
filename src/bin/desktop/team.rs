//! The Team page: the chat's own team mode (helper subagents with a model and effort each, saved in the chat's workbench and passed to
//! `claude` by the app), its workbench contract and the guard's rule file. The SmartWorkbench plug-in is never used in the app.
use std::cell::RefCell;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tenrec::view::i18n::{fill, tr, Lang};
use tenrec::view::guardfile::{self, GuardFile, Rule};
use tenrec::view::team::pick;
use tenrec::workbench::team::Sub;

use crate::{App, AppWindow, GfRule, Team, WbItem};
use tenrec::workbench::{Op, Workbench};

thread_local! {
    // The app, for the snapshot to be read again after a command's answer came.
    // The guard file as the page showed it (None: no file): a change quotes it, and is refused if the file is no longer that.
    static GF_SEEN: RefCell<Option<Option<String>>> = const { RefCell::new(None) };
    // The chat's workbench as the page showed it (a toggle needs the item's current state).
    static WB: RefCell<Option<Workbench>> = const { RefCell::new(None) };
    static APP: RefCell<Option<Arc<App>>> = const { RefCell::new(None) };
}

/// The chat on screen changed state: the team card's buttons are usable when no turn runs and no permission waits.
pub fn apply(ui: &AppWindow, can_send: bool) {
    ui.global::<Team>().set_can_send(can_send);
}

/// The chat changed or was closed: the card is closed and what it showed of the old chat is dropped.
pub fn reset(ui: &AppWindow) {
    let t = ui.global::<Team>();
    t.set_open(false);
    t.set_wb_ready(false);
    WB.with(|w| *w.borrow_mut() = None);
}

/// The chat's own workbench is read and shown (nothing of its text is logged).
/// The Team page's data for the chat on screen: its workbench (with the team settings) and the project's guard rules.
pub fn open_page(ui: &AppWindow, app: &Arc<App>) {
    gf_open(ui, app);
    wb_open(ui, app);
}

pub fn wb_open(ui: &AppWindow, app: &Arc<App>) {
    let t = ui.global::<Team>();
    let sid = app.current.lock().expect("current").sid.clone();
    match sid.as_deref().map(|sid| app.registry.workbench_get(sid)) {
        Some(Ok(wb)) => wb_show(ui, app.lang, &wb),
        Some(Err(key)) => {
            WB.with(|w| *w.borrow_mut() = None);
            t.set_wb_ready(false);
            crate::toast(ui, tr(app.lang, key));
        }
        None => {
            WB.with(|w| *w.borrow_mut() = None);
            t.set_wb_ready(false);
        }
    }
}

fn wb_show(ui: &AppWindow, lang: Lang, wb: &Workbench) {
    WB.with(|w| *w.borrow_mut() = Some(wb.clone()));
    let t = ui.global::<Team>();
    t.set_wb_ready(true);
    t.set_wb_goal(wb.task.goal.clone().into());
    t.set_wb_locked(wb.task.locked);
    t.set_wb_profile(match wb.profile {
        tenrec::workbench::Profile::Off => "off",
        tenrec::workbench::Profile::Permissive => "permissive",
        tenrec::workbench::Profile::Balanced => "balanced",
        tenrec::workbench::Profile::Strict => "strict",
    }
    .into());
    t.set_wb_paused(wb.paused);
    t.set_wb_stop_gate(wb.stop_gate);
    t.set_nt_on(wb.team.on);
    let who = |set: &tenrec::workbench::team::RoleSet| set.model.clone().unwrap_or_else(|| "lead".to_owned());
    t.set_nt_line(format!("explorer {} · advisor {}", who(&wb.team.explorer), who(&wb.team.advisor)).into());
    t.set_nt_lead_effort(pick::effort_index(wb.team.lead_effort));
    t.set_nt_exp_model(pick::model_index(wb.team.explorer.model.as_deref()));
    t.set_nt_exp_effort(pick::effort_index(wb.team.explorer.effort));
    t.set_nt_adv_model(pick::model_index(wb.team.advisor.model.as_deref()));
    t.set_nt_adv_effort(pick::effort_index(wb.team.advisor.effort));
    t.set_wb_lint(lint_text(wb, lang).into());
    let model = |items: Vec<WbItem>| ModelRc::from(std::rc::Rc::new(VecModel::from(items)));
    t.set_wb_c(model(wb.task.constraints.iter().map(|c| WbItem { id: c.id as i32, text: c.text.clone().into(), tag: if c.hard { "H" } else { "S" }.into(), on: c.active }).collect()));
    t.set_wb_n(model(wb.task.non_goals.iter().enumerate().map(|(i, n)| WbItem { id: i as i32, text: n.clone().into(), tag: SharedString::new(), on: true }).collect()));
    t.set_wb_d(model(wb.task.done.iter().map(|d| WbItem { id: d.id as i32, text: d.text.clone().into(), tag: d.link.clone().unwrap_or_default().into(), on: d.met }).collect()));
    t.set_wb_p(model(
        wb.pins
            .iter()
            .map(|p| match &p.kind {
                tenrec::workbench::PinKind::Note { text } => WbItem { id: p.id as i32, text: text.clone().into(), tag: "note".into(), on: true },
                tenrec::workbench::PinKind::File { path, from, to } => {
                    let range = match (from, to) {
                        (Some(a), Some(b)) => format!(":{a}-{b}"),
                        (Some(a), None) => format!(":{a}-"),
                        _ => String::new(),
                    };
                    WbItem { id: p.id as i32, text: format!("{path}{range}").into(), tag: "file".into(), on: true }
                }
            })
            .collect(),
    ));
}

/// One line for each way the done conditions could be written better (see `workbench::lint_done`); empty when there is nothing to say.
fn lint_text(wb: &Workbench, lang: Lang) -> String {
    tenrec::workbench::lint_done(&wb.task)
        .iter()
        .map(|lint| {
            let message = tr(lang, lint.key());
            match lint.about().and_then(|id| wb.task.done.iter().find(|d| d.id == id)) {
                Some(done) => format!("• {}: {message}", done.text.chars().take(28).collect::<String>()),
                None => format!("• {message}"),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Applies one change to the open chat's workbench; what is refused is said.
fn wb_do(app: &Arc<App>, op: Op) {
    let Some(sid) = app.current.lock().expect("current").sid.clone() else { return };
    let Some(ui) = app.weak.upgrade() else { return };
    match app.registry.workbench_apply(&sid, op) {
        Ok(wb) => wb_show(&ui, app.lang, &wb),
        Err(key) => crate::toast(&ui, tr(app.lang, key)),
    }
}

/// A line range written as `10-40` (or `10`, or `10-`): Ok(None) for blank, Err for something that is not a range.
fn parse_range(text: &str) -> Result<Option<(u32, Option<u32>)>, ()> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let (a, b) = t.split_once('-').map_or((t, None), |(a, b)| (a, Some(b)));
    let from = a.trim().parse::<u32>().map_err(|_| ())?;
    let to = match b.map(str::trim) {
        None | Some("") => None,
        Some(b) => Some(b.parse::<u32>().map_err(|_| ())?),
    };
    Ok(Some((from, to)))
}

/// The project's guard rules are read (off the window's thread) and shown. Nothing of the file is logged.
pub fn gf_open(ui: &AppWindow, app: &Arc<App>) {
    let Some(cwd) = app.cwd.lock().expect("cwd").clone().filter(|c| !c.is_empty()) else {
        gf_clear(ui);
        return;
    };
    let (a, weak) = (app.clone(), ui.as_weak());
    tokio::spawn(async move {
        let root = std::path::PathBuf::from(&cwd);
        let found = tokio::task::spawn_blocking(move || guardfile::read(&root).map(|g| (g, guardfile::path_of(std::path::Path::new(&cwd))))).await;
        let _ = weak.upgrade_in_event_loop(move |ui| match found {
            Ok(Ok((g, path))) => gf_show(&ui, a.lang, &g, &path),
            Ok(Err(key)) => {
                GF_SEEN.with(|s| *s.borrow_mut() = None);
                let t = ui.global::<Team>();
                t.set_gf_rules(ModelRc::default());
                t.set_gf_note(tr(a.lang, key).into());
            }
            Err(_) => {}
        });
    });
}

fn gf_clear(ui: &AppWindow) {
    GF_SEEN.with(|s| *s.borrow_mut() = None);
    let t = ui.global::<Team>();
    t.set_gf_rules(ModelRc::default());
    t.set_gf_note(SharedString::new());
}

fn gf_show(ui: &AppWindow, lang: Lang, g: &GuardFile, path: &std::path::Path) {
    GF_SEEN.with(|s| *s.borrow_mut() = Some(g.text.clone()));
    let t = ui.global::<Team>();
    let rules: Vec<GfRule> = g.rules.iter().map(|r| GfRule { policy: r.policy.clone().into(), tool: r.tool.clone().into(), command: r.command.clone().into(), path: r.path.clone().into(), reason: r.reason.clone().into() }).collect();
    t.set_gf_rules(ModelRc::from(std::rc::Rc::new(VecModel::from(rules))));
    t.set_gf_note(if g.exists { fill(&tr(lang, "gf_file"), &[("path", path.display().to_string())]) } else { tr(lang, "gf_nofile") }.into());
}

/// Appends or removes a rule, from the text the page showed; then the file is read again.
fn gf_change(app: &Arc<App>, change: impl FnOnce(&std::path::Path, Option<&str>) -> Result<(), &'static str> + Send + 'static) {
    let Some(cwd) = app.cwd.lock().expect("cwd").clone().filter(|c| !c.is_empty()) else { return };
    let Some(seen) = GF_SEEN.with(|s| s.borrow().clone()) else {
        if let Some(ui) = app.weak.upgrade() {
            gf_open(&ui, app);
        }
        return;
    };
    let a = app.clone();
    tokio::spawn(async move {
        let done = tokio::task::spawn_blocking(move || change(std::path::Path::new(&cwd), seen.as_deref())).await.unwrap_or(Err("gf_err_write"));
        let _ = a.weak.clone().upgrade_in_event_loop(move |ui| {
            match done {
                Ok(()) => crate::toast(&ui, tr(a.lang, "gf_saved")),
                Err(key) => crate::toast(&ui, tr(a.lang, key)),
            }
            gf_open(&ui, &a);
        });
    });
}

/// Changes the open chat's team settings (see `Registry::team_apply_many`); what is refused is said, and the card shows what was saved.
fn team_do(app: &Arc<App>, ops: Vec<Op>) {
    let Some(sid) = app.current.lock().expect("current").sid.clone() else { return };
    let a = app.clone();
    tokio::spawn(async move {
        match a.registry.team_apply_many(&sid, ops).await {
            Ok(wb) => {
                let (lang, b) = (a.lang, a.clone());
                let _ = a.weak.clone().upgrade_in_event_loop(move |ui| {
                    // The person may have gone to another chat meanwhile: its card must not show this one's settings.
                    if b.current.lock().expect("current").sid.as_deref() == Some(sid.as_str()) {
                        wb_show(&ui, lang, &wb);
                    }
                    crate::toast(&ui, tr(lang, "nt_saved"));
                });
            }
            Err(key) => crate::toast_later(&a.weak, tr(a.lang, key)),
        }
    });
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));
    let team = ui.global::<Team>();
    let a = app.clone();
    team.on_nt_on_set(move |on| team_do(&a, vec![Op::SetTeam { on }]));
    let a = app.clone();
    team.on_nt_save(move |lead, em, ee, am, ae| {
        team_do(
            &a,
            vec![
                Op::SetLeadEffort { effort: pick::effort(lead) },
                Op::SetRole { role: Sub::Explorer, model: pick::model(em), effort: pick::effort(ee) },
                Op::SetRole { role: Sub::Advisor, model: pick::model(am), effort: pick::effort(ae) },
            ],
        );
    });
    let a = app.clone();
    team.on_gf_reload(move || {
        if let Some(ui) = a.weak.upgrade() {
            gf_open(&ui, &a);
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    team.on_gf_add(move |policy, tool, cmd, path, reason| {
        let rule = Rule { policy: policy.to_string(), tool: tool.to_string(), command: cmd.to_string(), path: path.to_string(), reason: reason.to_string() };
        if let Err(key) = guardfile::check(&rule) {
            if let Some(ui) = weak.upgrade() {
                crate::toast(&ui, tr(a.lang, key));
            }
            return;
        }
        let Some(ui) = weak.upgrade() else { return };
        let add = {
            let (b, w) = (a.clone(), weak.clone());
            move || {
                let r = rule.clone();
                gf_change(&b, move |root, seen| guardfile::add(root, seen, &r));
                if let Some(ui) = w.upgrade() {
                    let t = ui.global::<Team>();
                    t.set_gf_command(SharedString::new());
                    t.set_gf_path(SharedString::new());
                    t.set_gf_reason(SharedString::new());
                }
            }
        };
        // A rule that lets calls through without asking is confirmed.
        if policy == "allow" {
            crate::panels::ask(&ui, tr(a.lang, "gf_confirm_allow"), tr(a.lang, "gf_add_btn"), true, Box::new(move |ok| {
                if ok {
                    add();
                }
            }));
        } else {
            add();
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    team.on_gf_remove(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        let b = a.clone();
        crate::panels::ask(&ui, tr(a.lang, "gf_confirm_remove"), tr(a.lang, "acct_delete"), true, Box::new(move |ok| {
            if ok {
                gf_change(&b, move |root, seen| guardfile::remove(root, seen, index.max(0) as usize));
            }
        }));
    });
    // The chat's own workbench.
    let a = app.clone();
    team.on_wb_goal_set(move |text| {
        wb_do(&a, Op::SetGoal { text: text.to_string() });
        if let Some(ui) = a.weak.upgrade() {
            ui.global::<Team>().set_wb_in_goal(SharedString::new());
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    team.on_wb_profile_set(move |word| {
        let Some(profile) = tenrec::workbench::Profile::parse(word.as_str()) else { return };
        // Switching the guard off is asked first.
        if profile == tenrec::workbench::Profile::Off {
            let Some(ui) = weak.upgrade() else { return };
            let b = a.clone();
            crate::panels::ask(&ui, tr(a.lang, "wb_off_confirm"), tr(a.lang, "tm_cmd_guard"), true, Box::new(move |ok| {
                if ok {
                    wb_do(&b, Op::SetProfile { profile });
                }
            }));
        } else {
            wb_do(&a, Op::SetProfile { profile });
        }
    });
    let a = app.clone();
    team.on_wb_paused_set(move |paused| wb_do(&a, Op::SetPaused { paused }));
    let a = app.clone();
    team.on_wb_lock(move |on| wb_do(&a, if on { Op::Lock } else { Op::Unlock }));
    let a = app.clone();
    team.on_wb_stop_gate_set(move |on| wb_do(&a, Op::SetStopGate { on }));
    let (a, weak) = (app.clone(), ui.as_weak());
    team.on_wb_new(move || {
        let Some(ui) = weak.upgrade() else { return };
        let b = a.clone();
        crate::panels::ask(&ui, tr(a.lang, "wb_new_confirm"), tr(a.lang, "wb_new"), true, Box::new(move |ok| {
            if ok {
                wb_do(&b, Op::New);
            }
        }));
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    team.on_wb_add(move |kind, text, extra| {
        let text = text.to_string();
        let op = match kind.as_str() {
            "c" => Op::AddConstraint { text, hard: extra.as_str() != "soft" },
            "n" => Op::AddNonGoal { text },
            "d" => Op::AddDone { text, link: Some(extra.to_string()).filter(|l| !l.is_empty()) },
            "pn" => Op::PinNote { text },
            "pf" => match parse_range(extra.as_str()) {
                Ok(None) => Op::PinFile { path: text, from: None, to: None },
                Ok(Some((from, to))) => Op::PinFile { path: text, from: Some(from), to },
                Err(()) => {
                    if let Some(ui) = weak.upgrade() {
                        crate::toast(&ui, tr(a.lang, "wb_err_range"));
                    }
                    return;
                }
            },
            _ => return,
        };
        wb_do(&a, op);
        if let Some(ui) = weak.upgrade() {
            let t = ui.global::<Team>();
            match kind.as_str() {
                "c" => t.set_wb_in_c(SharedString::new()),
                "n" => t.set_wb_in_n(SharedString::new()),
                "d" => t.set_wb_in_d(SharedString::new()),
                "pn" => t.set_wb_in_pn(SharedString::new()),
                _ => {
                    t.set_wb_in_pf(SharedString::new());
                    t.set_wb_in_range(SharedString::new());
                }
            }
        }
    });
    let a = app.clone();
    team.on_wb_remove(move |kind, id| {
        let op = match kind.as_str() {
            "c" => Op::RemoveConstraint { id: id.max(0) as u32 },
            "n" => Op::RemoveNonGoal { index: id.max(0) as usize },
            "d" => Op::RemoveDone { id: id.max(0) as u32 },
            "p" => Op::Unpin { id: id.max(0) as u32 },
            _ => return,
        };
        wb_do(&a, op);
    });
    let a = app.clone();
    team.on_wb_toggle(move |kind, id| {
        let id = id.max(0) as u32;
        let current = WB.with(|w| w.borrow().clone());
        let Some(wb) = current else { return };
        let op = match kind.as_str() {
            "c" => Op::ToggleConstraint { id },
            "ch" => match wb.task.constraints.iter().find(|c| c.id == id) {
                Some(c) => Op::SetConstraintHard { id, hard: !c.hard },
                None => return,
            },
            "d" => match wb.task.done.iter().find(|d| d.id == id) {
                Some(d) => Op::MarkDone { id, met: !d.met },
                None => return,
            },
            _ => return,
        };
        wb_do(&a, op);
    });
}
