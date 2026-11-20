//! A desktop notification when something needs the person: a chat that started to wait for a permission, a turn that took a while and
//! has ended, a program that stopped with an error (see `view::notify` for when). On Linux through `notify-send` when it is installed
//! (started with its arguments, never through a shell), on Windows as a balloon of the tray icon (see `tray::notify`: no program is
//! started for it); on other systems nothing is shown. The text is fixed: no title, command, file or answer of a chat goes out (a title
//! is the first words of the first message). It can be turned off from the palette.
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use tenrec::view::i18n::{tr, Lang};
use tenrec::view::notify::{Chat, Reason, Watch};

fn state() -> &'static Mutex<Watch> {
    static ST: OnceLock<Mutex<Watch>> = OnceLock::new();
    ST.get_or_init(|| Mutex::new(Watch::default()))
}

/// When the last notice was sent (ms): two are not sent within `GAP_MS`.
static LAST: AtomicU64 = AtomicU64::new(0);
const GAP_MS: u64 = 5_000;

/// Whether notifications are on (the setting `notify` of `desktop.json`; on unless it is false).
pub fn on(data: &Path) -> bool {
    crate::desktop_settings(data).get("notify").and_then(|v| v.as_bool()).unwrap_or(true)
}

/// `chats`: every chat as it looks now; `current`: the chat on screen.
pub fn check(data: &Path, lang: Lang, chats: &[Chat], current: Option<&str>) {
    // The numbers are always kept up to date, so turning notifications on does not announce what already waited.
    let now = tenrec::now_ms();
    let tell = state().lock().unwrap_or_else(|e| e.into_inner()).update(chats, current, crate::tray::app_in_front(), now);
    // One notification however many chats are due: the most urgent reason is the one said.
    let Some(reason) = tell.iter().map(|(_, r)| *r).max() else { return };
    if !on(data) || now.saturating_sub(LAST.load(Ordering::Relaxed)) < GAP_MS {
        return;
    }
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
