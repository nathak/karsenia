//! The limits `claude` reported for each account (what the core keeps in `usage.json`): the header chip and its card for the account the
//! window shows, and one line on each account's row in Settings. The numbers come from the core's `account_usage` answer, asked again
//! whenever it says `usage_changed`; what is on screen is worked out again every minute (a window may have reset meanwhile). The card
//! can ask the program for the numbers now (its refresh button, and by itself when it is opened with old or ended windows).
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use tenrec::usage::AccountUsage;
use tenrec::view::i18n::tr;
use tenrec::view::usage::usage_view;

use crate::{Acct, App, AppWindow, Usage, UsageAcct, UsageWin};

thread_local! {
    static DATA: RefCell<HashMap<String, AccountUsage>> = RefCell::new(HashMap::new());
    static TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
    // The cost ledger and the two budgets as the core last said them, and what Settings last showed in the budget fields.
    static LEDGER: RefCell<(tenrec::costs::Days, Option<f64>, Option<f64>)> = RefCell::new((tenrec::costs::Days::new(), None, None));
    static BUDGET_NOTE: RefCell<String> = const { RefCell::new(String::new()) };
    // An ask for the usage is on its way (the button waits), and the word about how the last one ended.
    static BUSY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    // How many accounts' answers are still to come for the ask that is on its way (the button waits for all of them).
    static PENDING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    // The word about how the last ask ended, and for which account (it shows only on that account's card).
    static NOTE: RefCell<(String, String)> = const { RefCell::new((String::new(), String::new())) };
    // Lets the button go if no answer ever comes (the link was cut): longer than the ask itself may take.
    // The ask on its way was started by the button (not by the card opening).
    static MANUAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static BUSY_TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// A card that is opened asks by itself when it holds nothing, or a window has ended, or what it holds is older than this.
const STALE_MS: u64 = 5 * 60 * 1000;

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
        "cost_changed" => app.bus.post_quiet(json!({"op": "cost_ledger"})),
        "cost_ledger" => {
            let Ok(days) = serde_json::from_value::<tenrec::costs::Days>(data["days"].clone()) else { return };
            let (daily, monthly) = (data["daily"].as_f64(), data["monthly"].as_f64());
            // The budget fields show what is stored, but only when that changed (a number being typed is not overwritten by a new cost).
            let changed = LEDGER.with(|l| {
                let l = l.borrow();
                l.1 != daily || l.2 != monthly
            });
            LEDGER.with(|l| *l.borrow_mut() = (days, daily, monthly));
            if changed {
                let u = ui.global::<Usage>();
                u.set_budget_day(daily.map(|v| v.to_string()).unwrap_or_default().into());
                u.set_budget_month(monthly.map(|v| v.to_string()).unwrap_or_default().into());
            }
            render(ui, app);
        }
        "cost_result" => {
            let note = if data["ok"] == true { tr(app.lang, "cost_budget_saved") } else { tr(app.lang, "cost_budget_bad") };
            BUDGET_NOTE.with(|n| *n.borrow_mut() = note);
            render(ui, app);
        }
        "usage_refresh" => {
            // The button waits for every account asked.
            let left = PENDING.with(|p| {
                p.set(p.get().saturating_sub(1));
                p.get()
            });
            if left == 0 {
                BUSY.with(|b| b.set(false));
                BUSY_TIMER.with(|t| *t.borrow_mut() = None);
            }
            // Said in words. A "soon" is said only to a press of the button (the person is waiting for something to happen); the
            // automatic ask that is too soon, or that meets one running, says nothing. What was learned arrives as `usage_changed`.
            let by_hand = MANUAL.with(|m| m.get());
            if left == 0 {
                MANUAL.with(|m| m.set(false));
            }
            let account = data["account"].as_str().unwrap_or_default().to_owned();
            let note = match data["error"].as_str() {
                None => String::new(),
                Some("busy") => String::new(),
                Some("soon") => if by_hand { tr(app.lang, "us_ref_soon") } else { String::new() },
                Some("unreadable") => tr(app.lang, "us_ref_unreadable"),
                Some(_) => tr(app.lang, "us_ref_failed"),
            };
            // A failure is kept (the card says which account's); a success of that account takes it away.
            NOTE.with(|n| {
                let mut n = n.borrow_mut();
                if !note.is_empty() {
                    *n = (account, note);
                } else if n.0 == account {
                    *n = (String::new(), String::new());
                }
            });
            render(ui, app);
        }
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
    let who = acct.get_names().row_data(acct.get_pre_acct().max(0) as usize).unwrap_or_default();
    u.set_who(who);
    u.set_chip(view.chip.into());
    u.set_level(view.chip_level.into());
    u.set_seen(view.seen.into());
    u.set_empty(!view.has_data);
    // Every account side by side: its 5-hour and weekly windows.
    let now = tenrec::now_ms();
    let table: Vec<UsageAcct> = crate::accounts::picker()
        .into_iter()
        .map(|(id, name)| {
            let v = DATA.with(|d| usage_view(d.borrow().get(&id), now, app.lang));
            UsageAcct { name: name.into(), five: v.five.0.into(), five_level: v.five.1.into(), week: v.week.0.into(), week_level: v.week.1.into(), shown: id == key }
        })
        .collect();
    u.set_accounts(ModelRc::from(Rc::new(VecModel::from(table))));
    // The cost block, for the account the window shows.
    let cost = LEDGER.with(|l| {
        let l = l.borrow();
        tenrec::view::cost::cost_view(&l.0, &key, &tenrec::costs::local_day(), l.1, l.2, app.lang)
    });
    u.set_cost_today(cost.today.into());
    u.set_cost_week(cost.week.into());
    u.set_cost_month(cost.month.into());
    u.set_cost_line(cost.budget_line.into());
    u.set_cost_level(cost.level.into());
    u.set_cost_empty(!cost.has_data);
    u.set_budget_note(BUDGET_NOTE.with(|n| n.borrow().clone()).into());
    u.set_busy(BUSY.with(|b| b.get()));
    let key = app.bus.account().unwrap_or_default();
    u.set_note(NOTE.with(|n| { let n = n.borrow(); if n.0 == key { n.1.clone() } else { String::new() } }).into());
    u.set_assume(view.wins.iter().any(|w| w.key == "seven_day_overage_included"));
    let wins: Vec<UsageWin> = view.wins.into_iter().map(|w| UsageWin { label: w.label.into(), pct: w.pct.into(), frac: w.frac, level: w.level.into(), reset: w.reset.into() }).collect();
    u.set_wins(ModelRc::from(Rc::new(VecModel::from(wins))));
}

