//! The "/" menu of the composer: what is shown for the word being typed, and what picking a row does (the list and the filter are
//! `tenrec::view::slash`). A command that needs an argument fills the draft; one that does not is sent at once; `/clear` asks first.
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tenrec::view::i18n::tr;
use tenrec::view::slash::{draft_query, list, Entry};

use crate::{App, AppWindow, Slash, SlashRow};

thread_local! {
    /// The entries the rows show, in the same order.
    static SHOWN: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

fn refresh(ui: &AppWindow, app: &App, draft: &str) {
    let g = ui.global::<Slash>();
    // The menu is for the composer of a chat that can take a command now (not while a turn runs, not in shell mode).
    let query = draft_query(draft).filter(|_| ui.get_has_chat() && ui.get_state() != "running" && !ui.get_shell_mode());
    let Some(query) = query else {
        g.set_open(false);
        return;
    };
    let names = app.slash_names.lock().expect("slash names").clone();
    let found = list(app.lang, &names, query);
    let rows: Vec<SlashRow> = found.iter().map(|e| SlashRow { name: e.name.clone().into(), about: e.about.clone().into(), arg: e.arg, confirm: e.confirm, builtin: e.builtin }).collect();
    SHOWN.with(|s| *s.borrow_mut() = found);
    g.set_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    g.set_sel(0);
    g.set_note(if names.is_empty() { tr(app.lang, "slash_first").into() } else { SharedString::new() });
    g.set_open(true);
}

fn pick(ui: &AppWindow, app: &Arc<App>, index: i32) {
    let Some(entry) = SHOWN.with(|s| usize::try_from(index).ok().and_then(|i| s.borrow().get(i).cloned())) else { return };
    ui.global::<Slash>().set_open(false);
    if ui.get_state() == "running" {
        crate::toast(ui, tr(app.lang, "slash_busy"));
        return;
    }
    let text = format!("/{}", entry.name);
    if entry.arg {
        ui.set_draft(format!("{text} ").into());
        ui.invoke_focus_composer();
        return;
    }
    ui.set_draft(SharedString::new());
    if entry.confirm {
        let a = app.clone();
        crate::panels::ask(ui, tr(app.lang, "slash_clear_confirm"), tr(app.lang, "slash_clear_ok"), true, Box::new(move |yes| {
            if yes {
                crate::send_text(&a, text, false);
            }
        }));
    } else {
        crate::send_text(app, text, false);
    }
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let g = ui.global::<Slash>();
    let (weak, a) = (ui.as_weak(), app.clone());
    g.on_draft_changed(move |draft| {
        if let Some(ui) = weak.upgrade() {
            refresh(&ui, &a, &draft);
        }
    });
    let (weak, a) = (ui.as_weak(), app.clone());
    g.on_pick(move |i| {
        if let Some(ui) = weak.upgrade() {
            pick(&ui, &a, i);
        }
    });
}
