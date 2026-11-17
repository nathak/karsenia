//! A desktop notification when a chat that is not on screen starts to wait for a permission. Only on Linux, through `notify-send` when
//! it is installed (started with its arguments, never through a shell); on other systems nothing is shown. The text is fixed: no
//! title, command, file or answer of a chat goes out (a title is the first words of the first message). It can be turned off from
//! the palette.
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use tenrec::view::i18n::{tr, Lang};
use tenrec::view::notify::Waiting;

fn state() -> &'static Mutex<Waiting> {
    static ST: OnceLock<Mutex<Waiting>> = OnceLock::new();
    ST.get_or_init(|| Mutex::new(Waiting::default()))
}

/// Whether notifications are on (the setting `notify` of `desktop.json`; on unless it is false).
pub fn on(data: &Path) -> bool {
    crate::desktop_settings(data).get("notify").and_then(|v| v.as_bool()).unwrap_or(true)
}

/// `chats`: (id, waiting requests) of every chat.
pub fn check(data: &Path, lang: Lang, chats: &[(String, u32)], current: Option<&str>) {
    // The numbers are always kept up to date, so turning notifications on does not announce what already waited.
    let tell = state().lock().unwrap_or_else(|e| e.into_inner()).update(chats, current);
    if tell.is_empty() || !on(data) {
        return;
    }
    // One notification however many chats started to wait.
    send(&tr(lang, "notify_title"), &tr(lang, "notify_body"));
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

#[cfg(not(target_os = "linux"))]
fn send(_title: &str, _body: &str) {}
