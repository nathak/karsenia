//! The Files tab: the open chat's project folder, one folder at a time, and a text file in an editor. Every operation names the
//! chat; the host decides the folder and what may be opened or saved. A file's text is never logged or put in a toast. A path is
//! only sent back as the host gave it or joined with a name the host listed or a single name typed in the window (no `/`; the host judges it).
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use serde_json::{json, Value};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tenrec::view::files::{crumbs, fmt_size, from_editor, gutter, is_under, join, line_col, listing_view, parent, readonly_reason, single_name, to_editor, MAX_OPEN};
use tenrec::server::project_fs::TRASH;
use tenrec::view::highlight::{highlight, view_of, View};
use tenrec::view::i18n::{fill, tr, Lang};
use tenrec::view::transcript::md_rows;

use crate::{App, AppWindow, Files, FsCrumb, FsRow, HlLine, HlTok};

struct Open {
    path: String,
    /// The version the text was loaded at, sent back with a save to find out whether the file changed meanwhile.
    base: String,
    eol: String,
    /// The note that belongs to the file itself (read-only reason, guard warning): put back when a "changed on disk" note is no longer true.
    note: String,
}

#[derive(Default)]
struct State {
    sid: Option<String>,
    /// The folder listed, as the host spelled it.
    dir: String,
    /// The number of the last listing or read sent: an answer to an older one is dropped.
    req: u64,
    /// The number of the save in flight, apart from `req`: a Refresh while a save waits must not make its answer unwelcome.
    save_req: u64,
    /// A save was sent and not answered yet: another is not sent over it (it would carry the same base and meet its own change).
    saving: bool,
    file: Option<Open>,
    /// The save waiting for an answer or a confirmation, kept to be sent again.
    pending: Option<Value>,
    /// The number of the disk check in flight (0: none), kept apart from `req` so a check never makes a listing or read unwelcome.
    check_req: u64,
    /// The file's base when the check was sent: an answer is judged against the base as it is on arrival (a save can finish between).
    check_base: String,
    /// The file is being read again because the check found it changed and nothing was typed.
    refresh: bool,
    /// The number and the text of the make, rename or delete in flight (apart from `req`, so a listing waiting for its answer stays
    /// welcome); the text is kept to be sent again with the person's confirmation.
    op_req: u64,
    op_pending: Option<Value>,
    /// A file to open once the folder listing that follows a change has arrived (a new file, a renamed open file).
    open_after: Option<String>,
}

thread_local! {
    static FS: RefCell<State> = RefCell::new(State::default());
}

/// An open file has edits that are not saved (the window asks before it closes).
pub fn has_unsaved(ui: &AppWindow) -> bool {
    dirty(ui)
}

fn dirty(ui: &AppWindow) -> bool {
    let f = ui.global::<Files>();
    !f.get_file().is_empty() && f.get_text() != f.get_orig()
}

/// Runs `then` now, or after the person agrees to drop unsaved changes.
fn leave_file(ui: &AppWindow, app: &Arc<App>, then: impl FnOnce(&AppWindow) + 'static) {
    if !dirty(ui) {
        then(ui);
        return;
    }
    let weak = ui.as_weak();
    crate::panels::ask(ui, tr(app.lang, "fs_dirty_discard"), tr(app.lang, "fs_discard"), true, Box::new(move |ok| {
        if let (true, Some(ui)) = (ok, weak.upgrade()) {
            then(&ui);
        }
    }));
}

fn close_file(ui: &AppWindow) {
    FS.with(|s| {
        let mut s = s.borrow_mut();
        s.file = None;
        s.pending = None;
        // The answer of a save for the file just left is not for the next one.
        s.save_req = 0;
        s.saving = false;
        s.check_req = 0;
        s.refresh = false;
        s.open_after = None;
    });
    let f = ui.global::<Files>();
    f.set_file(SharedString::new());
    f.set_text(SharedString::new());
    f.set_orig(SharedString::new());
    f.set_note(SharedString::new());
    clear_view(&f);
}

fn clear_view(f: &Files) {
    f.set_kind(SharedString::new());
    f.set_view(false);
    f.set_hl(ModelRc::default());
    f.set_hl_cols(0);
    f.set_md(ModelRc::default());
}

