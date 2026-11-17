//! Whole-window zoom (Ctrl+= / Ctrl+- / Ctrl+0 and the sidebar buttons). Slint lays out in logical pixels and draws them
//! times the window's scale factor, so zoom is the system scale times a percentage: fonts, paddings and fixed heights grow
//! together as in a browser, and app.slint needs no per-text factor.
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use slint::platform::WindowEvent;
use slint::ComponentHandle;
use tenrec::view::i18n::{tr, Lang};

use crate::AppWindow;

pub const STEPS: [u32; 9] = [80, 90, 100, 110, 120, 130, 140, 150, 160];

/// The next percentage: `dir` 0 is back to 100, otherwise one step smaller or larger, kept inside the steps.
pub fn step(pct: u32, dir: i32) -> u32 {
    let Some(i) = STEPS.iter().position(|s| *s == pct) else { return 100 };
    match dir {
        0 => 100,
        d if d < 0 => STEPS[i.saturating_sub(1)],
        _ => STEPS[(i + 1).min(STEPS.len() - 1)],
    }
}

/// The saved percentage; 100 when none is saved or it is not one of the steps.
pub fn saved(data: &Path) -> u32 {
    crate::desktop_settings(data).get("zoom").and_then(|v| v.as_u64()).and_then(|v| u32::try_from(v).ok()).filter(|v| STEPS.contains(v)).unwrap_or(100)
}

struct State {
    pct: Cell<u32>,
    /// The system's own scale factor for the window.
    base: Cell<f32>,
    /// The scale factor this module set last.
    applied: Cell<Option<f32>>,
    /// A failed apply is logged once, not at every tick that retries it.
    warned: Cell<bool>,
}

fn differs(a: Option<f32>, now: f32) -> bool {
    a.is_none_or(|a| (a - now).abs() > 1e-4)
}

fn apply(ui: &AppWindow, st: &State) {
    let window = ui.window();
    let now = window.scale_factor();
    // Anything but what was set last is the system's own scale: the one winit gives when it creates the window, or a new
    // monitor or display setting, either of which dropped the zoom.
    if differs(st.applied.get(), now) {
        st.base.set(now);
    }
    let size = window.size();
    // Minimised on Windows (zero size): a zero-sized surface fails; the next tick applies it.
    if size.width == 0 || size.height == 0 {
        return;
    }
    let scale = st.base.get() * st.pct.get() as f32 / 100.0;
    let follow = ui.get_at_bottom();
    // The scale first: Resized converts the logical size back with the window's current scale.
    let mut ok = window.dispatch_event_with_result(WindowEvent::ScaleFactorChanged { scale_factor: scale }).map(drop);
    // Same physical size, new logical size: the window stays where and as big as it is; the content is laid out again.
    if ok.is_ok() {
        ok = window.dispatch_event_with_result(WindowEvent::Resized { size: size.to_logical(scale) }).map(drop);
    }
    if let Err(error) = ok {
        if !st.warned.replace(true) {
            tracing::warn!("zoom not applied: {error}");
        }
    }
    st.applied.set(Some(scale));
    ui.set_zoom_pct(st.pct.get() as i32);
    if follow {
        // The viewport changed height: the end of the chat stays in view.
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(50), move || {
            if let Some(ui) = weak.upgrade() {
                ui.invoke_scroll_bottom();
            }
        });
    }
}

/// Wires the zoom and returns the timer that keeps it applied; the caller holds it for the window's life.
pub fn start(ui: &AppWindow, data: PathBuf, lang: Lang, pct: u32) -> slint::Timer {
    let st = Rc::new(State { pct: Cell::new(pct), base: Cell::new(ui.window().scale_factor()), applied: Cell::new(None), warned: Cell::new(false) });
    ui.set_zoom_pct(pct as i32);
    let (weak, s) = (ui.as_weak(), st.clone());
    ui.on_zoom(move |dir| {
        let Some(ui) = weak.upgrade() else { return };
        let next = step(s.pct.get(), dir);
        if next != s.pct.get() {
            s.pct.set(next);
            apply(&ui, &s);
            crate::remember(&data, "zoom", next.into());
        }
        // Shown at the ends too, so a key press that changes nothing still answers.
        crate::toast(&ui, format!("{} {next}%", tr(lang, "zoom")));
    });
    // The first apply waits for the window: winit sets the system scale when it creates it, after run() starts. The same
    // check puts the zoom back after the system changed the scale (another monitor, a new display setting).
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(250), move || {
        let Some(ui) = weak.upgrade() else { return };
        if differs(st.applied.get(), ui.window().scale_factor()) {
            apply(&ui, &st);
        }
    });
    timer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_stop_at_the_ends_and_odd_values_go_back_to_100() {
        assert_eq!(step(100, 1), 110);
        assert_eq!(step(100, -1), 90);
        assert_eq!(step(160, 1), 160);
        assert_eq!(step(80, -1), 80);
        assert_eq!(step(130, 0), 100);
        assert_eq!(step(105, 1), 100);
    }

    #[test]
    fn the_saved_zoom_keeps_the_folder_and_bad_files_start_over() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(saved(dir.path()), 100);
        crate::remember(dir.path(), "last_cwd", "/x".into());
        crate::remember(dir.path(), "zoom", 120.into());
        assert_eq!(saved(dir.path()), 120);
        assert_eq!(crate::last_folder(dir.path()), "/x");
        crate::remember(dir.path(), "zoom", 125.into());
        assert_eq!(saved(dir.path()), 100, "not a step");
        std::fs::write(dir.path().join("desktop.json"), "[1,2]").expect("write");
        crate::remember(dir.path(), "zoom", 90.into());
        assert_eq!(saved(dir.path()), 90);
        assert_eq!(crate::last_folder(dir.path()), "");
    }
}
