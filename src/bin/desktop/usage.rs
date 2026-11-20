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

use crate::{Acct, App, AppWindow, Usage, UsageWin};

thread_local! {
    static DATA: RefCell<HashMap<String, AccountUsage>> = RefCell::new(HashMap::new());
    static TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
    // An ask for the usage is on its way (the button waits), and the word about how the last one ended.
    static BUSY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
        "usage_refresh" => {
            BUSY.with(|b| b.set(false));
            BUSY_TIMER.with(|t| *t.borrow_mut() = None);
            // Said in words, once. A "soon" is said only to a press of the button (the person is waiting for something to happen);
            // the automatic ask that is too soon, or that meets one running, says nothing. What was learned arrives as `usage_changed`.
            let by_hand = MANUAL.with(|m| m.replace(false));
            let note = match data["error"].as_str() {
                None => String::new(),
                Some("busy") => String::new(),
                Some("soon") => if by_hand { tr(app.lang, "us_ref_soon") } else { String::new() },
                Some("unreadable") => tr(app.lang, "us_ref_unreadable"),
                Some(_) => tr(app.lang, "us_ref_failed"),
            };
            NOTE.with(|n| *n.borrow_mut() = (data["account"].as_str().unwrap_or_default().to_owned(), note));
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
    let key = app.bus.account().unwrap_or_default();
    if auto {
        let now = tenrec::now_ms();
        // What the program's `/usage` can tell: the 5-hour window and the week (other windows come only from a stream and are not
        // refreshed by it, so their age says nothing).
        let worth = DATA.with(|d| match d.borrow().get(&key) {
            None => true,
            Some(wins) => {
                let asked: Vec<_> = wins.iter().filter(|(k, _)| matches!(k.as_str(), "five_hour" | "seven_day")).map(|(_, w)| w).collect();
                asked.is_empty() || asked.iter().any(|w| w.resets_at + 60 < now / 1000) || asked.iter().map(|w| w.seen_at).max().is_some_and(|seen| now.saturating_sub(seen) > STALE_MS)
            }
        });
        if !worth {
            return;
        }
    }
    BUSY.with(|b| b.set(true));
    MANUAL.with(|m| m.set(!auto));
    NOTE.with(|n| *n.borrow_mut() = (String::new(), String::new()));
    let weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::SingleShot, Duration::from_secs(45), move || {
        BUSY.with(|b| b.set(false));
        if let Some(ui) = weak.upgrade() {
            ui.global::<Usage>().set_busy(false);
        }
    });
    BUSY_TIMER.with(|t| *t.borrow_mut() = Some(timer));
    app.bus.post_quiet(json!({"op": "account_usage_refresh", "account": key}));
    render(ui, app);
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
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
