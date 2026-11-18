//! The team-mode chip and card. What they show is only what the chat itself showed of the SmartWorkbench plug-in (see
//! `tenrec::view::team`); the buttons send a `/swb team` command to the open chat like a typed message, which the plug-in
//! answers without calling a model.
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tenrec::view::i18n::{fill, tr, Lang};
use tenrec::view::panels::ago;
use tenrec::view::guardfile::{self, GuardFile, Rule};
use tenrec::view::swb::{self, Refusal, Snapshot, Swb, SwbTeam};
use tenrec::view::team::{Seen, TeamSeen};
use tenrec::view::transcript::Head;

use crate::{App, AppWindow, GfRule, SwbDone, SwbEvidence, Team, TeamRole, WbItem};
use tenrec::workbench::{Op, Workbench};

thread_local! {
    // A command was sent when the chat had ended this many `/swb` turns; the buttons wait for that number to change.
    static WAIT: Cell<Option<u32>> = const { Cell::new(None) };
    // The two things the team state can come from, kept apart so that neither overwrites the other: what the chat showed of the
    // plug-in (set by `apply`) and the team part of the plug-in's own snapshot for this chat's session (set when it is read).
    static LAST_SEEN: RefCell<Option<TeamSeen>> = const { RefCell::new(None) };
    static SNAP_TEAM: RefCell<Option<(u64, SwbTeam)>> = const { RefCell::new(None) };
    // Whether the card is shown for a reason other than the snapshot (the chat is wired to the plug-in or showed its state).
    static BASE_SHOW: Cell<bool> = const { Cell::new(false) };
    // `<chat>/<Claude session id>` the snapshot was last read for: a chat that gets its session id later is read again once.
    static LAST_CSID: RefCell<Option<String>> = const { RefCell::new(None) };
    // The app, for the snapshot to be read again after a command's answer came.
    // The guard file as the page showed it (None: no file): a change quotes it, and is refused if the file is no longer that.
    static GF_SEEN: RefCell<Option<Option<String>>> = const { RefCell::new(None) };
    // The chat's workbench as the page showed it (a toggle needs the item's current state).
    static WB: RefCell<Option<Workbench>> = const { RefCell::new(None) };
    static LAST_SWB: RefCell<Option<Swb>> = const { RefCell::new(None) };
    static PANEL_TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
    static APP: RefCell<Option<Arc<App>>> = const { RefCell::new(None) };
}

/// The chip and the card from what the chat has shown. `can_send`: no turn runs and no permission waits.
pub fn apply(ui: &AppWindow, head: &Head, can_send: bool, lang: Lang) {
    // The count only grows within a chat: a snapshot taken just before the click is older and must not end the wait.
    if WAIT.with(|w| w.get().is_some_and(|n| head.team_turns > n)) {
        WAIT.with(|w| w.set(None));
        // The plug-in writes its snapshot a little after a change: it is read again once, a few seconds after the answer.
        slint::Timer::single_shot(Duration::from_secs(3), || {
            if let Some(app) = APP.with(|a| a.borrow().clone()) {
                if let Some(ui) = app.weak.upgrade() {
                    swb_open(&ui, &app);
                }
            }
        });
    }
    let team = ui.global::<Team>();
    // The plug-in's own text summary (from `/swb status`), for when its snapshot file does not exist.
    match &head.swb_status {
        Some((text, at)) => {
            let now = tenrec::now_ms();
            team.set_status_text(text.clone().into());
            team.set_status_age(if *at > 0 && *at <= now { ago(*at, now, lang) } else { String::new() }.into());
        }
        None => {
            team.set_status_text(SharedString::new());
            team.set_status_age(SharedString::new());
        }
    }
    LAST_SEEN.with(|s| *s.borrow_mut() = head.team.clone());
    let base_show = head.swb == Some(true) || (head.swb.is_none() && head.team.is_some());
    BASE_SHOW.with(|b| b.set(base_show));
    team.set_show(base_show || SNAP_TEAM.with(|s| s.borrow().is_some()));
    team.set_can_send(can_send);
    team.set_busy(WAIT.with(|w| w.get().is_some()));
    paint_team(ui, lang);
}