/// Makes what the viewer draws from the text now in the editor: coloured lines for source, rows for markdown. False when this file
/// has no viewer (or its text is too long for one): the editor stays, and the viewer's state is cleared.
fn rebuild_view(ui: &AppWindow, lang: Lang) -> bool {
    let f = ui.global::<Files>();
    let (path, text) = (f.get_file().to_string(), f.get_text().to_string());
    match view_of(&path) {
        View::Code(code) => match highlight(code, &text) {
            Some((lines, cols)) => {
                let lines: Vec<HlLine> = lines
                    .into_iter()
                    .map(|l| HlLine { n: l.n as i32, toks: ModelRc::from(Rc::new(VecModel::from(l.toks.into_iter().map(|t| HlTok { text: t.text.into(), kind: t.kind.code() }).collect::<Vec<_>>()))) })
                    .collect();
                f.set_hl(ModelRc::from(Rc::new(VecModel::from(lines))));
                f.set_hl_cols(cols as i32);
                f.set_md(ModelRc::default());
                f.set_kind("code".into());
                true
            }
            None => {
                // Too long to colour: the button stays (a click says why), so a text cut down again can still be viewed.
                f.set_hl(ModelRc::default());
                f.set_hl_cols(0);
                f.set_md(ModelRc::default());
                f.set_kind("code".into());
                false
            }
        },
        View::Markdown => {
            let rows: Vec<_> = md_rows(&text, lang).iter().map(crate::to_slint).collect();
            f.set_md(ModelRc::from(Rc::new(VecModel::from(rows))));
            f.set_hl(ModelRc::default());
            f.set_hl_cols(0);
            f.set_kind("md".into());
            true
        }
        View::Plain => {
            clear_view(&f);
            false
        }
    }
}

fn send(app: &App, mut op: Value) {
    let sid = FS.with(|s| s.borrow().sid.clone());
    let Some(sid) = sid else { return };
    let is_save = op["op"] == "fs_write";
    op["sid"] = json!(sid);
    let req = app.bus.send(op);
    FS.with(|s| {
        let mut s = s.borrow_mut();
        // Anything the person asked for replaces a re-read waiting for its answer.
        s.refresh = false;
        if is_save {
            s.save_req = req;
            s.saving = true;
        } else {
            s.req = req;
        }
    });
}

/// Runs `then` now, or after the person agrees to drop unsaved edits of the open file: used before the chat on screen changes.
pub fn guard_dirty(ui: &AppWindow, app: &Arc<App>, then: impl FnOnce(&AppWindow) + 'static) {
    leave_file(ui, app, then);
}

fn list(app: &App, dir: &str) {
    send(app, json!({"op": "fs_list", "path": dir}));
}

/// A check made when a question was asked and again when it is answered: the same chat and the same open file (a question that
/// outlived its chat or its file must not send a save to the one now on screen).
fn same_file(op: &Value) -> impl Fn() -> bool {
    let sid = FS.with(|s| s.borrow().sid.clone());
    let path = op["path"].as_str().unwrap_or_default().to_owned();
    move || FS.with(|s| {
        let s = s.borrow();
        s.sid == sid && s.file.as_ref().is_some_and(|f| f.path == path)
    })
}

/// The chat on screen changed: nothing of the old one stays.
pub fn reset(ui: &AppWindow) {
    close_file(ui);
    FS.with(|s| *s.borrow_mut() = State::default());
    let f = ui.global::<Files>();
    f.set_rows(ModelRc::default());
    f.set_crumbs(ModelRc::default());
    f.set_notice(SharedString::new());
    f.set_has_chat(false);
}

/// The Files tab was opened: the chat's folder is listed (again, when the chat is another one).
pub fn panel_open(ui: &AppWindow, app: &App) {
    let sid = app.current.lock().expect("current").sid.clone();
    let f = ui.global::<Files>();
    f.set_has_chat(sid.is_some());
    let Some(sid) = sid else { return };
    let same = FS.with(|s| s.borrow().sid.as_deref() == Some(sid.as_str()));
    if !same {
        close_file(ui);
        FS.with(|s| *s.borrow_mut() = State { sid: Some(sid), ..State::default() });
        list(app, "");
    } else {
        check_disk(ui, app);
    }
}

