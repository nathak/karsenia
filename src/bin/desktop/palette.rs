//! The command palette's rows and what each command does (the list and the filter are `tenrec::view::palette`). Every command is
//! something a click already does; nothing here sends text to a chat.
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, VecModel};
use tenrec::view::i18n::tr;
use tenrec::view::palette::{commands, filter, Cmd};

use crate::{App, AppWindow, Files, Pal, PalRow, Theme};

thread_local! {
    /// The commands the rows show, in the same order.
    static SHOWN: RefCell<Vec<Cmd>> = const { RefCell::new(Vec::new()) };
}

fn show(ui: &AppWindow, app: &App, query: &str) {
    let found = filter(query, app.lang, &commands(crate::term::enabled()));
    let rows: Vec<PalRow> = found.iter().map(|c| PalRow { label: tr(app.lang, c.key).into(), hint: c.hint.into() }).collect();
    SHOWN.with(|s| *s.borrow_mut() = found);
    ui.global::<Pal>().set_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
}

fn page(ui: &AppWindow, side: i32) {
    ui.set_new_open(false);
    ui.set_side(side);
}

fn theme(ui: &AppWindow, app: &App, mode: i32) {
    ui.global::<Theme>().set_mode(mode);
    crate::remember(&app.data, "theme", mode.into());
}

fn run(ui: &AppWindow, app: &Arc<App>, index: i32) {
    let Some(cmd) = SHOWN.with(|s| usize::try_from(index).ok().and_then(|i| s.borrow().get(i).copied())) else { return };
    ui.global::<Pal>().set_open(false);
    match cmd.id {
        "go_chat" => page(ui, 0),
        "go_memory" => page(ui, 1),
        "go_skills" => page(ui, 2),
        "go_sched" => page(ui, 3),
        "go_files" => page(ui, 4),
        "settings" => page(ui, 5),
        "go_team" => page(ui, 6),
        "go_vhdb" => page(ui, 7),
        "go_board" => page(ui, 8),
        "new_chat" => {
            ui.set_new_acct(ui.global::<crate::Acct>().get_pre_acct());
            ui.set_new_prov(0);
            ui.global::<crate::Imp>().set_open(false);
            ui.set_new_open(true);
        }
        "terminal" => {
            page(ui, 4);
            let f = ui.global::<Files>();
            f.set_term_open(true);
            f.invoke_term_set(true);
        }
        "theme_dark" => theme(ui, app, 0),
        "theme_light" => theme(ui, app, 1),
        "theme_system" => theme(ui, app, 2),
        "notify_on" | "notify_off" => {
            let on = cmd.id == "notify_on";
            crate::remember(&app.data, "notify", on.into());
            crate::toast(ui, tr(app.lang, if on { "notify_is_on" } else { "notify_is_off" }));
        }
        _ => {}
    }
    ui.invoke_refocus();
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let pal = ui.global::<Pal>();
    let (weak, a) = (ui.as_weak(), app.clone());
    pal.on_filter(move |q| {
        if let Some(ui) = weak.upgrade() {
            show(&ui, &a, &q);
        }
    });
    let (weak, a) = (ui.as_weak(), app.clone());
    pal.on_run(move |i| {
        if let Some(ui) = weak.upgrade() {
            run(&ui, &a, i);
        }
    });
}