/// The chip, the roles and the source line. The plug-in's own snapshot of this chat's session comes first (with its age, and a
/// note when the chat showed something newer that differs); without one, what the chat showed of the plug-in is used and said to be
/// that; without either the state is not known. A role the plug-in gave no model for says so: no default is filled in.
fn paint_team(ui: &AppWindow, lang: Lang) {
    let team = ui.global::<Team>();
    let now = tenrec::now_ms();
    let snap = SNAP_TEAM.with(|s| s.borrow().clone());
    let seen = LAST_SEEN.with(|s| s.borrow().clone());
    let set_chip = |on: Option<bool>| {
        let (chip, label) = match on {
            None => ("unknown", tr(lang, "team_unknown")),
            Some(true) => ("on", tr(lang, "team_on")),
            Some(false) => ("off", tr(lang, "team_off")),
        };
        team.set_chip(chip.into());
        team.set_chip_label(label.into());
    };
    if let Some((at, t)) = snap {
        set_chip(Some(t.enabled));
        team.set_codex(if t.codex { "on" } else { "off" }.into());
        let onoff = |b: bool| if b { "on" } else { "off" };
        team.set_codex_review(onoff(t.codex_review).into());
        team.set_codex_rescue(onoff(t.codex_rescue).into());
        let roles: Vec<TeamRole> = t
            .roles
            .iter()
            .map(|(role, model, effort)| TeamRole { role: (*role).into(), model: model.clone().unwrap_or_else(|| tr(lang, "team_role_none")).into(), effort: effort.clone().unwrap_or_default().into() })
            .collect();
        team.set_roles(ModelRc::from(std::rc::Rc::new(VecModel::from(roles))));
        let known_age = at > 0 && at <= now;
        team.set_seen(if known_age { fill(&tr(lang, "team_src_snap"), &[("ago", ago(at, now, lang))]) } else { tr(lang, "team_src_snap_noage") }.into());
        team.set_seen_stale(known_age && now - at > swb::STALE_MS);
        // The count of Opus reviews is in the chat's reply only, so it is told as of that reply.
        let reviews = seen.as_ref().and_then(|s| s.line.opus_reviews.map(|n| fill(&tr(lang, "team_reviews_then"), &[("n", n.to_string()), ("ago", ago(s.ts, now, lang))])));
        team.set_reviews(reviews.unwrap_or_default().into());
        let note = seen.as_ref().filter(|s| s.source == Seen::Line && s.ts > at && s.line.on != t.enabled).map(|s| {
            let state = tr(lang, if s.line.on { "team_on" } else { "team_off" });
            fill(&tr(lang, "team_src_conflict"), &[("ago", ago(s.ts, now, lang)), ("state", state)])
        });
        team.set_team_note(note.unwrap_or_default().into());
        return;
    }
    team.set_seen_stale(false);
    team.set_codex("".into());
    team.set_codex_review("".into());
    team.set_codex_rescue("".into());
    team.set_team_note("".into());
    set_chip(seen.as_ref().map(|s| s.line.on));
    let Some(seen) = seen else {
        team.set_roles(ModelRc::default());
        team.set_reviews("".into());
        team.set_seen("".into());
        return;
    };
    let roles: Vec<TeamRole> = seen.line.roles.iter().map(|(r, m, e)| TeamRole { role: r.as_str().into(), model: m.as_str().into(), effort: e.as_str().into() }).collect();
    team.set_roles(ModelRc::from(std::rc::Rc::new(VecModel::from(roles))));
    team.set_reviews(seen.line.opus_reviews.map(|n| fill(&tr(lang, "team_reviews"), &[("n", n.to_string())])).unwrap_or_default().into());
    let key = if seen.source == Seen::Guard { "team_seen_guard" } else { "team_src_chat" };
    team.set_seen(if seen.ts > 0 { fill(&tr(lang, key), &[("ago", ago(seen.ts, now, lang))]) } else { String::new() }.into());
}

/// The chat changed: nothing is waiting, the card is closed.
pub fn reset(ui: &AppWindow, lang: Lang) {
    WAIT.with(|w| w.set(None));
    ui.global::<Team>().set_open(false);
    ui.global::<Team>().set_show(false);
    // The snapshot of the old chat does not stay, and an answer still on its way for it is dropped.
    SWB_GEN.fetch_add(1, Ordering::AcqRel);
    LAST_SEEN.with(|s| *s.borrow_mut() = None);
    SNAP_TEAM.with(|s| *s.borrow_mut() = None);
    LAST_CSID.with(|s| *s.borrow_mut() = None);
    BASE_SHOW.with(|b| b.set(false));
    swb_clear(ui);
    paint_team(ui, lang);
}

/// The newest read of the plug-in's snapshot: an older answer is dropped.
static SWB_GEN: AtomicU64 = AtomicU64::new(0);