/// Looks at the open file on disk (a turn ended, the tab was shown again). Nothing is shown when it is unchanged; see `on_check`.
pub fn check_disk(ui: &AppWindow, app: &App) {
    let current = app.current.lock().expect("current").sid.clone();
    let go = FS.with(|s| {
        let s = s.borrow();
        let file = s.file.as_ref()?;
        (!s.saving && !s.refresh && s.sid.is_some() && s.sid == current).then(|| (s.sid.clone().unwrap_or_default(), file.path.clone(), file.base.clone()))
    });
    let Some((sid, path, base)) = go else { return };
    if ui.get_side() != 4 {
        return;
    }
    // Not through `send`: that would take the number of the listing or read the window waits for.
    let req = app.bus.send(json!({"op": "fs_check", "sid": sid, "path": path}));
    FS.with(|s| {
        let mut s = s.borrow_mut();
        s.check_req = req;
        s.check_base = base;
    });
}

/// The answer to a disk check. Unchanged: the file's own note. Changed and nothing typed: the file is read again (`fs_text` applies it
/// unless something was typed meanwhile). Changed and text typed: a note only; saving then asks what to do (`fs_conflict`).
fn on_check(ui: &AppWindow, app: &Arc<App>, kind: &str, data: &Value) {
    let lang = app.lang;
    let (file, check_base, saving) = FS.with(|s| {
        let mut s = s.borrow_mut();
        s.check_req = 0;
        (s.file.as_ref().map(|f| (f.path.clone(), f.base.clone(), f.note.clone())), s.check_base.clone(), s.saving)
    });
    let Some((path, base, note)) = file else { return };
    if saving || base != check_base || data["path"].as_str() != Some(path.as_str()) {
        return;
    }
    let f = ui.global::<Files>();
    if kind == "fs_error" {
        // No toast: this runs at every turn end. The sentence is the host's own (it holds no file text).
        if let Some(message) = data["message"].as_str().filter(|m| !m.is_empty()) {
            f.set_note(message.chars().filter(|c| !c.is_control()).take(160).collect::<String>().into());
        }
        return;
    }
    if data["version"].as_str() == Some(base.as_str()) {
        f.set_note(note.into());
    } else if dirty(ui) {
        f.set_note(tr(lang, "fs_disk_changed").into());
    } else {
        send(app, json!({"op": "fs_read", "path": path}));
        FS.with(|s| s.borrow_mut().refresh = true);
    }
}

