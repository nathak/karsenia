//! The limits `claude` reported for each account (what the core keeps in `usage.json`): the header chip and its card for the account the
//! window shows, and one line on each account's row in Settings. The numbers come from the core's `account_usage` answer, asked again
//! whenever it says `usage_changed`; what is on screen is worked out again every minute (a window may have reset meanwhile).
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use tenrec::usage::AccountUsage;
use tenrec::view::usage::usage_view;

use crate::{Acct, App, AppWindow, Usage, UsageWin};

thread_local! {
    static DATA: RefCell<HashMap<String, AccountUsage>> = RefCell::new(HashMap::new());
    static TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

pub fn on_reply(ui: &AppWindow, app: &Arc<App>, kind: &str, data: Value) {
    match kind {
        "account_usage" => {
            let Ok(accounts) = serde_json::from_value::<HashMap<String, AccountUsage>>(data["accounts"].clone()) else { return };
            DATA.with(|d| *d.borrow_mut() = accounts);
            // The rows of Settings show it too; that redraws the chip as well.
            crate::accounts::render(ui, app);
            render(ui, app);
        }
        "usage_changed" => app.bus.post_quiet(json!({"op": "account_usage"})),
        _ => {}
    }
}

/// The line on an account's row: what it says and how worrying it is ("" when nothing was reported yet).
pub fn compact(id: &str, app: &App) -> (String, String) {
    DATA.with(|d| {
        let d = d.borrow();
        let view = usage_view(d.get(id), tenrec::now_ms(), app.lang);
        (view.compact, view.compact_level.to_owned())
    })
}

/// The chip and the card, for the account the window shows.
pub fn render(ui: &AppWindow, app: &App) {
    let key = app.bus.account().unwrap_or_default();
    let view = DATA.with(|d| usage_view(d.borrow().get(&key), tenrec::now_ms(), app.lang));
    let u = ui.global::<Usage>();
    let acct = ui.global::<Acct>();
    let who = acct.get_names().row_data(acct.get_view().max(0) as usize).unwrap_or_default();
    u.set_who(who);
    u.set_chip(view.chip.into());
    u.set_level(view.chip_level.into());
    u.set_seen(view.seen.into());
    u.set_empty(!view.has_data);
    u.set_assume(view.wins.iter().any(|w| w.key == "seven_day_overage_included"));
    let wins: Vec<UsageWin> = view.wins.into_iter().map(|w| UsageWin { label: w.label.into(), pct: w.pct.into(), frac: w.frac, level: w.level.into(), reset: w.reset.into() }).collect();
    u.set_wins(ModelRc::from(Rc::new(VecModel::from(wins))));
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    render(ui, app);
    app.bus.post_quiet(json!({"op": "account_usage"}));
    let (a, weak) = (app.clone(), ui.as_weak());
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_secs(60), move || {
        if let Some(ui) = weak.upgrade() {
            crate::accounts::render(&ui, &a);
            render(&ui, &a);
        }
    });
    TIMER.with(|t| *t.borrow_mut() = Some(timer));
}