fn swb_clear(ui: &AppWindow) {
    LAST_SWB.with(|l| *l.borrow_mut() = None);
    let t = ui.global::<Team>();
    t.set_swb_state(SharedString::new());
    t.set_swb_note(SharedString::new());
    t.set_swb_age(SharedString::new());
    t.set_swb_stale(false);
    t.set_evidence(ModelRc::default());
    t.set_done(ModelRc::default());
    // Nothing of the last chat's guard or counts stays under another chat.
    t.set_guard_profile(SharedString::new());
    t.set_guard_profile_label(SharedString::new());
    t.set_guard_paused(false);
    t.set_guard_rules(SharedString::new());
    t.set_guard_open(SharedString::new());
    t.set_task_status(SharedString::new());
    t.set_pins(SharedString::new());
    t.set_changed(SharedString::new());
    t.set_constraints(ModelRc::default());
    t.set_non_goals(ModelRc::default());
    t.set_task_goal(SharedString::new());
    t.set_task_locked(false);
}

fn swb_note(ui: &AppWindow, state: &str, note: String) {
    swb_clear(ui);
    let t = ui.global::<Team>();
    t.set_swb_state(state.into());
    t.set_swb_note(note.into());
}

/// How old the snapshot is. A time in the future (a clock that was set back) is not an age.
fn swb_age(ui: &AppWindow, lang: Lang, at: u64) {
    let (t, now) = (ui.global::<Team>(), tenrec::now_ms());
    t.set_swb_age(if at > 0 && at <= now { fill(&tr(lang, "tm_swb_updated"), &[("ago", ago(at, now, lang))]) } else { String::new() }.into());
    t.set_swb_stale(at > 0 && at <= now && now - at > swb::STALE_MS);
}

fn swb_show(ui: &AppWindow, lang: Lang, s: &Swb) {
    let t = ui.global::<Team>();
    let now = tenrec::now_ms();
    t.set_swb_state("found".into());
    t.set_swb_note(SharedString::new());
    swb_age(ui, lang, s.at);
    t.set_guard_profile(s.profile.word().into());
    t.set_guard_profile_label(tr(lang, &format!("tm_gp_{}", s.profile.word())).into());
    t.set_guard_paused(s.paused);
    t.set_guard_rules(fill(&tr(lang, "tm_guard_rules"), &[("n", s.rules.to_string())]).into());
    t.set_guard_open(fill(&tr(lang, "tm_guard_open"), &[("asks", s.open_asks.to_string()), ("props", s.open_proposals.to_string())]).into());
    let evidence: Vec<SwbEvidence> = s.latest_evidence().into_iter().map(|e| SwbEvidence { label: tr(lang, &format!("tm_ev_{}", e.kind)).into(), ok: e.ok, when: if e.at > 0 && e.at <= now { ago(e.at, now, lang) } else { String::new() }.into() }).collect();
    t.set_evidence(ModelRc::from(std::rc::Rc::new(VecModel::from(evidence))));
    t.set_pins(s.pins.map(|n| fill(&tr(lang, "swbp_pins_n"), &[("n", n.to_string())])).unwrap_or_else(|| tr(lang, "swbp_na")).into());
    t.set_changed(s.changed.map(|(f, a, r)| fill(&tr(lang, "swbp_changed"), &[("f", f.to_string()), ("a", a.to_string()), ("r", r.to_string())])).unwrap_or_else(|| tr(lang, "swbp_na")).into());
    t.set_task_goal(s.goal.clone().into());
    t.set_task_status(s.status.clone().into());
    t.set_task_locked(s.locked);
    let strings = |v: &[String]| ModelRc::from(std::rc::Rc::new(VecModel::from(v.iter().map(|x| SharedString::from(x.as_str())).collect::<Vec<_>>())));
    t.set_constraints(strings(&s.constraints));
    t.set_non_goals(strings(&s.non_goals));
    let done: Vec<SwbDone> = s
        .done
        .iter()
        .map(|c| SwbDone { text: c.text.clone().into(), link: c.link.clone().into(), status: c.status.word().into(), label: tr(lang, &format!("tm_st_{}", c.status.word())).into(), met: c.status.met() })
        .collect();
    t.set_done(ModelRc::from(std::rc::Rc::new(VecModel::from(done))));
    t.set_done_met(fill(&tr(lang, "tm_dod_met"), &[("met", s.met().to_string()), ("n", s.done.len().to_string())]).into());
}