pub fn on_reply(ui: &AppWindow, app: &Arc<App>, kind: &str, data: Value) {
    let lang = app.lang;
    let asked = data["req"].as_u64();
    if asked.is_some() && asked == Some(FS.with(|s| s.borrow().check_req)) {
        on_check(ui, app, kind, &data);
        return;
    }
    if asked.is_some() && asked == Some(FS.with(|s| s.borrow().op_req)) {
        on_op(ui, app, kind, &data);
        return;
    }
    // An answer is for the listing or read asked last, or for the save in flight; anything else is dropped.
    let (current, save) = FS.with(|s| (s.borrow().req, s.borrow().save_req));
    if asked != Some(current) && asked != Some(save) {
        return;
    }
    if asked == Some(save) {
        FS.with(|s| s.borrow_mut().saving = false);
    }
    let f = ui.global::<Files>();
    match kind {
        "fs_listing" => {
            let dir = data["path"].as_str().unwrap_or_default().to_owned();
            let (rows, notice) = listing_view(&data, &dir, lang);
            FS.with(|s| s.borrow_mut().dir = dir.clone());
            f.set_notice(notice.into());
            let rows: Vec<FsRow> = rows.into_iter().map(|r| FsRow { name: r.name.into(), path: r.path.into(), kind: r.kind.into(), size: r.size.into(), tags: r.tags.into(), blocked: r.blocked, from: r.from.into(), place: r.place.into() }).collect();
            f.set_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
            let crumbs: Vec<FsCrumb> = crumbs(&dir).into_iter().map(|(label, path)| FsCrumb { label: label.into(), path: path.into() }).collect();
            f.set_crumbs(ModelRc::from(Rc::new(VecModel::from(crumbs))));
            // After a change: the file that was made, or the open file under its new name.
            let next = FS.with(|s| s.borrow_mut().open_after.take());
            if let Some(path) = next {
                send(app, json!({"op": "fs_read", "path": path}));
            }
        }
        "fs_text" => {
            let text = data["text"].as_str().unwrap_or_default();
            if text.len() > MAX_OPEN {
                close_file(ui);
                f.set_note(tr(lang, "fs_too_large").into());
                return;
            }
            let refreshing = FS.with(|s| std::mem::take(&mut s.borrow_mut().refresh));
            if refreshing && dirty(ui) {
                // Typed while the answer was on its way: the text stays, and the note says why the disk and the editor differ.
                f.set_note(tr(lang, "fs_disk_changed").into());
                return;
            }
            let eol = data["eol"].as_str().unwrap_or("lf").to_owned();
            let shown = to_editor(text, &eol);
            let reason = readonly_reason(&eol, data["guard_kind"].as_str().unwrap_or_default());
            let guarded = data["guard"].as_str().is_some() && reason.is_empty();
            let note = if !reason.is_empty() { tr(lang, reason) } else if guarded { tr(lang, "fs_guarded_note") } else { String::new() };
            FS.with(|s| s.borrow_mut().file = Some(Open { path: data["path"].as_str().unwrap_or_default().to_owned(), base: data["version"].as_str().unwrap_or_default().to_owned(), eol, note: note.clone() }));
            f.set_file(data["path"].as_str().unwrap_or_default().into());
            f.set_orig(shown.as_str().into());
            f.set_gutter(gutter(&shown).into());
            f.set_ln(1);
            f.set_col(1);
            f.set_text(shown.into());
            f.set_readonly(!reason.is_empty());
            f.set_note(note.into());
            // A file is opened in its viewer; a file read again stays as it was shown.
            let was_view = f.get_view();
            let made = rebuild_view(ui, lang);
            f.set_view(made && (!refreshing || was_view));
            if refreshing {
                crate::toast(ui, tr(lang, "fs_disk_reloaded"));
            }
        }
        "fs_saved" => {
            // What was saved is what the save carried, not what is on screen now (more may have been typed meanwhile).
            let (saved, eol, note) = FS.with(|s| {
                let mut s = s.borrow_mut();
                let saved = s.pending.take().and_then(|op| op["text"].as_str().map(str::to_owned));
                if let Some(file) = s.file.as_mut() {
                    file.base = data["version"].as_str().unwrap_or_default().to_owned();
                }
                (saved, s.file.as_ref().map(|o| o.eol.clone()).unwrap_or_default(), s.file.as_ref().map(|o| o.note.clone()).unwrap_or_default())
            });
            f.set_note(note.into());
            if let Some(saved) = saved {
                f.set_orig(to_editor(&saved, &eol).into());
            }
            crate::toast(ui, tr(lang, "fs_saved"));
        }
        "fs_needs_confirm" => {
            let Some(op) = FS.with(|s| s.borrow().pending.clone()) else { return };
            let a = app.clone();
            let same = same_file(&op);
            crate::panels::ask(ui, tr(lang, "fs_confirm_body"), tr(lang, "fs_save"), false, Box::new(move |ok| {
                if ok && same() {
                    let mut op = op;
                    op["confirm"] = json!(true);
                    FS.with(|s| s.borrow_mut().pending = Some(op.clone()));
                    send(&a, op);
                }
            }));
        }
        "fs_conflict" => {
            let Some(op) = FS.with(|s| s.borrow().pending.clone()) else { return };
            let version = data["version"].as_str().unwrap_or_default().to_owned();
            let a = app.clone();
            let same = same_file(&op);
            crate::panels::ask(ui, tr(lang, "fs_conflict_body"), tr(lang, "fs_overwrite"), true, Box::new(move |ok| {
                if ok && same() {
                    // The file as it is now is the base of this save: the person has seen the question and chose to overwrite.
                    let mut op = op;
                    op["base"] = json!(version);
                    FS.with(|s| s.borrow_mut().pending = Some(op.clone()));
                    send(&a, op);
                }
            }));
        }
        "fs_error" => {
            // A listing or read that failed: the file that was to be opened after a change is not opened later by another listing.
            FS.with(|s| s.borrow_mut().open_after = None);
            // A re-read that failed (the file became binary or too big): the flag must not stay on, or no check would run again.
            let refreshing = FS.with(|s| std::mem::take(&mut s.borrow_mut().refresh));
            if refreshing {
                if let Some(message) = data["message"].as_str().filter(|m| !m.is_empty()) {
                    f.set_note(message.chars().filter(|c| !c.is_control()).take(160).collect::<String>().into());
                }
                return;
            }
            if let Some(message) = data["message"].as_str().filter(|m| !m.is_empty()) {
                f.set_note(SharedString::new());
                // On a file that cannot be opened (Windows has no editor) the sentence goes where the file would be.
                if f.get_file().is_empty() {
                    f.set_note(message.into());
                } else {
                    crate::toast(ui, message.to_owned());
                }
            }
        }
        _ => {}
    }
}

