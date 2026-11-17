//! The VHDB page: what the plug-in left in the open chat's project (see `tenrec::view::vhdb`), read off the window's thread and
//! shown as it is. Nothing is written, and nothing is sent to a chat.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, VecModel};
use tenrec::view::i18n::{fill, tr, Lang};
use tenrec::view::panels::ago;
use tenrec::view::vhdb::{read, session_log, vhdb_dir, Mine, SessionLog, Summary};

use crate::{App, AppWindow, Vh, VhDecision};

fn show(ui: &AppWindow, lang: Lang, found_dir: bool, s: &Summary, mine: SessionLog) {
    let vh = ui.global::<Vh>();
    vh.set_has_chat(true);
    vh.set_found(found_dir && s.found);
    vh.set_record(s.switches.record);
    vh.set_inject(s.switches.inject);
    vh.set_guard(s.switches.guard);
    vh.set_upload(s.switches.upload);
    vh.set_budget(s.switches.budget_tokens as i32);
    vh.set_deadline(s.switches.deadline_ms as i32);
    let now = tenrec::now_ms();
    // The first line is about this chat's own session (its log, or why there is none); the decisions below are the project's, since the
    // plug-in does not tie a decision to a Claude session, and the switches above are the project's too.
    let when = if mine.last_at > 0 { ago(mine.last_at, now, lang) } else { "—".to_owned() };
    let session_line = match mine.state {
        Mine::Log => fill(&tr(lang, "vh_logs_session"), &[("when", when)]),
        Mine::NoLog => tr(lang, "vh_session_nolog"),
        Mine::NoSession => tr(lang, "vh_session_none"),
        Mine::Unmappable => tr(lang, "vh_session_unmapped"),
    };
    vh.set_logs(format!("{session_line}\n{}", tr(lang, "vh_decisions_scope")).into());
    let rows: Vec<VhDecision> = s
        .decisions
        .iter()
        .map(|d| VhDecision {
            id: d.id.clone().into(),
            topic: d.topic.clone().into(),
            chosen: d.chosen.clone().into(),
            reason: d.reason.clone().into(),
            files: d.files.join(" · ").into(),
            when: if d.at > 0 { ago(d.at, now, lang) } else { String::new() }.into(),
            active: d.active,
        })
        .collect();
    vh.set_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
}

use std::rc::Rc;

/// The newest request: an answer to an older one (another chat was opened since, or Refresh was pressed again) is dropped.
static GEN: AtomicU64 = AtomicU64::new(0);

/// The page was opened, the chat changed or Refresh was pressed: the folder of the chat on screen is read again. The chat's own
/// folder is asked of the registry (the window's copy of it may still be the previous chat's), and the answer is shown only if
/// it is still the newest request and the chat is still the one on screen.
pub fn open(ui: &AppWindow, app: &Arc<App>) {
    let generation = GEN.fetch_add(1, Ordering::AcqRel) + 1;
    let Some(sid) = app.current.lock().expect("current").sid.clone() else {
        let vh = ui.global::<Vh>();
        vh.set_has_chat(false);
        vh.set_found(false);
        vh.set_rows(ModelRc::default());
        return;
    };
    let a = app.clone();
    tokio::spawn(async move {
        let Some(info) = async { a.registry.get(&sid)?.info().await }.await else { return };
        let cwd = info.meta.cwd.clone();
        // The chat's own Claude session: only its log line is about it.
        let claude_session = info.meta.claude_session_id.clone();
        // A few small files: read where the window never waits for the disk.
        let Ok((dir, summary, mine)) = tokio::task::spawn_blocking(move || {
            let dir = vhdb_dir(&cwd);
            let summary = dir.as_deref().map(read).unwrap_or_default();
            let mine = dir.as_deref().map(|d| session_log(d, claude_session.as_deref())).unwrap_or(SessionLog { state: Mine::NoLog, last_at: 0 });
            (dir, summary, mine)
        })
        .await
        else {
            return;
        };
        let (b, lang) = (a.clone(), a.lang);
        let _ = a.weak.upgrade_in_event_loop(move |ui| {
            let same = b.current.lock().expect("current").sid.as_deref() == Some(sid.as_str());
            if same && GEN.load(Ordering::Acquire) == generation {
                show(&ui, lang, dir.is_some(), &summary, mine);
            }
        });
    });
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let (weak, a) = (ui.as_weak(), app.clone());
    ui.global::<Vh>().on_refresh(move || {
        if let Some(ui) = weak.upgrade() {
            open(&ui, &a);
        }
    });
}
