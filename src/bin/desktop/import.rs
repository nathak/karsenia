//! Bringing in a session of the `claude` command line program: the list of what an account's folder holds, and the import of the one
//! picked. The scan and the reading of a session file run off the UI thread; what a session says is never logged.
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use slint::{ComponentHandle, ModelRc, VecModel};
use tenrec::cli_sessions::{list, read_history, CliSession, History};
use tenrec::view::i18n::tr;
use tenrec::view::import::rows;

use crate::{accounts, files, panels, App, AppWindow, Imp, ImpRow};

/// How many sessions the list shows (the newest ones).
const LIMIT: usize = 300;
const MAX_EVENTS: usize = 400;
const MAX_BYTES: usize = 4 << 20;

#[derive(Default)]
struct State {
    sessions: Vec<CliSession>,
    /// The ids already imported (or made by this app) and the folders that are still there, as of the last scan.
    imported: HashSet<String>,
    existing: HashSet<String>,
    /// Of those, the folders a session can be imported into.
    fit: HashSet<String>,
    /// The account the list is of (None: the default one).
    account: Option<String>,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State::default());
    /// The number of the last scan: an older one's answer is dropped.
    static GEN: Cell<u64> = const { Cell::new(0) };
}

/// An import is going on (a pick while one is does nothing). Shared, because it is cleared from tasks off the UI thread.
static IMPORTING: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The rows for what the filter box says now.
fn render(ui: &AppWindow, app: &App) {
    let imp = ui.global::<Imp>();
    let filter = imp.get_filter().to_string();
    let shown: Vec<ImpRow> = ST.with(|s| {
        let s = s.borrow();
        rows(&s.sessions, &s.imported, &|cwd| s.existing.contains(cwd), &|cwd| s.fit.contains(cwd), &filter, now_ms(), app.lang)
            .into_iter()
            .map(|r| ImpRow { id: r.id.into(), title: r.title.into(), folder: r.folder.into(), branch: r.branch.into(), when: r.when.into(), blocked: r.blocked.into(), blocked_text: r.blocked_text.into(), recent: r.recent })
            .collect()
    });
    imp.set_rows(ModelRc::from(Rc::new(VecModel::from(shown))));
}

/// The folder an account's `claude` keeps its sessions in.
fn config_dir(app: &App, account: Option<&str>) -> Result<PathBuf, String> {
    match account {
        Some(id) => app.registry.account_login(id).map(|(dir, _)| dir),
        None => {
            let home = tenrec::platform::home_dir();
            tenrec::accounts::default_config_dir(std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).as_deref(), home.as_deref()).ok_or_else(|| "no home folder".to_owned())
        }
    }
}

fn scan(app: &Arc<App>, index: i32, name: &str) {
    let Ok(account) = accounts::chosen(index, name) else {
        crate::toast_later(&app.weak, tr(app.lang, "acct_changed_retry"));
        return;
    };
    let n = GEN.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    if let Some(ui) = app.weak.upgrade() {
        let imp = ui.global::<Imp>();
        imp.set_busy(true);
        imp.set_note(slint::SharedString::new());
    }
    let a = app.clone();
    tokio::spawn(async move {
        let dir = config_dir(&a, account.as_deref());
        let imported: HashSet<String> = a.registry.list().await.into_iter().flat_map(|i| [i.meta.claude_session_id, i.meta.imported_from]).flatten().collect();
        let found = match dir {
            Ok(dir) => tokio::task::spawn_blocking(move || {
                let sessions = list(&dir, LIMIT);
                let existing: HashSet<String> = sessions.iter().map(|s| s.cwd.clone()).collect::<HashSet<_>>().into_iter().filter(|c| Path::new(c).is_dir()).collect();
                let fit: HashSet<String> = existing.iter().filter(|c| tenrec::registry::import_folder(c).is_ok()).cloned().collect();
                (sessions, existing, fit)
            })
            .await
            .map_err(|e| e.to_string()),
            Err(e) => Err(e),
        };
        let _ = a.weak.clone().upgrade_in_event_loop(move |ui| {
            if GEN.with(Cell::get) != n {
                return;
            }
            let imp = ui.global::<Imp>();
            imp.set_busy(false);
            match found {
                Ok((sessions, existing, fit)) => ST.with(|s| *s.borrow_mut() = State { sessions, imported, existing, fit, account }),
                Err(message) => {
                    ST.with(|s| *s.borrow_mut() = State::default());
                    imp.set_note(message.into());
                }
            }
            render(&ui, &a);
        });
    });
}

