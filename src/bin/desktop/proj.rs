//! The card that asks about a project's own Claude Code settings (see `tenrec::project_check`). The core refuses the first message of a
//! chat in a risky project and sends what it found; the person trusts it as it is (the hash of what was shown) or starts without the
//! project's settings, and sends the message again.
use std::cell::RefCell;
use std::sync::Arc;
use std::rc::Rc;

use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};
use tenrec::view::i18n::tr;
use tenrec::view::project_check::lines;

use crate::{App, AppWindow, Proj, WtLine};

/// The most lines the card lists (a project can have many; the rest are counted).
const MAX_LINES: usize = 12;

thread_local! {
    /// The hash of what the card shows: a trust is for exactly that.
    static HASH: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The chat on screen changed: nothing of the old one stays.
pub fn reset(ui: &AppWindow) {
    HASH.with(|h| h.borrow_mut().clear());
    let p = ui.global::<Proj>();
    p.set_show(false);
    p.set_busy(false);
    p.set_lines(ModelRc::default());
}

/// The core refused a message and says what it found.
pub fn show(ui: &AppWindow, app: &App, data: &Value) {
    let mut all = lines(data, app.lang);
    let more = all.len().saturating_sub(MAX_LINES);
    all.truncate(MAX_LINES);
    let mut rows: Vec<WtLine> = all.into_iter().map(|(text, bad)| WtLine { text: text.into(), bad }).collect();
    if more > 0 {
        rows.push(WtLine { text: format!("… +{more}").into(), bad: false });
    }
    HASH.with(|h| *h.borrow_mut() = data["hash"].as_str().unwrap_or_default().to_owned());
    let p = ui.global::<Proj>();
    p.set_lines(ModelRc::from(Rc::new(VecModel::from(rows))));
    p.set_busy(false);
    p.set_show(true);
}

/// A decision was taken (here or elsewhere): the card goes.
pub fn close(ui: &AppWindow) {
    let p = ui.global::<Proj>();
    p.set_show(false);
    p.set_busy(false);
}

fn decide(ui: &AppWindow, app: &Arc<App>, without: bool) {
    let Some(sid) = app.current.lock().expect("current").sid.clone() else { return };
    let hash = HASH.with(|h| h.borrow().clone());
    ui.global::<Proj>().set_busy(true);
    let a = app.clone();
    tokio::spawn(async move {
        let result = match a.registry.get(&sid) {
            Some(handle) => handle.project_decide(hash, without).await,
            None => Err("the chat is gone".into()),
        };
        let b = a.clone();
        let _ = a.weak.upgrade_in_event_loop(move |ui| {
            ui.global::<Proj>().set_busy(false);
            match result {
                Ok(()) => {
                    close(&ui);
                    crate::toast(&ui, tr(b.lang, "proj_done"));
                }
                // The settings changed again: the core sent the new findings, which the card already shows.
                Err(message) => crate::toast(&ui, message),
            }
        });
    });
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let p = ui.global::<Proj>();
    let (weak, a) = (ui.as_weak(), app.clone());
    p.on_trust(move || {
        let Some(ui) = weak.upgrade() else { return };
        let (b, w) = (a.clone(), weak.clone());
        crate::panels::ask(&ui, tr(a.lang, "proj_trust_confirm"), tr(a.lang, "proj_trust"), true, Box::new(move |yes| {
            if let (true, Some(ui)) = (yes, w.upgrade()) {
                decide(&ui, &b, false);
            }
        }));
    });
    let (weak, a) = (ui.as_weak(), app.clone());
    p.on_without(move || {
        let Some(ui) = weak.upgrade() else { return };
        let (b, w) = (a.clone(), weak.clone());
        crate::panels::ask(&ui, tr(a.lang, "proj_without_note"), tr(a.lang, "proj_without_ok"), false, Box::new(move |yes| {
            if let (true, Some(ui)) = (yes, w.upgrade()) {
                decide(&ui, &b, true);
            }
        }));
    });
}