/// The Team page was opened, the chat changed or Refresh was pressed: the plug-in's snapshot of the chat's Claude session is read
/// again. Only the one file; nothing is logged of what it says.
pub fn swb_open(ui: &AppWindow, app: &Arc<App>) {
    gf_open(ui, app);
    wb_open(ui, app);
    let generation = SWB_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    let Some(sid) = app.current.lock().expect("current").sid.clone() else {
        SNAP_TEAM.with(|s| *s.borrow_mut() = None);
        swb_note(ui, "nochat", tr(app.lang, "tm_swb_no_chat"));
        paint_team(ui, app.lang);
        return;
    };
    let a = app.clone();
    tokio::spawn(async move {
        let Some(info) = async { a.registry.get(&sid)?.info().await }.await else { return };
        let csid = info.meta.claude_session_id.clone();
        let csid_now = csid.clone();
        // Not read on the window's thread: a few bytes from the disk, but not waited for.
        let snapshot = match csid {
            None => None,
            Some(id) => tokio::task::spawn_blocking(move || match swb::state_dir_from_env() {
                Some(dir) => swb::read(&dir, &id),
                None => Snapshot::Missing,
            })
            .await
            .ok(),
        };
        let (b, lang) = (a.clone(), a.lang);
        let _ = a.weak.upgrade_in_event_loop(move |ui| {
            let same = b.current.lock().expect("current").sid.as_deref() == Some(sid.as_str());
            if !same || SWB_GEN.load(Ordering::Acquire) != generation {
                return;
            }
            // Remembered, so that a chat which only gets its session id later (after its first turn) is read again then.
            if let Some(id) = &csid_now {
                LAST_CSID.with(|l| *l.borrow_mut() = Some(format!("{sid}/{id}")));
            }
            // The team part of the snapshot, when it has one: what the chip and the roles show first.
            let team = match &snapshot {
                Some(Snapshot::Found(found)) => found.team.clone().map(|t| (found.at, t)),
                _ => None,
            };
            SNAP_TEAM.with(|s| *s.borrow_mut() = team);
            match snapshot {
                None => swb_note(&ui, "missing", tr(lang, "tm_swb_not_started")),
                Some(Snapshot::Missing) => swb_note(&ui, "missing", tr(lang, "tm_swb_missing")),
                Some(Snapshot::Refused(why)) => {
                    let key = match why {
                        Refusal::BadId => "tm_swb_why_id",
                        Refusal::NotRegular => "tm_swb_why_file",
                        Refusal::TooBig => "tm_swb_why_big",
                        Refusal::NotJson => "tm_swb_why_json",
                        Refusal::Version => "tm_swb_why_version",
                        Refusal::OtherSession => "tm_swb_why_session",
                    };
                    swb_note(&ui, "refused", fill(&tr(lang, "tm_swb_refused"), &[("why", tr(lang, key))]));
                }
                Some(Snapshot::Found(found)) => {
                    // The plug-in rewrites the file every few seconds: when only its time changed the lists are not rebuilt (that would
                    // jump the scroll), only the age line is.
                    let same = LAST_SWB.with(|l| l.borrow().as_ref().is_some_and(|old| Swb { at: found.at, ..old.clone() } == *found));
                    if same {
                        swb_age(&ui, lang, found.at);
                    } else {
                        swb_show(&ui, lang, &found);
                    }
                    LAST_SWB.with(|l| *l.borrow_mut() = Some((*found).clone()));
                }
            }
            // The chip and the roles follow what was just read, and the card is shown when the snapshot has a team.
            ui.global::<Team>().set_show(BASE_SHOW.with(|b| b.get()) || SNAP_TEAM.with(|s| s.borrow().is_some()));
            paint_team(&ui, lang);
        });
    });
}