/// The question about a session used moments ago, then the import.
fn pick(ui: &AppWindow, app: &Arc<App>, id: &str) {
    if IMPORTING.load(Ordering::SeqCst) {
        return;
    }
    let found = ST.with(|s| {
        let s = s.borrow();
        s.sessions.iter().find(|x| x.id == id).cloned().map(|x| {
            let usable = s.existing.contains(&x.cwd) && s.fit.contains(&x.cwd);
            (x, s.account.clone(), s.imported.contains(id), usable)
        })
    });
    let Some((session, account, done, exists)) = found else { return };
    if done || !exists || !session.folder_ok {
        return;
    }
    let recent = now_ms().saturating_sub(session.modified_ms) < tenrec::view::import::RECENT_MS;
    let a = app.clone();
    // The chat on screen is replaced only when the import is done (see `finish`): unsaved edits are asked about then, so edits made
    // while the file was being read are not lost.
    let start = move |_: &AppWindow| load(a, session, account);
    if recent {
        panels::ask(ui, tr(app.lang, "imp_inuse_q"), tr(app.lang, "imp_pick"), false, Box::new({
            let weak = ui.as_weak();
            move |ok| {
                if let (true, Some(ui)) = (ok, weak.upgrade()) {
                    start(&ui);
                }
            }
        }));
    } else {
        start(ui);
    }
}

/// Reads the session file, then imports; a file too big to show is asked about.
fn load(app: Arc<App>, session: CliSession, account: Option<String>) {
    IMPORTING.store(true, Ordering::SeqCst);
    tokio::spawn(async move {
        let path = session.path.clone();
        let read = tokio::task::spawn_blocking(move || read_history(&path, MAX_EVENTS, MAX_BYTES)).await;
        match read {
            Ok(Ok(history)) => finish(app, session, account, history).await,
            Ok(Err("imp_too_big")) => {
                let a = app.clone();
                let _ = app.weak.clone().upgrade_in_event_loop(move |ui| {
                    // Not answered: nothing is imported and the list stays.
                    let yes = a.clone();
                    panels::ask(&ui, tr(a.lang, "imp_too_big_q"), tr(a.lang, "imp_pick"), false, Box::new(move |ok| {
                        if ok {
                            tokio::spawn(finish(yes, session, account, History::default()));
                        } else {
                            IMPORTING.store(false, Ordering::SeqCst);
                        }
                    }));
                });
            }
            _ => {
                IMPORTING.store(false, Ordering::SeqCst);
                crate::toast_later(&app.weak, tr(app.lang, "imp_read_fail"));
            }
        }
    });
}

async fn finish(app: Arc<App>, session: CliSession, account: Option<String>, history: History) {
    let made = app.registry.import_cli(&session.cwd, &session.id, &session.title, account.as_deref(), history).await;
    IMPORTING.store(false, Ordering::SeqCst);
    match made {
        Ok(handle) => {
            crate::remember_folder(&app.data, &session.cwd);
            let sid = handle.sid.clone();
            let b = app.clone();
            let cli_id = session.id.clone();
            let _ = app.weak.clone().upgrade_in_event_loop(move |ui| {
                // Made now: the row says so even if the person keeps the file they were editing and stays where they are.
                ST.with(|s| s.borrow_mut().imported.insert(cli_id));
                render(&ui, &b);
                let c = b.clone();
                files::guard_dirty(&ui, &b, move |ui| {
                    ui.global::<Imp>().set_open(false);
                    ui.set_new_open(false);
                    ui.set_side(0);
                    crate::open_chat(&c, sid);
                });
            });
            crate::refresh_sessions(&app).await;
        }
        Err(message) => crate::toast_later(&app.weak, message),
    }
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let imp = ui.global::<Imp>();
    let a = app.clone();
    imp.on_scan(move |index, name| scan(&a, index, name.as_str()));
    let (a, weak) = (app.clone(), ui.as_weak());
    imp.on_filter_edited(move |_| {
        if let Some(ui) = weak.upgrade() {
            render(&ui, &a);
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    imp.on_pick(move |id| {
        if let Some(ui) = weak.upgrade() {
            pick(&ui, &a, id.as_str());
        }
    });
}
