//! The terminal drawer of the Files page: one shell in the chat's folder, a plain-text scrollback and a line to type in (TERM=dumb,
//! no cursor addressing). It exists only when the app was started with TENREC_TERMINAL=1; the shell and its limits are the host's
//! (`server::term_ops`). What the shell prints is shown in the drawer and nowhere else: it is never logged or put in a toast.
//! Which shell belongs to the drawer is decided by `TermState` (the open request's number), never by "the next one that opens".
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use slint::ComponentHandle;
use tenrec::view::i18n::{fill, tr};
use tenrec::view::term::{parse_frame, TermState};
use tokio::sync::mpsc::Receiver;

use crate::{App, AppWindow, Files};

/// Longest line sent in one piece (the host refuses more than 4 KiB at once).
const MAX_LINE: usize = 3000;

fn st() -> &'static Mutex<TermState> {
    static ST: OnceLock<Mutex<TermState>> = OnceLock::new();
    ST.get_or_init(|| Mutex::new(TermState::default()))
}

fn lock() -> std::sync::MutexGuard<'static, TermState> {
    st().lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the shell is on for this run (a start option only; the window has no switch for it).
pub fn enabled() -> bool {
    cfg!(unix) && std::env::var("TENREC_TERMINAL").map(|v| v == "1").unwrap_or(false)
}

fn show(ui: &AppWindow) {
    let (text, status, live) = {
        let s = lock();
        (s.sb.text(), s.status.clone(), s.tid().is_some())
    };
    let f = ui.global::<Files>();
    if f.get_term_text() != text.as_str() {
        f.set_term_text(text.into());
    }
    if f.get_term_status() != status.as_str() {
        f.set_term_status(status.into());
    }
    f.set_term_live(live);
}

/// Ends the open shell (if any) and forgets its output.
fn close(app: &App) {
    let tid = lock().close();
    if let Some(tid) = tid {
        app.bus.post_quiet(json!({"op": "term_close", "tid": tid}));
    }
}

/// The chat on screen changed: the shell ended and the drawer is shut.
pub fn reset(ui: &AppWindow, app: &App) {
    close(app);
    ui.global::<Files>().set_term_open(false);
    show(ui);
}

fn open(ui: &AppWindow, app: &App) {
    let Some(sid) = app.current.lock().expect("current").sid.clone() else { return };
    // About one column per 8 px of the window; the shell is not a full terminal, so this only shapes its `ls` columns.
    let cols = (ui.window().size().width / 8).clamp(40, 200);
    {
        // The lock is held across the send: output cannot be handled before the request is known.
        let mut s = lock();
        if s.busy() {
            return;
        }
        let req = app.bus.send(json!({"op": "term_open", "sid": sid, "cols": cols, "rows": 12}));
        s.begin(req);
    }
    show(ui);
}

/// A line typed in the composer's shell mode: it goes to the shell (opened first if need be) and the output page is shown. False when
/// it cannot be run (no terminal in this run, no chat, too long), and the line stays in the composer.
pub fn run_line(ui: &AppWindow, app: &Arc<App>, line: &str) -> bool {
    // One line, plain text: a pasted newline would run several commands and a control character (Ctrl+D) could end the shell.
    if !enabled() || line.is_empty() || line.len() > MAX_LINE || line.chars().any(char::is_control) || app.current.lock().expect("current").sid.is_none() {
        return false;
    }
    let tid = lock().tid();
    match tid {
        Some(tid) => app.bus.post_quiet(json!({"op": "term_input", "tid": tid, "data": format!("{line}\n")})),
        None => {
            if !lock().busy() {
                open(ui, app);
            }
            lock().queue(line.to_owned());
        }
    }
    ui.set_new_open(false);
    ui.global::<Files>().set_term_open(true);
    ui.set_side(4);
    true
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let f = ui.global::<Files>();
    f.set_term_enabled(enabled());
    let (weak, a) = (ui.as_weak(), app.clone());
    f.on_term_set(move |on| {
        let Some(ui) = weak.upgrade() else { return };
        if on {
            if enabled() {
                open(&ui, &a);
            }
        } else {
            close(&a);
            show(&ui);
        }
    });
    let (weak, a) = (ui.as_weak(), app.clone());
    f.on_term_submit(move |line| {
        let Some(ui) = weak.upgrade() else { return };
        let tid = lock().tid();
        let Some(tid) = tid else { return };
        if line.len() > MAX_LINE {
            lock().status = tr(a.lang, "fs_term_long");
            show(&ui);
            return;
        }
        // The shell echoes what is typed, so the line is not added to the output here.
        a.bus.post_quiet(json!({"op": "term_input", "tid": tid, "data": format!("{line}\n")}));
    });
    let a = app.clone();
    f.on_term_interrupt(move || {
        let tid = lock().tid();
        if let Some(tid) = tid {
            a.bus.post_quiet(json!({"op": "term_input", "tid": tid, "data": "\u{3}"}));
        }
    });
}

/// `term_opened` and `term_error` replies.
pub fn on_reply(ui: &AppWindow, app: &Arc<App>, kind: &str, data: Value) {
    match kind {
        "term_opened" => {
            let Some(tid) = data["tid"].as_u64().and_then(|t| u32::try_from(t).ok()) else { return };
            let lang = app.lang;
            let accepted = lock().opened(data["req"].as_u64(), tid, &|code| fill(&tr(lang, "fs_term_ended"), &[("code", code.to_string())]));
            if !accepted {
                // Not the shell this drawer asked for: ended at once.
                app.bus.post_quiet(json!({"op": "term_close", "tid": tid}));
                return;
            }
            // A command typed in shell mode while the shell was being opened.
            let queued = lock().take_queued();
            if let Some(line) = queued {
                app.bus.post_quiet(json!({"op": "term_input", "tid": tid, "data": format!("{line}\n")}));
            }
        }
        _ => {
            // The host's own sentence; it holds no output of a shell. Only the answer to the open that is waited for counts.
            let message = data["message"].as_str().unwrap_or_default().chars().filter(|c| !c.is_control()).take(160).collect();
            lock().failed(data["req"].as_u64(), message);
        }
    }
    show(ui);
}

/// Reads the shell's output frames; the drawer is redrawn at most about 30 times a second however much comes.
pub async fn read_frames(app: Arc<App>, mut bin: Receiver<Vec<u8>>) {
    let lang = app.lang;
    let ended = move |code: i32| fill(&tr(lang, "fs_term_ended"), &[("code", code.to_string())]);
    while let Some(first) = bin.recv().await {
        let mut frames = vec![first];
        while let Ok(more) = bin.try_recv() {
            frames.push(more);
            if frames.len() >= 64 {
                break;
            }
        }
        let mut changed = false;
        {
            let mut s = lock();
            for frame in &frames {
                if let Some(parsed) = parse_frame(frame) {
                    changed |= s.frame(&parsed, &ended);
                }
            }
        }
        if changed {
            let _ = app.weak.upgrade_in_event_loop(|ui| show(&ui));
        }
        tokio::time::sleep(Duration::from_millis(33)).await;
    }
}