/// The chat's own workbench is read and shown (nothing of its text is logged).
pub fn wb_open(ui: &AppWindow, app: &Arc<App>) {
    let t = ui.global::<Team>();
    t.set_wb_native(app.registry.settings().get().workbench_native);
    let sid = app.current.lock().expect("current").sid.clone();
    match sid.as_deref().map(|sid| app.registry.workbench_get(sid)) {
        Some(Ok(wb)) => wb_show(ui, &wb),
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

fn wb_show(ui: &AppWindow, wb: &Workbench) {
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

/// Applies one change to the open chat's workbench; what is refused is said.
fn wb_do(app: &Arc<App>, op: Op) {
    let Some(sid) = app.current.lock().expect("current").sid.clone() else { return };
    let Some(ui) = app.weak.upgrade() else { return };
    match app.registry.workbench_apply(&sid, op) {
        Ok(wb) => wb_show(&ui, &wb),
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

/// The chat on screen now has a Claude session id (its first turn gave it one): the snapshot is read once for it.
pub fn session_known(ui: &AppWindow, app: &Arc<App>, sid: &str, csid: &str) {
    let key = format!("{sid}/{csid}");
    if LAST_CSID.with(|l| l.borrow().as_deref() == Some(key.as_str())) {
        return;
    }
    LAST_CSID.with(|l| *l.borrow_mut() = Some(key));
    swb_open(ui, app);
}

/// The lead's model, from the header's model picker while team mode is on: the plug-in's own command for it.
pub fn send_role(app: &Arc<App>, role: &str, model: &str) {
    if let Ok(text) = tenrec::view::team::role_command(role, model, None) {
        command(app, &text);
    }
}

/// A send that failed: the buttons come back.
pub fn cancel_wait() {
    WAIT.with(|w| w.set(None));
}

fn command(app: &Arc<App>, text: &str) {
    let (ok, turns) = {
        let cur = app.current.lock().expect("current");
        (cur.sid.is_some() && cur.transcript.head.state != "running" && cur.transcript.pending.is_empty(), cur.transcript.head.team_turns)
    };
    if !ok || WAIT.with(|w| w.get().is_some()) {
        // Said, not dropped: a click that does nothing and tells nothing looks like a broken button.
        if let Some(ui) = app.weak.upgrade() {
            crate::toast(&ui, tr(app.lang, if ok { "tm_cmd_waiting" } else { "tm_cmd_blocked" }));
        }
        return;
    }
    WAIT.with(|w| w.set(Some(turns)));
    if let Some(ui) = app.weak.upgrade() {
        crate::toast(&ui, fill(&tr(app.lang, "tm_cmd_sent"), &[("c", text.to_owned())]));
        ui.global::<Team>().set_busy(true);
        // A command whose answer never comes does not keep the buttons off for good.
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_secs(30), move || {
            if WAIT.with(|w| w.get()) == Some(turns) {
                WAIT.with(|w| w.set(None));
                if let Some(ui) = weak.upgrade() {
                    ui.global::<Team>().set_busy(false);
                }
            }
        });
    }
    crate::send_text(app, text.to_owned(), false);
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));
    let team = ui.global::<Team>();
    let a = app.clone();
    team.on_turn(move |on| command(&a, if on { "/swb team on" } else { "/swb team off" }));
    let a = app.clone();
    team.on_check(move || command(&a, "/swb team"));
    let a = app.clone();
    let weak_cmd = ui.as_weak();
    team.on_swb_cmd(move |key| {
        let Some(text) = tenrec::view::team::swb_command(&key) else { return };
        // Switching the guard off weakens what protects the person's files: it is asked first.
        if key == "guard_off" {
            let Some(ui) = weak_cmd.upgrade() else { return };
            let b = a.clone();
            crate::panels::ask(&ui, tr(a.lang, "gf_confirm_off"), tr(a.lang, "gf_confirm_off_ok"), true, Box::new(move |ok| {
                if ok {
                    command(&b, text);
                }
            }));
            return;
        }
        command(&a, text);
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
    // The panel at the right is on screen: the snapshot is read now and every ten seconds while it is (the plug-in writes about as
    // often); an old snapshot is read once a minute. Nothing runs while the panel is closed.
    let (weak, a) = (ui.as_weak(), app.clone());
    team.on_panel_visible(move |shown| {
        PANEL_TIMER.with(|t| *t.borrow_mut() = None);
        let Some(ui) = weak.upgrade() else { return };
        if !shown {
            return;
        }
        swb_open(&ui, &a);
        let (weak, a, tick) = (ui.as_weak(), a.clone(), std::cell::Cell::new(0u32));
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, Duration::from_secs(10), move || {
            let Some(ui) = weak.upgrade() else { return };
            tick.set(tick.get() + 1);
            if ui.global::<Team>().get_swb_stale() && tick.get() % 6 != 0 {
                return;
            }
            swb_open(&ui, &a);
        });
        PANEL_TIMER.with(|t| *t.borrow_mut() = Some(timer));
    });
    // The chat's own workbench.
    let a = app.clone();
    team.on_wb_native_set(move |on| {
        a.registry.settings().update(|s| s.workbench_native = on);
        if let Some(ui) = a.weak.upgrade() {
            ui.global::<Team>().set_wb_native(on);
        }
    });
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
    let (weak, a) = (ui.as_weak(), app.clone());
    team.on_set_role(move |role, model, effort| {
        match tenrec::view::team::role_command(&role, &model, Some(&effort)) {
            Ok(text) => command(&a, &text),
            Err(_) => {
                if let Some(ui) = weak.upgrade() {
                    crate::toast(&ui, tr(a.lang, "team_role_bad"));
                }
            }
        }
    });
    let (weak, a) = (ui.as_weak(), app.clone());
    team.on_swb_refresh(move || {
        if let Some(ui) = weak.upgrade() {
            swb_open(&ui, &a);
        }
    });
}