/// Asks the program for the usage of the account the window shows. `auto`: only when what the card holds is worth asking about.
fn refresh(ui: &AppWindow, app: &Arc<App>, auto: bool) {
    if BUSY.with(|b| b.get()) {
        return;
    }
    // Every Claude account (the card's table has them all); with none known yet, the one the window shows.
    let mut keys: Vec<String> = crate::accounts::picker().into_iter().map(|(id, _)| id).collect();
    if keys.is_empty() {
        keys.push(app.bus.account().unwrap_or_default());
    }
    if auto {
        let now = tenrec::now_ms();
        // What the program's `/usage` can tell: the 5-hour window and the week (other windows come only from a stream and are not
        // refreshed by it, so their age says nothing). Only an account whose numbers are worth asking about is asked.
        keys.retain(|key| {
            DATA.with(|d| match d.borrow().get(key) {
                None => true,
                Some(wins) => {
                    let asked: Vec<_> = wins.iter().filter(|(k, _)| matches!(k.as_str(), "five_hour" | "seven_day")).map(|(_, w)| w).collect();
                    asked.is_empty() || asked.iter().any(|w| w.resets_at + 60 < now / 1000) || asked.iter().map(|w| w.seen_at).max().is_some_and(|seen| now.saturating_sub(seen) > STALE_MS)
                }
            })
        });
        if keys.is_empty() {
            return;
        }
    }
    BUSY.with(|b| b.set(true));
    PENDING.with(|p| p.set(keys.len()));
    MANUAL.with(|m| m.set(!auto));
    NOTE.with(|n| *n.borrow_mut() = (String::new(), String::new()));
    let weak = ui.as_weak();
    let timer = slint::Timer::default();
    // The longer ask of several accounts (each one runs the program's start-up hooks): let the button go if some answer never comes.
    timer.start(slint::TimerMode::SingleShot, Duration::from_secs(45 + 15 * keys.len() as u64), move || {
        BUSY.with(|b| b.set(false));
        PENDING.with(|p| p.set(0));
        if let Some(ui) = weak.upgrade() {
            ui.global::<Usage>().set_busy(false);
        }
    });
    BUSY_TIMER.with(|t| *t.borrow_mut() = Some(timer));
    for key in keys {
        app.bus.post_quiet(json!({"op": "account_usage_refresh", "account": key}));
    }
    render(ui, app);
}

/// A budget as typed: empty is none (null); a number is sent as it is (the core checks the range). None when it is not a number.
fn typed_budget(text: &str) -> Option<serde_json::Value> {
    let t = text.trim();
    if t.is_empty() {
        return Some(serde_json::Value::Null);
    }
    // A comma is a decimal comma (`12,5`). A comma with three digits after it (`1,000`) is a thousands mark in some places and a
    // decimal comma in others: it is not guessed, and one beside a point is refused too.
    if t.contains(',') && (t.contains('.') || t.rsplit(',').next().is_some_and(|after| after.len() == 3 && after.bytes().all(|c| c.is_ascii_digit()))) {
        return None;
    }
    let t = t.replace(',', ".");
    t.parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| json!(v))
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    app.bus.post_quiet(json!({"op": "cost_ledger"}));
    let (a, weak) = (app.clone(), ui.as_weak());
    ui.global::<Usage>().on_budget_save(move |daily, monthly| {
        let Some(ui) = weak.upgrade() else { return };
        match (typed_budget(daily.as_str()), typed_budget(monthly.as_str())) {
            (Some(d), Some(m)) => a.bus.post_quiet(json!({"op": "cost_budget_set", "daily": d, "monthly": m})),
            _ => {
                BUDGET_NOTE.with(|n| *n.borrow_mut() = tr(a.lang, "cost_budget_bad"));
                render(&ui, &a);
            }
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    ui.global::<Usage>().on_refresh(move || {
        if let Some(ui) = weak.upgrade() {
            refresh(&ui, &a, false);
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    ui.global::<Usage>().on_opened(move || {
        if let Some(ui) = weak.upgrade() {
            refresh(&ui, &a, true);
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_budget_is_none_a_number_or_refused() {
        assert_eq!(typed_budget("  "), Some(serde_json::Value::Null));
        assert_eq!(typed_budget("12,5"), Some(json!(12.5)));
        assert_eq!(typed_budget("20"), Some(json!(20.0)));
        for bad in ["abc", "1e999", "NaN", "$5", "1,000", "1.000,5", "1,000.5"] {
            assert_eq!(typed_budget(bad), None, "{bad}");
        }
    }
}
