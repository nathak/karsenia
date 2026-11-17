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
use tenrec::view::swb::{self, Refusal, Snapshot, Swb, SwbTeam};
use tenrec::view::team::{Seen, TeamSeen};
use tenrec::view::transcript::Head;

use crate::{App, AppWindow, SwbDone, SwbEvidence, Team, TeamRole};

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
    let t = ui.global::<Team>();
    t.set_swb_state(SharedString::new());
    t.set_swb_note(SharedString::new());
    t.set_swb_age(SharedString::new());
    t.set_swb_stale(false);
    t.set_evidence(ModelRc::default());
    t.set_done(ModelRc::default());
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

fn swb_show(ui: &AppWindow, lang: Lang, s: &Swb) {
    let t = ui.global::<Team>();
    let now = tenrec::now_ms();
    t.set_swb_state("found".into());
    t.set_swb_note(SharedString::new());
    // A time in the future (a clock that was set back) is not an age.
    t.set_swb_age(if s.at > 0 && s.at <= now { fill(&tr(lang, "tm_swb_updated"), &[("ago", ago(s.at, now, lang))]) } else { String::new() }.into());
    t.set_swb_stale(s.at > 0 && s.at <= now && now - s.at > swb::STALE_MS);
    t.set_guard_profile(s.profile.word().into());
    t.set_guard_profile_label(tr(lang, &format!("tm_gp_{}", s.profile.word())).into());
    t.set_guard_paused(s.paused);
    t.set_guard_rules(fill(&tr(lang, "tm_guard_rules"), &[("n", s.rules.to_string())]).into());
    t.set_guard_open(fill(&tr(lang, "tm_guard_open"), &[("asks", s.open_asks.to_string()), ("props", s.open_proposals.to_string())]).into());
    let evidence: Vec<SwbEvidence> = s.latest_evidence().into_iter().map(|e| SwbEvidence { label: tr(lang, &format!("tm_ev_{}", e.kind)).into(), ok: e.ok, when: if e.at > 0 && e.at <= now { ago(e.at, now, lang) } else { String::new() }.into() }).collect();
    t.set_evidence(ModelRc::from(std::rc::Rc::new(VecModel::from(evidence))));
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
                Some(Snapshot::Found(found)) => swb_show(&ui, lang, &found),
            }
            // The chip and the roles follow what was just read, and the card is shown when the snapshot has a team.
            ui.global::<Team>().set_show(BASE_SHOW.with(|b| b.get()) || SNAP_TEAM.with(|s| s.borrow().is_some()));
            paint_team(&ui, lang);
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
        return;
    }
    WAIT.with(|w| w.set(Some(turns)));
    if let Some(ui) = app.weak.upgrade() {
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
    team.on_swb_cmd(move |key| {
        if let Some(text) = tenrec::view::team::swb_command(&key) {
            command(&a, text);
        }
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
