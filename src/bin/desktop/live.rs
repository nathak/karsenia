//! The Team page's live view card: starts and stops the local web server of `tenrec::live` (read only, 127.0.0.1, its own token), opens
//! its page in the browser and copies its address. The server is the app's own and ends with it (the window going to the tray does not stop it).
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tenrec::live::server::{start, LiveServer, DEFAULT_PORT};
use tenrec::live::Prompts;
use slint::ComponentHandle;
use tenrec::view::i18n::{fill, tr};

use crate::{App, AppWindow, Team};

/// Set while a start is on its way: a second click does not start a second server.
static STARTING: AtomicBool = AtomicBool::new(false);

fn slot() -> &'static Mutex<Option<LiveServer>> {
    static SLOT: OnceLock<Mutex<Option<LiveServer>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Ends the server (when the app ends).
pub fn stop() {
    if let Some(server) = slot().lock().expect("live lock").take() {
        server.stop();
    }
}

/// The address to open for the chat on screen (its tab is the one shown first), with the page in the app's language.
fn address(app: &App) -> Option<String> {
    let lang = if app.lang == tenrec::view::i18n::Lang::Ko { "ko" } else { "en" };
    let sid = app.current.lock().expect("current").sid.clone();
    slot().lock().expect("live lock").as_ref().map(|server| server.url(sid.as_deref(), lang))
}

/// Opens an address in the browser the system names for it. The address goes to the opener as an argument (not through a shell).
fn open_url(url: &str) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).creation_flags(0x0800_0000).spawn().is_ok()
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn().is_ok()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().is_ok()
    }
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let team = ui.global::<Team>();
    let a = app.clone();
    team.on_live_toggle(move || toggle(&a));
    let a = app.clone();
    team.on_live_open(move || {
        let Some(url) = address(&a) else { return };
        if !open_url(&url) {
            crate::toast_later(&a.weak, tr(a.lang, "live_open_failed"));
        }
    });
    let a = app.clone();
    team.on_live_copy(move || {
        if let Some(url) = address(&a) {
            crate::copy_text(&a, url);
        }
    });
    team.on_live_short_set(move |on| {
        if let Some(server) = slot().lock().expect("live lock").as_ref() {
            server.set_prompts(if on { Prompts::Short } else { Prompts::Full });
        }
    });
}

fn toggle(app: &Arc<App>) {
    if let Some(server) = slot().lock().expect("live lock").take() {
        server.stop();
        set_state(app, false, String::new());
        crate::toast_later(&app.weak, tr(app.lang, "live_stopped"));
        return;
    }
    if STARTING.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(ui) = app.weak.upgrade() else {
        STARTING.store(false, Ordering::SeqCst);
        return;
    };
    let prompts = if ui.global::<Team>().get_live_short() { Prompts::Short } else { Prompts::Full };
    let a = app.clone();
    let registry = app.registry.clone();
    tokio::spawn(async move {
        let started = start(registry, DEFAULT_PORT, prompts).await;
        STARTING.store(false, Ordering::SeqCst);
        match started {
            Ok(server) => {
                let port = server.addr.port();
                *slot().lock().expect("live lock") = Some(server);
                set_state(&a, true, fill(&tr(a.lang, "live_on"), &[("port", port.to_string())]));
            }
            Err(error) => {
                tracing::warn!("the live view could not start: {error:#}");
                crate::toast_later(&a.weak, fill(&tr(a.lang, "live_failed"), &[("e", error.to_string())]));
            }
        }
    });
}

fn set_state(app: &Arc<App>, on: bool, label: String) {
    let _ = app.weak.upgrade_in_event_loop(move |ui| {
        let team = ui.global::<Team>();
        team.set_live_on(on);
        team.set_live_label(label.into());
    });
}
