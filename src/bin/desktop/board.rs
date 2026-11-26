//! The board page's glue: while the page is on screen, the host is asked for the lines (see `tenrec::board`), they are kept, and the
//! page is drawn from them filtered and sorted (see `tenrec::view::board`). Nothing here sends anything to a chat; a click only goes to it.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tenrec::board::Row;
use tenrec::view::board::{page, Filter, Sort};

use crate::{App, AppWindow, Board, BoardLine};

/// The page is on screen (the window says so; see `board-shown` in app.slint): the host is asked for lines only then.
static VISIBLE: AtomicBool = AtomicBool::new(false);
/// Bumped whenever the page is shown or hidden: an answer that comes after the page went away is dropped.
static GEN: AtomicU64 = AtomicU64::new(0);
/// A request is on its way (the next tick does not start another).
static BUSY: AtomicBool = AtomicBool::new(false);
/// When the host was asked last (ms), so that a burst of events does not ask every time.
static LAST: AtomicU64 = AtomicU64::new(0);
/// The rows of the last answer: what a change of filter or order draws again without asking.
static ROWS: Mutex<Vec<Row>> = Mutex::new(Vec::new());

/// Oftenest the host is asked.
const EVERY_MS: u64 = 1000;

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let board = ui.global::<Board>();
    board.set_filters(ModelRc::from(Rc::new(VecModel::from(Filter::names(app.lang).into_iter().map(SharedString::from).collect::<Vec<_>>()))));
    board.set_sorts(ModelRc::from(Rc::new(VecModel::from(Sort::names(app.lang).into_iter().map(SharedString::from).collect::<Vec<_>>()))));
    let a = app.clone();
    board.on_visible(move |on| {
        VISIBLE.store(on, Ordering::Release);
        GEN.fetch_add(1, Ordering::AcqRel);
        if on {
            // At once, not at the next tick.
            LAST.store(0, Ordering::Relaxed);
            start(&a, false);
        }
    });
    let (weak, a) = (ui.as_weak(), app.clone());
    board.on_view_changed(move || {
        if let Some(ui) = weak.upgrade() {
            render(&ui, &a);
        }
    });
    let a = app.clone();
    board.on_refresh(move || start(&a, true));
}

/// Called by the chat list's loop on every round: asks the host when the page is on screen and the last ask is not too recent.
pub fn tick(app: &Arc<App>) {
    if VISIBLE.load(Ordering::Acquire) {
        start(app, false);
    }
}

fn start(app: &Arc<App>, force: bool) {
    let now = tenrec::now_ms();
    if !force && now.saturating_sub(LAST.load(Ordering::Relaxed)) < EVERY_MS {
        return;
    }
    if BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    let (a, generation) = (app.clone(), GEN.load(Ordering::Acquire));
    tokio::spawn(async move {
        let rows = a.registry.board(force).await;
        BUSY.store(false, Ordering::Release);
        // The page went away (or came back) while the host was answering: this answer is old.
        if !VISIBLE.load(Ordering::Acquire) || GEN.load(Ordering::Acquire) != generation {
            return;
        }
        *ROWS.lock().unwrap_or_else(|p| p.into_inner()) = rows;
        let b = a.clone();
        let _ = a.weak.upgrade_in_event_loop(move |ui| render(&ui, &b));
    });
}

/// Draws the page from the rows of the last answer, as the filter and the order say.
fn render(ui: &AppWindow, app: &Arc<App>) {
    let board = ui.global::<Board>();
    let rows = ROWS.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let shown = page(&rows, Filter::from_index(board.get_filter()), Sort::from_index(board.get_sort()), tenrec::now_ms(), app.lang);
    let lines: Vec<BoardLine> = shown
        .lines
        .into_iter()
        .map(|l| BoardLine {
            sid: l.sid.into(),
            title: l.title.into(),
            project: l.project.into(),
            chip: l.chip.into(),
            state_tone: l.state_tone.into(),
            state_text: l.state_text.into(),
            progress: l.progress.into(),
            notes: l.notes.into(),
            change: l.change.into(),
            dim: l.dim,
            merge: l.merge.into(),
            merge_tone: l.merge_tone.into(),
            cost: l.cost.into(),
            ask: l.ask,
            worktree: l.worktree,
            spoken: l.spoken.into(),
        })
        .collect();
    board.set_lines(ModelRc::from(Rc::new(VecModel::from(lines))));
    board.set_summary(shown.summary.into());
    board.set_total(i32::try_from(shown.total).unwrap_or(i32::MAX));
}

/// What the page should show before the first answer comes (a page opened again shows the old lines for a moment, then the new ones).
#[allow(dead_code)]
pub fn is_visible() -> bool {
    VISIBLE.load(Ordering::Acquire)
}

use std::rc::Rc;
