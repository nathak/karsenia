//! A desktop notification when something needs the person: a chat that started to wait for a permission, a turn that took a while and
//! has ended, a program that stopped with an error (see `view::notify` for when). On Linux through `notify-send` when it is installed
//! (started with its arguments, never through a shell), on Windows as a balloon of the tray icon (see `tray::notify`: no program is
//! started for it); on other systems nothing is shown. The text is fixed: no title, command, file or answer of a chat goes out (a title
//! is the first words of the first message). It can be turned off from the palette.
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use tenrec::view::i18n::{tr, Lang};
use tenrec::view::notify::{gate, Chat, Reason, Watch};

fn state() -> &'static Mutex<Watch> {
    static ST: OnceLock<Mutex<Watch>> = OnceLock::new();
    ST.get_or_init(|| Mutex::new(Watch::default()))
}

/// When the last notice was sent (ms): two are not sent within `GAP_MS`.
static LAST: AtomicU64 = AtomicU64::new(0);
const GAP_MS: u64 = 5_000;
/// What is due and not yet said (a notice that came within `GAP_MS` of the last one waits, it is not dropped): the most urgent reason.
fn due() -> &'static Mutex<Option<Reason>> {
    static DUE: OnceLock<Mutex<Option<Reason>>> = OnceLock::new();
    DUE.get_or_init(|| Mutex::new(None))
}

/// Whether notifications are on (the setting `notify` of `desktop.json`; on unless it is false).
pub fn on(data: &Path) -> bool {
    crate::desktop_settings(data).get("notify").and_then(|v| v.as_bool()).unwrap_or(true)
}

/// `chats`: every chat as it looks now; `current`: the chat on screen.
pub fn check(data: &Path, lang: Lang, chats: &[Chat], current: Option<&str>) {
    // The numbers are always kept up to date, so turning notifications on does not announce what already waited.
    let now = tenrec::now_ms();
    let tell = state().lock().unwrap_or_else(|e| e.into_inner()).update(chats, current, crate::tray::app_in_front(), now);
    // One notification however many chats are due: the most urgent reason is the one said. What cannot be said yet (a notice went out a
    // moment ago) waits for a later look, so it is not lost; turned off, nothing is kept.
    let mut waiting = due().lock().unwrap_or_else(|e| e.into_inner());
    if !on(data) {
        *waiting = None;
        return;
    }
    let Some(reason) = gate(&mut waiting, tell.iter().map(|(_, r)| *r), now, LAST.load(Ordering::Relaxed), GAP_MS) else { return };
    LAST.store(now, Ordering::Relaxed);
    let (title, body) = match reason {
        Reason::Permission => ("notify_title", "notify_body"),
        Reason::Failed => ("notify_failed_title", "notify_failed_body"),
        Reason::Done => ("notify_done_title", "notify_done_body"),
    };
    send(&tr(lang, title), &tr(lang, body));
}

#[cfg(target_os = "linux")]
fn send(title: &str, body: &str) {
    // Not waited for: a missing `notify-send` or a notification daemon that is slow must not hold the refresh.
    let _ = std::process::Command::new("notify-send")
        .args(["--app-name=Tenrec", "--expire-time=8000", "--", title, body])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut child| {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        });
}

#[cfg(windows)]
fn send(title: &str, body: &str) {
    crate::tray::notify(title, body);
}

#[cfg(not(any(target_os = "linux", windows)))]
fn send(_title: &str, _body: &str) {}

/// The first look at the budgets is the start of the app: what is already used up is written down and not announced.
static BUDGET_SEEN: AtomicBool = AtomicBool::new(false);

/// A notice when a cost budget (the day's or the month's) is first used up in its period. Told once per period: the day and the month
/// that were told are kept in `desktop.json`, so a restart does not tell it again.
pub fn check_budget(data: &Path, lang: Lang, days: &tenrec::costs::Days, daily: Option<f64>, monthly: Option<f64>) {
    let started = BUDGET_SEEN.swap(true, Ordering::Relaxed);
    if daily.is_none() && monthly.is_none() {
        return;
    }
    let today = tenrec::costs::local_day();
    // A period is told with the budget it was told for: raising the budget and using that up too is news again.
    let crossed: Vec<(bool, String)> = tenrec::view::cost::crossed(days, &today, daily, monthly)
        .into_iter()
        .map(|(month, id)| {
            let budget = if month { monthly } else { daily }.unwrap_or(0.0);
            (month, format!("{id}@{budget}"))
        })
        .collect();
    let told = crate::desktop_settings(data).get("budget_told").cloned().unwrap_or_default();
    let (told_day, told_month) = (told.get("d").and_then(|v| v.as_str()).map(str::to_owned), told.get("m").and_then(|v| v.as_str()).map(str::to_owned));
    let fresh = tenrec::view::cost::untold(&crossed, told_day.as_deref(), told_month.as_deref());
    if fresh.is_empty() {
        return;
    }
    let now = tenrec::now_ms();
    // At the start, or with notices turned off, it is only written down; within the gap of the last notice it waits for the next look.
    let say = started && on(data);
    if say && now.saturating_sub(LAST.load(Ordering::Relaxed)) < GAP_MS {
        return;
    }
    let mut record = serde_json::json!({"d": told_day, "m": told_month});
    for (month, id) in &fresh {
        record[if *month { "m" } else { "d" }] = serde_json::json!(id);
    }
    crate::remember(data, "budget_told", record);
    if say {
        LAST.store(now, Ordering::Relaxed);
        send(&tr(lang, "notify_budget_title"), &tr(lang, "notify_budget_body"));
    }
}