/// A make, rename or delete is waiting for its answer: another is not sent over it (its answer would be dropped, and the open
/// file and the listing would not follow what the host already did).
fn op_busy() -> bool {
    FS.with(|s| s.borrow().op_req != 0)
}

/// Sends a make, rename or delete. Not through `send`: that would take the number of the listing or read the window waits for.
fn send_op(app: &App, mut op: Value) {
    let sid = FS.with(|s| s.borrow().sid.clone());
    let Some(sid) = sid else { return };
    op["sid"] = json!(sid);
    let req = app.bus.send(op.clone());
    FS.with(|s| {
        let mut s = s.borrow_mut();
        s.op_req = req;
        s.op_pending = Some(op);
    });
}

/// The answer to a make, rename or delete: the folder is listed again, and the open file follows what happened to it.
fn on_op(ui: &AppWindow, app: &Arc<App>, kind: &str, data: &Value) {
    let lang = app.lang;
    let (op, dir) = FS.with(|s| {
        let mut s = s.borrow_mut();
        s.op_req = 0;
        (s.op_pending.take(), s.dir.clone())
    });
    let Some(op) = op else { return };
    let path = op["path"].as_str().unwrap_or_default().to_owned();
    let f = ui.global::<Files>();
    match kind {
        "fs_needs_confirm" => {
            // A place a later chat reads or runs: the same request goes again only after the person agrees.
            let (a, sid) = (app.clone(), op["sid"].as_str().unwrap_or_default().to_owned());
            // Where it would go, when the host says (a restore) or the request names it (a move).
            let to = data["to"].as_str().or(op["to"].as_str()).unwrap_or_default().chars().filter(|c| !c.is_control()).collect::<String>();
            let body = if to.is_empty() { tr(lang, "fs_change_confirm_body") } else { format!("{}\n{path} → {to}", tr(lang, "fs_change_confirm_body")) };
            crate::panels::ask(ui, body, tr(lang, "fs_change_confirm"), false, Box::new(move |ok| {
                if ok && !op_busy() && FS.with(|s| s.borrow().sid.as_deref() == Some(sid.as_str())) {
                    let mut op = op;
                    op["confirm"] = json!(true);
                    send_op(&a, op);
                }
            }));
        }
        "fs_created" => {
            crate::toast(ui, tr(lang, "fs_created"));
            f.set_creating(SharedString::new());
            if op["kind"] == "file" {
                FS.with(|s| s.borrow_mut().open_after = Some(path));
            }
            list(app, &dir);
        }
        "fs_purge_summary" => {
            // The host counted what a delete for good would reach; the same request goes again only after the person agrees.
            let emptying = op["op"] == "fs_empty_trash";
            let size = fmt_size(data["bytes"].as_u64().unwrap_or(0));
            let text = fill(&tr(lang, if emptying { "fs_empty_body" } else { "fs_purge_body" }), &[("path", path.clone()), ("entries", data["entries"].as_u64().unwrap_or(0).to_string()), ("items", data["items"].as_u64().unwrap_or(0).to_string()), ("size", size)]);
            let text = match data["left"].as_u64().filter(|l| *l > 0) {
                Some(left) => format!("{text}\n{}", fill(&tr(lang, "fs_purge_left"), &[("left", left.to_string())])),
                None => text,
            };
            let (a, sid, weak, open_path) = (app.clone(), op["sid"].as_str().unwrap_or_default().to_owned(), ui.as_weak(), path.clone());
            crate::panels::ask(ui, text, tr(lang, if emptying { "fs_empty_trash" } else { "fs_purge" }), true, Box::new(move |ok| {
                let (true, Some(ui)) = (ok, weak.upgrade()) else { return };
                if !FS.with(|s| s.borrow().sid.as_deref() == Some(sid.as_str())) {
                    return;
                }
                if op_busy() {
                    crate::toast(&ui, tr(a.lang, "fs_busy"));
                    return;
                }
                let mut op = op;
                op["confirm"] = json!(true);
                let b = a.clone();
                let go = move |_: &AppWindow| {
                    if !op_busy() {
                        send_op(&b, op);
                    }
                };
                // An open file inside what is deleted: unsaved edits are asked about first (and a cancel there deletes nothing).
                if FS.with(|s| s.borrow().file.as_ref().is_some_and(|o| is_under(&o.path, &open_path))) {
                    leave_file(&ui, &a, go);
                } else {
                    go(&ui);
                }
            }));
        }
        "fs_purged" => {
            let left = data["left"].as_u64().unwrap_or(0);
            let removed = data["removed"].as_u64().unwrap_or(0);
            let text = if left > 0 { fill(&tr(lang, "fs_purged_left"), &[("removed", removed.to_string()), ("left", left.to_string())]) } else { tr(lang, "fs_purged") };
            crate::toast(ui, text);
            f.set_edit_path(SharedString::new());
            if FS.with(|s| s.borrow().file.as_ref().is_some_and(|o| is_under(&o.path, &path))) {
                close_file(ui);
            }
            list(app, &dir);
        }
        "fs_restored" => {
            crate::toast(ui, tr(lang, "fs_restored"));
            f.set_edit_path(SharedString::new());
            list(app, &dir);
        }
        "fs_renamed" => {
            crate::toast(ui, tr(lang, "fs_renamed"));
            f.set_edit_path(SharedString::new());
            let to = op["to"].as_str().unwrap_or_default().to_owned();
            // The open file (or one inside a renamed folder) is read again under its new name: its guard may differ.
            let open = FS.with(|s| s.borrow().file.as_ref().map(|o| o.path.clone())).filter(|open| is_under(open, &path));
            if let Some(open) = open {
                close_file(ui);
                FS.with(|s| s.borrow_mut().open_after = Some(format!("{to}{}", &open[path.len()..])));
            }
            list(app, &dir);
        }
        "fs_deleted" => {
            f.set_edit_path(SharedString::new());
            match data["trashed"].as_str() {
                // Into the trash: the toast offers to take it back.
                Some(trashed) => {
                    let (a, sid, trashed) = (app.clone(), op["sid"].as_str().unwrap_or_default().to_owned(), trashed.to_owned());
                    let undo = Box::new(move || {
                        if !FS.with(|s| s.borrow().sid.as_deref() == Some(sid.as_str())) {
                            return;
                        }
                        // Another change is still waiting for its answer: said, not silently dropped.
                        if op_busy() {
                            if let Some(ui) = a.weak.upgrade() {
                                crate::toast(&ui, tr(a.lang, "fs_busy"));
                            }
                            return;
                        }
                        send_op(&a, json!({"op": "fs_restore", "path": trashed}));
                    });
                    crate::panels::toast_with(ui, tr(lang, "fs_trashed"), Some((tr(lang, "fs_undo"), undo)), 10_000);
                }
                None => crate::toast(ui, tr(lang, "fs_deleted")),
            }
            if FS.with(|s| s.borrow().file.as_ref().is_some_and(|o| is_under(&o.path, &path))) {
                close_file(ui);
            }
            list(app, &dir);
        }
        _ => {
            if let Some(message) = data["message"].as_str().filter(|m| !m.is_empty()) {
                crate::toast(ui, message.chars().filter(|c| !c.is_control()).take(160).collect());
            }
        }
    }
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let files = ui.global::<Files>();
    let (a, weak) = (app.clone(), ui.as_weak());
    files.on_create(move |kind, name| {
        let Some(ui) = weak.upgrade() else { return };
        if !single_name(&name) {
            crate::toast(&ui, tr(a.lang, "fs_name_slash"));
            return;
        }
        if op_busy() {
            return;
        }
        let dir = FS.with(|s| s.borrow().dir.clone());
        let op = json!({"op": "fs_create", "path": join(&dir, &name), "kind": kind.to_string()});
        let b = a.clone();
        // A new file is opened when it is made: unsaved edits of the file on screen are asked about first.
        if kind == "file" && dirty(&ui) {
            leave_file(&ui, &a, move |ui| {
                close_file(ui);
                if !op_busy() {
                    send_op(&b, op);
                }
            });
        } else {
            send_op(&a, op);
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    // The text is a whole path: the same folder renames, another existing folder of the project moves (the host judges the path).
    files.on_rename(move |path, to| {
        let Some(ui) = weak.upgrade() else { return };
        let (path, to) = (path.to_string(), to.to_string());
        if op_busy() {
            return;
        }
        if to == path {
            ui.global::<Files>().set_edit_path(SharedString::new());
            return;
        }
        // An open file under the entry must not carry unsaved edits across the rename.
        let holds = FS.with(|s| s.borrow().file.as_ref().is_some_and(|o| is_under(&o.path, &path)));
        let b = a.clone();
        // Checked again here: the question about unsaved edits may have been open while another change went out.
        let go = move |_: &AppWindow| {
            if !op_busy() {
                send_op(&b, json!({"op": "fs_rename", "path": path, "to": to}));
            }
        };
        if holds {
            leave_file(&ui, &a, go);
        } else {
            go(&ui);
        }
    });
    let a = app.clone();
    files.on_purge(move |path| {
        if !op_busy() {
            send_op(&a, json!({"op": "fs_purge", "path": path.to_string()}));
        }
    });
    let a = app.clone();
    files.on_empty_trash(move || {
        if !op_busy() {
            send_op(&a, json!({"op": "fs_empty_trash", "path": TRASH}));
        }
    });
    let a = app.clone();
    files.on_restore(move |path| {
        if !op_busy() {
            send_op(&a, json!({"op": "fs_restore", "path": path.to_string()}));
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    files.on_delete(move |path, kind| {
        let Some(ui) = weak.upgrade() else { return };
        let path = path.to_string();
        if op_busy() {
            return;
        }
        // Into the trash: a question that says so. Inside the trash a delete is for good.
        let in_trash = is_under(&path, TRASH) && path != TRASH;
        let key = if !in_trash { "fs_trash_ask" } else if kind == "dir" { "fs_delete_dir_ask" } else { "fs_purge_ask" };
        let text = fill(&tr(a.lang, key), &[("path", path.clone())]);
        let (b, weak) = (a.clone(), ui.as_weak());
        crate::panels::ask(&ui, text, tr(a.lang, "fs_delete"), true, Box::new(move |ok| {
            let (true, Some(ui)) = (ok, weak.upgrade()) else { return };
            // A file that is open is deleted only at the version the window holds, and unsaved edits are asked about first.
            let open = FS.with(|s| s.borrow().file.as_ref().map(|o| (o.path.clone(), o.base.clone())));
            let base = open.as_ref().filter(|(open, _)| *open == path).map(|(_, base)| base.clone());
            let mut op = json!({"op": "fs_delete", "path": path});
            if let Some(base) = base {
                op["base"] = json!(base);
            }
            let c = b.clone();
            let go = move |_: &AppWindow| {
                if !op_busy() {
                    send_op(&c, op);
                }
            };
            if open.is_some_and(|(open, _)| is_under(&open, &path)) {
                leave_file(&ui, &b, go);
            } else {
                go(&ui);
            }
        }));
    });
    let weak = ui.as_weak();
    files.on_edited(move |text| {
        if let Some(ui) = weak.upgrade() {
            ui.global::<Files>().set_gutter(gutter(&text).into());
        }
    });
    let weak = ui.as_weak();
    files.on_cursor(move |offset| {
        if let Some(ui) = weak.upgrade() {
            let f = ui.global::<Files>();
            let (line, col) = line_col(&f.get_text(), offset.max(0) as usize);
            f.set_ln(line as i32);
            f.set_col(col as i32);
        }
    });
    let weak = ui.as_weak();
    files.on_back(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_side(0);
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    files.on_open_dir(move |path| {
        let Some(ui) = weak.upgrade() else { return };
        let (b, path) = (a.clone(), path.to_string());
        leave_file(&ui, &a, move |ui| {
            close_file(ui);
            list(&b, &path);
        });
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    files.on_open_file(move |path| {
        let Some(ui) = weak.upgrade() else { return };
        let (b, path) = (a.clone(), path.to_string());
        leave_file(&ui, &a, move |ui| {
            close_file(ui);
            send(&b, json!({"op": "fs_read", "path": path}));
        });
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    files.on_up(move || {
        let Some(ui) = weak.upgrade() else { return };
        let b = a.clone();
        let dir = parent(&FS.with(|s| s.borrow().dir.clone()));
        leave_file(&ui, &a, move |ui| {
            close_file(ui);
            list(&b, &dir);
        });
    });
    let (weak, lang) = (ui.as_weak(), app.lang);
    files.on_mention(move |path| {
        let Some(ui) = weak.upgrade() else { return };
        let path = path.to_string();
        // Claude Code reads `@path` up to the next space, so only a plain path is put in.
        if path.is_empty() || path.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\'') {
            crate::toast(&ui, tr(lang, "fs_mention_space"));
            return;
        }
        let draft = ui.get_draft();
        let sep = if draft.is_empty() || draft.ends_with(char::is_whitespace) { "" } else { " " };
        ui.set_draft(format!("{draft}{sep}@{path} ").into());
        ui.set_side(0);
    });
    let a = app.clone();
    files.on_refresh(move || {
        FS.with(|s| s.borrow_mut().open_after = None);
        list(&a, &FS.with(|s| s.borrow().dir.clone()));
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    files.on_save(move || {
        let Some(ui) = weak.upgrade() else { return };
        let f = ui.global::<Files>();
        if FS.with(|s| s.borrow().saving) {
            return;
        }
        let Some((path, base, eol)) = FS.with(|s| s.borrow().file.as_ref().map(|o| (o.path.clone(), o.base.clone(), o.eol.clone()))) else { return };
        if f.get_readonly() || f.get_text() == f.get_orig() {
            return;
        }
        let op = json!({"op": "fs_write", "path": path, "text": from_editor(&f.get_text(), &eol), "base": base});
        FS.with(|s| s.borrow_mut().pending = Some(op.clone()));
        send(&a, op);
    });
    let (weak, a) = (ui.as_weak(), app.clone());
    files.on_set_view(move |on| {
        let Some(ui) = weak.upgrade() else { return };
        // The viewer is made from the text as it is now (what is typed and not yet saved shows too).
        let shown = on && rebuild_view(&ui, a.lang);
        let f = ui.global::<Files>();
        f.set_view(shown);
        if on && !shown {
            crate::toast(&ui, tr(a.lang, "fs_view_too_long"));
        }
        if !on {
            // The editor is made anew: its cursor is at the start.
            f.set_ln(1);
            f.set_col(1);
        }
    });
    let a = app.clone();
    files.on_copy(move |text| crate::copy_text(&a, text.to_string()));
    let a = app.clone();
    files.on_link(move |url| crate::copy_text(&a, url.to_string()));
    let (a, weak) = (app.clone(), ui.as_weak());
    files.on_close(move || {
        let Some(ui) = weak.upgrade() else { return };
        let _ = &a;
        leave_file(&ui, &a, close_file);
    });
}
