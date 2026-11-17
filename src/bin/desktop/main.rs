//! Tenrec as a native desktop app (Slint). The core runs in this process on a tokio runtime; the window runs Slint's event loop
//! on the main thread. There is no network listener: `claude` reaches the app only through the MCP socket (a named pipe on
//! Windows) that `start_core` opens. A chat's events are read here, turned into rows by `tenrec::view::transcript`, and
//! handed to the window at most every 33 ms.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

slint::include_modules!();

mod accounts;
mod attach;
mod files;
mod import;
mod bus;
mod panels;
mod prov;
mod sched;
mod shots;
mod team;
mod usage;
mod notify;
mod palette;
mod term;
mod vhdb;
mod wt;
mod zoom;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, SharedString, StyledText, VecModel};
use tenrec::registry::Registry;
use tenrec::view::i18n::{fill, tr, Lang};
use tenrec::view::panels::{ago, attention, in_context, Mark};
use tenrec::view::transcript::{model_label, Row, Transcript};

/// What the core side keeps about the chat on screen.
struct Current {
    sid: Option<String>,
    transcript: Transcript,
    /// Changes each time another chat is opened, so a late update of the old one is dropped.
    epoch: u64,
    follow: Option<tokio::task::JoinHandle<()>>,
}

struct App {
    registry: Arc<Registry>,
    lang: Lang,
    current: Mutex<Current>,
    weak: slint::Weak<AppWindow>,
    data: PathBuf,
    /// The in-process link the panels speak over (the web page's operations).
    bus: bus::Bus,
    /// The open chat's project folder as the core writes it: the Memory panel's "this project".
    cwd: Mutex<Option<String>>,
    /// The open chat's account (None: the default one); a skill proposed from the chat belongs to it.
    chat_acct: Mutex<Option<String>>,
}

thread_local! {
    // The UI thread's copy of the rows on screen, to change only what changed.
    static SHOWN: RefCell<Vec<Row>> = const { RefCell::new(Vec::new()) };
    // Every chat the host listed with what the account badges need; the list on screen is the part of it under the account shown.
    // The project folders of the chat list that are closed.
    static COLLAPSED: RefCell<std::collections::HashSet<String>> = RefCell::new(std::collections::HashSet::new());
    static ALL: RefCell<Vec<(SessionItem, Mark)>> = const { RefCell::new(Vec::new()) };
    static ROWS: RefCell<Option<Rc<VecModel<ChatRow>>>> = const { RefCell::new(None) };
    // Set when a chat is opened or a message is sent: the list follows the end until that turn is over.
    // What was typed in the composer of each chat that was left, put back when the chat is opened again.
    static DRAFTS: RefCell<std::collections::HashMap<String, String>> = RefCell::new(std::collections::HashMap::new());
    static STICK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    // Kept for the window's life: on Linux the copied text is served by this process and is lost when the clipboard is dropped.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

fn main() {
    // `tenrec-desktop mcp`: the MCP server claude starts for a chat (stdio), not the window.
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        let rt = tokio::runtime::Runtime::new().expect("no tokio runtime");
        std::process::exit(rt.block_on(tenrec::mcp::run_stdio()));
    }
    // `tenrec-desktop guard --data <folder> --sid <chat>`: the PreToolUse hook of a chat's claude (the app's own guard), not the window.
    if std::env::args_os().nth(1).is_some_and(|a| a == "guard") {
        let rest: Vec<String> = std::env::args_os().skip(2).map(|a| a.to_string_lossy().into_owned()).collect();
        std::process::exit(tenrec::workbench::hook::run(&rest));
    }
    let mut data: Option<PathBuf> = None;
    let mut claude: Option<String> = None;
    let mut size: Option<(u32, u32)> = None;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--data" => data = args.next().map(PathBuf::from),
            "--claude" => claude = args.next(),
            // `--size 1280x820`: the window's size in pixels.
            "--size" => size = args.next().and_then(|v| v.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))),
            _ => {}
        }
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("no tokio runtime");
    // Some core calls are plain functions that start tasks: the runtime must be entered for as long as the window lives.
    let _entered = rt.enter();
    let data = data.unwrap_or_else(tenrec::registry::data_dir);
    // The language chosen in Settings wins over the system's.
    let lang = desktop_settings(&data).get("lang").and_then(|v| v.as_str()).and_then(Lang::from_code).unwrap_or_else(Lang::from_system);
    #[cfg(windows)]
    {
        let inside = tenrec::platform::home_dir().is_some_and(|profile| tenrec::platform::data_dir_in_profile(&data, &profile));
        if !inside {
            fatal("the data folder must be inside your user profile folder on Windows (use --data with a folder there)");
        }
    }
    // One app per data folder: two of them would answer the same chats and sweep each other's containers. The lock is the system's,
    // so it goes away with the process, however it ends.
    let _single = single_instance(&data).unwrap_or_else(|| fatal(&tr(lang, "already_running")));
    let log = start_log(&data);
    let config = tenrec::server::Config { port: 0, data_dir: data.clone(), bin: claude, mcp_bin: std::env::current_exe().ok(), extract: Default::default(), propose: Default::default(), sched: Default::default() };
    let registry = match tenrec::server::start_core(config) {
        Ok(registry) => registry,
        Err(error) => fatal(&format!("could not start: {error:#}")),
    };

    let ui = AppWindow::new().unwrap_or_else(|e| fatal(&format!("could not open the window: {e}")));
    set_words(&ui, lang);
    // The look chosen in Settings: 0 dark, 1 light, 2 the system's.
    let mode = desktop_settings(&data).get("theme").and_then(|v| v.as_i64()).filter(|m| (0..=2).contains(m)).unwrap_or(0) as i32;
    ui.global::<Theme>().set_mode(mode);
    ui.set_model_name(tr(lang, "model_default").into());
    ui.set_foot_prov(tr(lang, "prov_anthropic").into());
    ui.set_new_cwd(last_folder(&data).into());
    let rows = Rc::new(VecModel::<ChatRow>::default());
    ui.set_rows(ModelRc::from(rows.clone()));
    ROWS.with(|r| *r.borrow_mut() = Some(rows));

    let zoom_pct = zoom::saved(&data);
    let zoom_data = data.clone();
    let (bus_tx, bus_rx, term_rx) = tenrec::server::local_with_terminal(registry.clone(), term::enabled());
    let app = Arc::new(App { registry: registry.clone(), lang, current: Mutex::new(Current { sid: None, transcript: Transcript::new(lang), epoch: 0, follow: None }), weak: ui.as_weak(), data, bus: bus::Bus::new(bus_tx), cwd: Mutex::new(None), chat_acct: Mutex::new(None) });
    wire(&ui, &app);
    panels::wire(&ui, &app);
    team::wire(&ui, &app);
    sched::wire(&ui, &app);
    prov::wire(&ui, &app);
    attach::wire(&ui, &app);
    shots::wire(&ui, &app);
    files::wire(&ui, &app);
    wt::wire(&ui, &app);
    term::wire(&ui, &app);
    vhdb::wire(&ui, &app);
    palette::wire(&ui, &app);
    import::wire(&ui, &app);
    accounts::wire(&ui, &app);
    usage::wire(&ui, &app);
    {
        let prefs = ui.global::<Prefs>();
        let d = app.data.clone();
        prefs.on_theme_chosen(move |i| remember(&d, "theme", i.into()));
        prefs.set_lang(if lang == Lang::Ko { 0 } else { 1 });
        let (d, weak) = (app.data.clone(), ui.as_weak());
        prefs.on_lang_chosen(move |i| {
            let chosen = if i == 0 { Lang::Ko } else { Lang::En };
            remember(&d, "lang", chosen.code().into());
            if let Some(ui) = weak.upgrade() {
                let p = ui.global::<Prefs>();
                p.set_lang(i);
                p.set_changed(chosen != lang);
            }
        });
    }
    rt.spawn(panels::read(app.clone(), bus_rx));
    rt.spawn(term::read_frames(app.clone(), term_rx));
    // Held until the window closes.
    let _zoom = zoom::start(&ui, zoom_data, lang, zoom_pct);
    rt.spawn(sessions_loop(app.clone()));
    if let Some((w, h)) = size {
        ui.window().set_size(slint::PhysicalSize::new(w, h));
    }
    // The most recent chat is open when the window appears.
    let a = app.clone();
    rt.spawn(async move {
        if let Some(info) = a.registry.list().await.into_iter().next() {
            let sid = info.meta.sid.clone();
            let b = a.clone();
            // Not over a chat the person opened in the meantime.
            let _ = a.weak.upgrade_in_event_loop(move |_| {
                if b.current.lock().expect("current").sid.is_none() {
                    open_chat(&b, sid);
                }
            });
        }
    });

    // Closing the window with unsaved edits asks first.
    {
        let (a, weak) = (app.clone(), ui.as_weak());
        ui.window().on_close_requested(move || {
            let Some(ui) = weak.upgrade() else { return slint::CloseRequestResponse::HideWindow };
            if !files::has_unsaved(&ui) {
                return slint::CloseRequestResponse::HideWindow;
            }
            let w = ui.as_weak();
            panels::ask(&ui, tr(a.lang, "fs_dirty_discard"), tr(a.lang, "fs_discard"), true, Box::new(move |ok| {
                if let (true, Some(ui)) = (ok, w.upgrade()) {
                    let _ = ui.hide();
                }
            }));
            slint::CloseRequestResponse::KeepWindowShown
        });
    }
    let _ = log;
    let result = ui.run();
    tracing::info!("window closed");
    // Leave no claude, and nothing it started, behind (nor a shell of the terminal drawer).
    registry.kill_all();
    tenrec::server::term_ops::kill_all();
    std::thread::sleep(Duration::from_millis(300));
    if let Err(error) = result {
        eprintln!("{error}");
    }
}

/// A release build on Windows has no console: what the app reports, and a crash, go to `<data>/logs/desktop.log` (kept short:
/// it is started again when it passes 1 MB). Returns the file's path.
fn start_log(data: &std::path::Path) -> Option<PathBuf> {
    let dir = data.join("logs");
    tenrec::private::create_dir(data).ok()?;
    tenrec::private::create_dir(&dir).ok()?;
    let path = dir.join("desktop.log");
    if std::fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;
    let file = std::sync::Arc::new(std::sync::Mutex::new(file));
    let writer = file.clone();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")))
        .with_ansi(false)
        .with_writer(move || LogWriter(writer.clone()))
        .try_init();
    let crash = file;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        if let Ok(mut f) = crash.lock() {
            let _ = writeln!(f, "{} PANIC {info}", tenrec::now_ms());
        }
        previous(info);
    }));
    tracing::info!("tenrec-desktop {} started ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    Some(path)
}

struct LogWriter(std::sync::Arc<std::sync::Mutex<std::fs::File>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().map_err(|_| std::io::Error::other("log lock"))?.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().map_err(|_| std::io::Error::other("log lock"))?.flush()
    }
}

/// Says why the app cannot run, and ends it: in a message box on Windows (a release build has no console), on stderr elsewhere.
/// Takes the lock of this data folder: None when another app holds it. Any other trouble (no folder, no permission) is not a
/// reason to refuse: the core reports it when it starts.
fn single_instance(data: &std::path::Path) -> Option<std::fs::File> {
    use std::fs::TryLockError;
    let _ = std::fs::create_dir_all(data);
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(data.join("desktop.lock")).ok()?;
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(TryLockError::WouldBlock) => None,
        Err(TryLockError::Error(_)) => Some(file),
    }
}

fn fatal(message: &str) -> ! {
    eprintln!("{message}");
    tracing::error!("{message}");
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (text, title) = (wide(message), wide("Tenrec"));
        // SAFETY: two NUL-ended UTF-16 strings that outlive the call; no owner window.
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR) };
    }
    std::process::exit(2);
}

fn set_words(ui: &AppWindow, lang: Lang) {
    let t = ui.global::<T>();
    t.on_tr(move |key| tr(lang, &key).into());
    let w = |k: &str| SharedString::from(tr(lang, k));
    t.set_app_sub(w("app_sub"));
    t.set_tab_chats(w("tab_chats"));
    t.set_tab_memory(w("tab_memory"));
    t.set_tab_skills(w("tab_skills"));
    t.set_tab_sched(w("tab_sched"));
    t.set_tab_files(w("tab_files"));
    t.set_sessions(w("sessions"));
    t.set_new_chat(w("new_chat"));
    t.set_folder(w("folder"));
    t.set_model(w("model"));
    t.set_create(w("create"));
    t.set_cancel(w("cancel"));
    t.set_empty(w("empty"));
    t.set_placeholder(w("placeholder"));
    t.set_send(w("send"));
    t.set_stop(w("stop"));
    t.set_hint(w("hint"));
    t.set_copy(w("copy"));
    t.set_perm_title(w("perm_title"));
    t.set_deny_reason(w("deny_reason"));
    t.set_deny(w("deny"));
    t.set_allow_once(w("allow_once"));
    t.set_allow_chat(w("allow_chat"));
    t.set_perm_suggest(w("perm_suggest"));
    t.set_perm_blocked(w("perm_blocked"));
    t.set_connected(w("connected"));
    t.set_phase_note(w("phase_note"));
    t.set_pwd(w("pwd"));
    t.set_new_intro(w("new_intro"));
    t.set_folder_ph(w("folder_ph"));
    t.set_start(w("start"));
    t.set_zoom(w("zoom"));
}

/// `<data>/desktop.json`: what the window remembers between runs. Each writer changes only its own key, under one lock,
/// because the folder is saved from a tokio task and the zoom from the UI thread.
static DESKTOP_JSON: Mutex<()> = Mutex::new(());

/// The remembered settings; empty when the file is missing, unreadable or not a JSON object.
fn desktop_settings(data: &std::path::Path) -> serde_json::Map<String, serde_json::Value> {
    match std::fs::read_to_string(data.join("desktop.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    }
}

/// Saves one setting and keeps the others.
fn remember(data: &std::path::Path, key: &str, value: serde_json::Value) {
    let _held = DESKTOP_JSON.lock().unwrap_or_else(|e| e.into_inner());
    let mut map = desktop_settings(data);
    map.insert(key.to_owned(), value);
    if let Err(error) = tenrec::private::write(&data.join("desktop.json"), &serde_json::Value::Object(map).to_string()) {
        tracing::warn!("desktop.json not saved: {error}");
    }
}

fn last_folder(data: &std::path::Path) -> String {
    desktop_settings(data).get("last_cwd").and_then(|v| v.as_str()).map(str::to_owned).unwrap_or_default()
}

fn remember_folder(data: &std::path::Path, cwd: &str) {
    remember(data, "last_cwd", cwd.into());
}

/// The end of a long path for the header.
fn tail_path(path: &str, max: usize) -> String {
    // A Windows path made canonical starts with `\\?\`; it is shown as the plain drive path.
    let path = path.strip_prefix(r"\\?\UNC\").map(|p| format!(r"\\{p}")).unwrap_or_else(|| path.strip_prefix(r"\\?\").unwrap_or(path).to_owned());
    let path = path.as_str();
    if path.chars().count() <= max {
        return path.to_owned();
    }
    let sep = if path.contains('\\') { '\\' } else { '/' };
    let mut parts: Vec<&str> = path.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    let mut out = parts.pop().unwrap_or("").to_owned();
    while let Some(next) = parts.last() {
        if out.chars().count() + next.chars().count() + 1 > max.saturating_sub(2) {
            break;
        }
        out = format!("{next}{sep}{out}");
        parts.pop();
    }
    format!("…{sep}{out}")
}

fn toast(ui: &AppWindow, text: String) {
    panels::toast_with(ui, text, None, 2600);
}

fn toast_later(weak: &slint::Weak<AppWindow>, text: String) {
    tracing::warn!("shown to the person: {text}");
    let _ = weak.upgrade_in_event_loop(move |ui| toast(&ui, text));
}

fn wire(ui: &AppWindow, app: &Arc<App>) {
    let a = app.clone();
    ui.on_open_chat(move |sid| {
        let Some(ui) = a.weak.upgrade() else { return };
        let (b, sid) = (a.clone(), sid.to_string());
        // Unsaved edits of an open file are asked about before the chat changes.
        files::guard_dirty(&ui, &a, move |_| open_chat(&b, sid));
    });

    let a = app.clone();
    ui.on_new_chat(move |cwd, model, index, name, prov_index, prov_name, container, worktree, perm| {
        let (cwd, model) = (cwd.trim().to_owned(), model.to_string());
        // The pickers' indexes are checked against the names they showed: the lists can change while they are open.
        let Ok(account) = accounts::chosen(index, &name) else {
            toast_later(&a.weak, tr(a.lang, "acct_changed_retry"));
            return;
        };
        let Ok(provider) = prov::chosen(prov_index, &prov_name) else {
            toast_later(&a.weak, tr(a.lang, "prov_changed_retry"));
            return;
        };
        // A provider chat has no account of its own, and a container needs a provider.
        let account = if provider.is_some() { None } else { account };
        // The permission mode is one of the six, by the picker's index, and fits the kind of chat (checked again when it is made).
        let Some(mode) = tenrec::claude::perm::PermMode::ALL.get(perm.max(0) as usize).copied() else {
            toast_later(&a.weak, tr(a.lang, "perm_unknown"));
            return;
        };
        // The same combinations the host refuses, said in the window's language (a worktree chat never runs in a container).
        if let Some(key) = tenrec::registry::mode_refusal(mode, provider.is_some(), container && provider.is_some() && !worktree) {
            toast_later(&a.weak, tr(a.lang, key));
            return;
        }
        let a = a.clone();
        // The new chat replaces the chat on screen: unsaved edits of an open file are asked about before it is made.
        let Some(ui) = a.weak.upgrade() else { return };
        let (a_weak, lang) = (ui.as_weak(), a.lang);
        let chat_name = ui.get_new_name().to_string();
        let make: Box<dyn FnOnce(&AppWindow)> = Box::new(move |ui: &AppWindow| { let a = a.clone(); files::guard_dirty(ui, &a.clone(), move |_| { tokio::spawn(async move {
            let model = (model != "default" && !model.is_empty()).then_some(model);
            // A worktree chat is made in a new git worktree (it cannot also run in a container).
            let made = if worktree {
                a.registry.create_isolated_mode(&cwd, model, provider.as_deref(), account.as_deref(), mode).await
            } else {
                a.registry.create_in_mode(&cwd, model, provider.as_deref(), container && provider.is_some(), account.as_deref(), mode).map(|handle| (handle, None))
            };
            match made {
                Ok((handle, notice)) => {
                    if !chat_name.trim().is_empty() {
                        let _ = handle.rename(chat_name.clone()).await;
                    }
                    if notice == Some("below_top") {
                        toast_later(&a.weak, tr(a.lang, "new_wt_below_top"));
                    }
                    remember_folder(&a.data, &cwd);
                    let sid = handle.sid.clone();
                    let b = a.clone();
                    let _ = a.weak.upgrade_in_event_loop(move |ui| {
                        // Edits made while the chat was being made are asked about now, just before the window switches to it.
                        let c = b.clone();
                        files::guard_dirty(&ui, &b, move |ui| {
                            ui.set_new_open(false);
                            // The new chat is what is shown, whichever panel was open before.
                            ui.set_side(0);
                            open_chat(&c, sid);
                        });
                    });
                    refresh_sessions(&a).await;
                }
                Err(message) => toast_later(&a.weak, message),
            }
        }); }); });
        // Skipping every check is asked about first, in words that say what it means; any other mode goes ahead.
        if mode == tenrec::claude::perm::PermMode::Bypass {
            let weak = a_weak.clone();
            panels::ask(&ui, tr(lang, "perm_bypass_confirm"), tr(lang, "perm_bypass_ok"), true, Box::new(move |yes| {
                if let (true, Some(ui)) = (yes, weak.upgrade()) {
                    make(&ui);
                }
            }));
        } else {
            make(&ui);
        }
    });

    // The open chat's model. With team mode on it is the lead's model, set through the plug-in's own command; otherwise the chat's
    // `claude` is started again with the other model (between turns only).
    let a = app.clone();
    ui.on_set_chat_model(move |index| {
        const ALIASES: [&str; 7] = ["default", "sonnet", "opus", "haiku", "fable", "sonnet[1m]", "opus[1m]"];
        let (Some(name), Some(ui)) = (ALIASES.get(index.max(0) as usize).copied(), a.weak.upgrade()) else { return };
        let team = ui.global::<Team>();
        if team.get_show() && team.get_chip() == "on" {
            if name == "default" {
                crate::toast(&ui, tr(a.lang, "model_lead_default"));
            } else {
                team::send_role(&a, "lead", name);
            }
            return;
        }
        if team.get_show() && team.get_chip() == "unknown" {
            crate::toast(&ui, tr(a.lang, "model_team_check"));
            return;
        }
        let Some(sid) = a.current.lock().expect("current").sid.clone() else { return };
        let (a, model) = (a.clone(), tenrec::registry::clean_model(name));
        tokio::spawn(async move {
            let Some(handle) = a.registry.get(&sid) else { return };
            match handle.set_model(model).await {
                Ok(()) => toast_later(&a.weak, tr(a.lang, "model_changed")),
                Err(message) => toast_later(&a.weak, message),
            }
            refresh_sessions(&a).await;
        });
    });

    // The open chat's name and deleting it.
    let (a, weak) = (app.clone(), ui.as_weak());
    ui.on_toggle_group(move |key| {
        COLLAPSED.with(|c| {
            let mut c = c.borrow_mut();
            if !c.remove(key.as_str()) {
                c.insert(key.to_string());
            }
        });
        if let Some(ui) = weak.upgrade() {
            show_sessions(&ui, &a);
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    ui.on_continue_chat(move |index, name| {
        let Some(ui) = weak.upgrade() else { return };
        let Ok(target) = accounts::chosen(index, name.as_str()) else { return };
        let Some(sid) = a.current.lock().expect("current").sid.clone() else { return };
        let body = fill(&tr(a.lang, "cont_confirm"), &[("name", name.to_string())]);
        let b = a.clone();
        panels::ask(&ui, body, tr(a.lang, "cont_go"), false, Box::new(move |ok| {
            if !ok {
                return;
            }
            let (c, sid, target) = (b.clone(), sid.clone(), target.clone());
            tokio::spawn(async move {
                match c.registry.continue_as(&sid, target.as_deref()).await {
                    Ok(handle) => {
                        let new_sid = handle.sid.clone();
                        let d = c.clone();
                        let _ = c.weak.clone().upgrade_in_event_loop(move |_| open_chat(&d, new_sid));
                        refresh_sessions(&c).await;
                    }
                    Err(message) => toast_later(&c.weak, message),
                }
            });
        }));
    });
    let a = app.clone();
    ui.on_rename_chat(move |text| {
        let Some(sid) = a.current.lock().expect("current").sid.clone() else { return };
        let (a, text) = (a.clone(), text.to_string());
        tokio::spawn(async move {
            let Some(handle) = a.registry.get(&sid) else { return };
            if let Err(message) = handle.rename(text).await {
                toast_later(&a.weak, message);
            }
            refresh_sessions(&a).await;
        });
    });
    let a = app.clone();
    ui.on_delete_chat(move || {
        let Some(ui) = a.weak.upgrade() else { return };
        let Some(sid) = a.current.lock().expect("current").sid.clone() else { return };
        let a = a.clone();
        panels::ask(&ui, tr(a.lang, "chat_delete_confirm"), tr(a.lang, "chat_delete_ok"), true, Box::new(move |yes| {
            if !yes {
                return;
            }
            let a = a.clone();
            let sid = sid.clone();
            tokio::spawn(async move {
                match a.registry.delete(&sid).await {
                    Ok(()) => {
                        let b = a.clone();
                        let _ = a.weak.upgrade_in_event_loop(move |ui| close_chat(&ui, &b, &sid));
                        refresh_sessions(&a).await;
                    }
                    Err(message) => toast_later(&a.weak, message),
                }
            });
        }));
    });

    // "Browse…": the system's folder picker, over the window.
    let (lang, weak) = (app.lang, ui.as_weak());
    ui.on_pick_folder(move || {
        let Some(ui) = weak.upgrade() else { return };
        let start = ui.get_new_cwd().to_string();
        pick_folder(&ui, lang, start, |ui, busy| ui.set_picking(busy), |ui, path| ui.set_new_cwd(path.into()));
    });

    let a = app.clone();
    ui.on_send(move |text| {
        // Shell mode: the line is a command for the terminal's shell, not a message to the chat.
        if let Some(ui) = a.weak.upgrade() {
            if ui.get_shell_mode() {
                // Attachments alone are not a command.
                if text.trim().is_empty() {
                    return;
                }
                if term::run_line(&ui, &a, text.trim()) {
                    ui.set_draft(SharedString::new());
                } else {
                    toast(&ui, tr(a.lang, "shell_refused"));
                }
                return;
            }
        }
        send_text(&a, text.trim().to_owned(), true)
    });

    let a = app.clone();
    ui.on_stop(move || {
        let Some(sid) = a.current.lock().expect("current").sid.clone() else { return };
        let a = a.clone();
        tokio::spawn(async move {
            if let Some(handle) = a.registry.get(&sid) {
                if let Err(message) = handle.interrupt().await {
                    toast_later(&a.weak, message);
                }
            }
        });
    });

    let a = app.clone();
    ui.on_toggle(move |key| {
        a.current.lock().expect("current").transcript.toggle(&key);
        push(&a);
    });

    let a = app.clone();
    ui.on_copy(move |text| copy_text(&a, text.to_string()));
    let a = app.clone();
    ui.on_link(move |url| copy_text(&a, url.to_string()));

    // A rule to remember: the request stays on the screen until the host accepts the rule (its "resolved" event closes it); a refusal
    // is told and the request waits for another answer.
    let a = app.clone();
    ui.on_grant_perm(move |request_id, grant| {
        let (request_id, grant) = (request_id.to_string(), grant.to_string());
        let sid = {
            let cur = a.current.lock().expect("current");
            let Some(sid) = cur.sid.clone() else { return };
            if !cur.transcript.pending.iter().any(|p| p["request_id"].as_str() == Some(request_id.as_str())) {
                return;
            }
            sid
        };
        let grant = if grant == "always" { tenrec::session::Grant::Always } else { tenrec::session::Grant::Session };
        let a = a.clone();
        tokio::spawn(async move {
            if let Some(handle) = a.registry.get(&sid) {
                if let Err(message) = handle.answer(request_id, true, None, grant).await {
                    toast_later(&a.weak, message);
                }
            }
        });
    });

    let a = app.clone();
    ui.on_answer_perm(move |request_id, allow, remember, reason| {
        let request_id = request_id.to_string();
        let sid = {
            let mut cur = a.current.lock().expect("current");
            let Some(sid) = cur.sid.clone() else { return };
            // The answer is for the request the dialog shows, and only while it still waits: a second click, a repeated Esc or a
            // request resolved elsewhere never answers another one the person has not seen.
            if !cur.transcript.pending.iter().any(|p| p["request_id"].as_str() == Some(request_id.as_str())) {
                return;
            }
            // Closed at once; the chat's own "resolved" event confirms it.
            cur.transcript.pending.retain(|p| p["request_id"].as_str() != Some(request_id.as_str()));
            sid
        };
        push(&a);
        let reason = reason.trim().to_owned();
        let a = a.clone();
        tokio::spawn(async move {
            if let Some(handle) = a.registry.get(&sid) {
                let message = (!allow && !reason.is_empty()).then_some(reason);
                if let Err(message) = handle.permission(request_id, allow, message, remember && allow).await {
                    toast_later(&a.weak, message);
                }
            }
        });
    });
}

/// Sends a message to the open chat. From the composer the draft is cleared and put back if the message was not sent; a message
/// the window makes itself (a team-mode command) leaves the draft alone.
fn send_text(a: &Arc<App>, text: String, from_composer: bool) {
    let Some(sid) = a.current.lock().expect("current").sid.clone() else { return };
    // The images being attached go with a message from the composer; a message the window makes itself carries none.
    let (images, held) = match a.weak.upgrade() {
        Some(ui) if from_composer => attach::take(&ui),
        _ => (Vec::new(), Vec::new()),
    };
    if text.is_empty() && images.is_empty() {
        return;
    }
    if from_composer {
        if let Some(ui) = a.weak.upgrade() {
            ui.set_draft(SharedString::new());
        }
    }
    stick_briefly();
    let a = a.clone();
    tokio::spawn(async move {
        let Some(handle) = a.registry.get(&sid) else { return };
        if let Err(message) = handle.send(text.clone(), images).await {
            // Not sent: the text and the images go back into the composer, unless something new was typed or another chat is
            // open (the draft is shared by every chat).
            let b = a.clone();
            let _ = a.weak.upgrade_in_event_loop(move |ui| {
                if from_composer && ui.get_draft().is_empty() && b.current.lock().expect("current").sid.as_deref() == Some(sid.as_str()) {
                    ui.set_draft(text.into());
                    attach::restore(&ui, held);
                }
                team::cancel_wait();
            });
            toast_later(&a.weak, message);
        }
    });
}

/// The system's folder picker, over the window. The field next to it stays editable (a pasted path, a UNC path, a desktop with no
/// picker), and a cancelled pick changes nothing. `busy` says whether a picker is open; `done` gets the chosen folder.
pub fn pick_folder(ui: &AppWindow, lang: Lang, start: String, busy: fn(&AppWindow, bool), done: fn(&AppWindow, String)) {
    // Windows's picker is modal over the window, so the button waits for it. A Linux portal can answer late or never (or show
    // its window elsewhere): there the button stays usable.
    busy(ui, cfg!(windows));
    let typed = PathBuf::from(start.trim());
    let mut dialog = rfd::AsyncFileDialog::new().set_title(tr(lang, "folder_pick_title"));
    if typed.is_dir() {
        dialog = dialog.set_directory(&typed);
    } else if let Some(home) = tenrec::platform::home_dir() {
        dialog = dialog.set_directory(home);
    }
    let handle = ui.window().window_handle();
    let picked = dialog.set_parent(&handle).pick_folder();
    let weak = ui.as_weak();
    let started = slint::spawn_local(async move {
        let picked = picked.await;
        let Some(ui) = weak.upgrade() else { return };
        busy(&ui, false);
        if let Some(folder) = picked {
            match folder.path().to_str() {
                Some(path) => done(&ui, path.to_owned()),
                None => toast(&ui, tr(lang, "folder_bad")),
            }
        }
    });
    if started.is_err() {
        busy(ui, false);
    }
}

fn copy_text(app: &Arc<App>, text: String) {
    if copy_quiet(text) {
        toast_later(&app.weak, tr(app.lang, "copied"));
    }
}

/// Puts text on the clipboard; true when it is there.
fn copy_quiet(text: String) -> bool {
    CLIPBOARD.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = arboard::Clipboard::new().ok();
        }
        let ok = c.as_mut().is_some_and(|c| c.set_text(text).is_ok());
        if !ok {
            // Made again on the next copy.
            *c = None;
        }
        ok
    })
}

/// Opens a chat: its log is read from the start, then its live events follow.
/// The chat on screen was deleted: the window shows no chat (nothing is switched to without a click).
fn close_chat(ui: &AppWindow, app: &Arc<App>, sid: &str) {
    {
        let mut cur = app.current.lock().expect("current");
        if cur.sid.as_deref() != Some(sid) {
            return;
        }
        if let Some(task) = cur.follow.take() {
            task.abort();
        }
        cur.epoch += 1;
        cur.sid = None;
        cur.transcript = Transcript::new(app.lang);
    }
    // The chat is gone: so is what was typed for it.
    DRAFTS.with(|d| d.borrow_mut().remove(sid));
    ui.set_draft(SharedString::new());
    attach::forget(ui, sid);
    SHOWN.with(|s| s.borrow_mut().clear());
    if let Some(ui) = app.weak.upgrade() {
        shots::reset(&ui);
    }
    ROWS.with(|r| {
        if let Some(rows) = r.borrow().as_ref() {
            rows.set_vec(Vec::new());
        }
    });
    ui.set_has_chat(false);
    ui.set_chat_title(SharedString::new());
    ui.set_chat_account(SharedString::new());
    ui.set_chat_perm(SharedString::new());
    team::reset(ui, app.lang);
    files::reset(ui);
}

/// The chat follows its end for the moment after a message is sent or a chat is opened (its rows are measured as they come into view);
/// after that it follows only while the person is at the end, so scrolling up during a running turn is left alone.
fn stick_briefly() {
    STICK.with(|s| s.set(true));
    slint::Timer::single_shot(Duration::from_millis(1500), || STICK.with(|s| s.set(false)));
}

fn open_chat(app: &Arc<App>, sid: String) {
    let (epoch, left) = {
        let mut cur = app.current.lock().expect("current");
        if let Some(task) = cur.follow.take() {
            task.abort();
        }
        cur.epoch += 1;
        let left = cur.sid.replace(sid.clone());
        cur.transcript = Transcript::new(app.lang);
        (cur.epoch, left)
    };
    let left_att = left.clone();
    // The composer belongs to the chat: what was typed stays with the chat that was left, and the opened chat has its own.
    if let Some(ui) = app.weak.upgrade() {
        DRAFTS.with(|d| {
            let mut d = d.borrow_mut();
            if let Some(old) = left {
                let text = ui.get_draft().to_string();
                if text.is_empty() {
                    d.remove(&old);
                } else {
                    d.insert(old, text);
                }
            }
            ui.set_draft(d.remove(&sid).unwrap_or_default().into());
        });
    }
    SHOWN.with(|s| s.borrow_mut().clear());
    stick_briefly();
    ROWS.with(|r| {
        if let Some(rows) = r.borrow().as_ref() {
            rows.set_vec(Vec::new());
        }
    });
    if let Some(ui) = app.weak.upgrade() {
        ui.set_has_chat(true);
        ui.set_chat_title(SharedString::new());
        ui.set_chat_account(SharedString::new());
        ui.set_chat_perm(SharedString::new());
        ui.set_model_name(tr(app.lang, "model_default").into());
        ui.set_deny_text(SharedString::new());
        ui.set_turn_cost(SharedString::new());
        ui.set_turn_tokens(SharedString::new());
        ui.set_crumb_branch(SharedString::new());
        term::reset(&ui, app);
        // Shell mode belongs to the chat it was turned on in.
        ui.set_shell_mode(false);
        team::reset(&ui, app.lang);
        attach::switch(&ui, left_att.as_deref(), &sid);
        files::reset(&ui);
        // The new chat's snapshot is read whichever page is on screen: the team chip in the header follows it too.
        team::swb_open(&ui, app);
        // The VHDB page is on screen: it reads the new chat's project.
        if ui.get_side() == 7 {
            vhdb::open(&ui, app);
        }
        // The Files tab is on screen: it lists the new chat's folder (the tab was not opened, so nothing else would).
        if ui.get_side() == 4 {
            files::panel_open(&ui, app);
        }
        wt::reset(&ui);
        ui.invoke_focus_composer();
    }
    let a = app.clone();
    let task = tokio::spawn(async move { follow(a, sid, epoch).await });
    app.current.lock().expect("current").follow = Some(task);
    let a = app.clone();
    tokio::spawn(async move { refresh_sessions(&a).await });
}

async fn follow(app: Arc<App>, sid: String, epoch: u64) {
    let Some(handle) = app.registry.get(&sid) else { return };
    let Some((missed, mut rx)) = handle.subscribe(0).await else { return };
    let mut last = 0;
    {
        let mut cur = app.current.lock().expect("current");
        if cur.epoch != epoch {
            return;
        }
        cur.transcript.replaying = true;
        for envelope in &missed {
            last = last.max(envelope.seq);
            cur.transcript.apply(&envelope.kind, &envelope.data, envelope.ts);
        }
        cur.transcript.replaying = false;
    }
    push(&app);
    // The window is updated at most every 33 ms, and at the latest 33 ms after a change: the deadline is set by the first change
    // and kept while more events arrive, so a continuous stream is shown as it comes.
    let mut dirty = false;
    let mut deadline: Option<tokio::time::Instant> = None;
    loop {
        if dirty && deadline.is_none() {
            deadline = Some(tokio::time::Instant::now() + Duration::from_millis(33));
        }
        let until = deadline.unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(3600));
        tokio::select! {
            got = rx.recv() => match got {
                Ok(envelope) => {
                    last = last.max(envelope.seq);
                    let mut cur = app.current.lock().expect("current");
                    if cur.epoch != epoch {
                        return;
                    }
                    cur.transcript.apply(&envelope.kind, &envelope.data, envelope.ts);
                    dirty = true;
                    // A turn ended: the file open in the editor may have been changed by it.
                    if envelope.kind == "claude" && envelope.data["type"] == "result" {
                        let a = app.clone();
                        let _ = app.weak.upgrade_in_event_loop(move |ui| {
                            if a.current.lock().expect("current").epoch == epoch {
                                files::check_disk(&ui, &a);
                            }
                        });
                    }
                }
                // Events were dropped for a slow reader: take them again from the log, so a permission is never missed.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let Some((again, fresh)) = handle.subscribe(last).await else { return };
                    rx = fresh;
                    let mut cur = app.current.lock().expect("current");
                    if cur.epoch != epoch {
                        return;
                    }
                    let mut ended = false;
                    for envelope in again {
                        last = last.max(envelope.seq);
                        cur.transcript.apply(&envelope.kind, &envelope.data, envelope.ts);
                        ended |= envelope.kind == "claude" && envelope.data["type"] == "result";
                    }
                    dirty = true;
                    drop(cur);
                    if ended {
                        let a = app.clone();
                        let _ = app.weak.upgrade_in_event_loop(move |ui| {
                            if a.current.lock().expect("current").epoch == epoch {
                                files::check_disk(&ui, &a);
                            }
                        });
                    }
                }
                Err(_) => return,
            },
            _ = tokio::time::sleep_until(until) => {
                if dirty {
                    push(&app);
                    dirty = false;
                }
                deadline = None;
            }
        }
    }
}

/// Sends what the chat on screen looks like now to the window.
fn push(app: &Arc<App>) {
    let (rows, head, perm, title, epoch) = {
        let cur = app.current.lock().expect("current");
        let perm = cur.transcript.pending.first().cloned().map(|p| (p, cur.transcript.pending.len()));
        (cur.transcript.rows(), cur.transcript.head.clone(), perm, cur.transcript.first_text.clone(), cur.epoch)
    };
    let lang = app.lang;
    let has_perm = perm.is_some();
    let weak = app.weak.clone();
    let a = app.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        if a.current.lock().expect("current").epoch != epoch {
            return;
        }
        let follow = ui.get_at_bottom() || STICK.with(|s| s.get()) || SHOWN.with(|s| s.borrow().is_empty());
        apply_rows(rows, &a);
        let state = if head.state.is_empty() { "idle".to_owned() } else { head.state.clone() };
        ui.set_state(state.clone().into());
        ui.set_state_label(tr(lang, &state).into());
        if let Some((cost, tokens)) = &head.turn {
            ui.set_turn_cost(cost.clone().unwrap_or_default().into());
            ui.set_turn_tokens(tokens.clone().unwrap_or_default().into());
        }
        // The chat's size against the model's window, when a result has named the window.
        if let Some(line) = tenrec::view::transcript::context_line(head.ctx_used, head.ctx_max) {
            ui.set_turn_tokens(line.into());
        }
        if let Some(model) = &head.model {
            ui.set_model_name(model_label(model).into());
        }
        // The header's name is the chat's own (kept in its meta, put on screen by the list refresh): not the first message's words, which
        // would show for a moment as if the name had been reset.
        let _ = title;
        let view = perm_view(perm, lang, &a.cwd.lock().expect("cwd").clone().unwrap_or_default(), &a.current.lock().expect("current").sid.clone().unwrap_or_default());
        // A new request starts with an empty reason.
        let closed = view.request_id.is_empty() && !ui.get_perm().request_id.is_empty();
        if view.request_id != ui.get_perm().request_id {
            ui.set_deny_text(SharedString::new());
        }
        ui.set_perm(view);
        team::apply(&ui, &head, state != "running" && !has_perm, lang);
        // The dialog took focus with it: back to the composer, so typing and the zoom keys keep working.
        if closed {
            ui.invoke_focus_composer();
        }
        // The turn is over: the list stops following the end once it has shown it.
        if state != "running" && !head.state.is_empty() {
            STICK.with(|s| s.set(false));
        }
        if follow {
            // Twice: rows of different heights are measured as they come into view, so the end moves once more.
            for ms in [16, 150, 400, 900] {
                let weak = ui.as_weak();
                slint::Timer::single_shot(Duration::from_millis(ms), move || {
                    if let Some(ui) = weak.upgrade() {
                        ui.invoke_scroll_bottom();
                    }
                });
            }
        }
    });
}

fn perm_view(perm: Option<(serde_json::Value, usize)>, lang: Lang, cwd: &str, sid: &str) -> PermView {
    let Some((p, count)) = perm else { return PermView::default() };
    let tool = p["tool_name"].as_str().unwrap_or_default();
    let title = p["display_name"].as_str().filter(|s| !s.is_empty()).unwrap_or(tool);
    let suggestions = match p.get("permission_suggestions") {
        Some(serde_json::Value::Array(list)) if !list.is_empty() => serde_json::to_string_pretty(list).unwrap_or_default(),
        _ => String::new(),
    };
    let input = &p["input"];
    // What the card tells of the request comes from the request itself: a shell command, or the file a tool names.
    let command = input["command"].as_str().unwrap_or_default().to_owned();
    let file = ["file_path", "notebook_path", "path"].iter().find_map(|k| input[*k].as_str()).unwrap_or_default().to_owned();
    let writes = ["Edit", "Write", "MultiEdit", "NotebookEdit"].contains(&tool);
    PermView {
        visible: true,
        request_id: p["request_id"].as_str().unwrap_or_default().into(),
        title: title.into(),
        more: if count > 1 { format!("+{} {}", count - 1, tr(lang, "perm_more")).into() } else { SharedString::new() },
        description: {
            // Why Claude Code asks, when a hook (the guard) or a rule is the reason, goes under what the tool says it is for.
            let purpose = p["description"].as_str().unwrap_or_default();
            match p["decision_reason"].as_str().filter(|r| !r.is_empty()) {
                Some(why) if purpose.is_empty() => why.to_owned(),
                Some(why) => format!("{purpose}\n{why}"),
                None => purpose.to_owned(),
            }
            .into()
        },
        input: serde_json::to_string_pretty(input).unwrap_or_default().into(),
        blocked: p["blocked_path"].as_str().unwrap_or_default().into(),
        suggestions: suggestions.into(),
        // Offered for this app's own read-only search tool only; Claude Code keeps the rule for this chat's process.
        can_remember: tool == "mcp__tenrec__search_history",
        grant_session: p["grants"]["session"].as_str().unwrap_or_default().into(),
        grant_always: p["grants"]["always"].as_str().unwrap_or_default().into(),
        grant_note: {
            let g = &p["grants"];
            let mut note = Vec::new();
            if g["session"].is_string() {
                note.push(tr(lang, "perm_grant_session_note"));
            }
            if g["always"].is_string() {
                note.push(tr(lang, "perm_grant_always_note"));
            }
            if g["session"].is_string() || g["always"].is_string() {
                note.push(tr(lang, if g["exact"] == true { "perm_grant_exact" } else { "perm_grant_wild" }));
            }
            note.join(" ").into()
        },
        tool: title.into(),
        cwd: cwd.into(),
        warn: !command.is_empty() || writes,
        command: command.into(),
        file: file.into(),
        file_state: if writes { "modified" } else { "read" }.into(),
        sid: sid.chars().take(8).collect::<String>().into(),
    }
}

/// Escaped markdown back to its text, for a paragraph Slint's parser refuses.
fn unescape(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut chars = md.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                if next.is_ascii_punctuation() {
                    out.push(next);
                    chars.next();
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// The text of escaped inline markdown without its marks, as it reads on screen (for measuring its height).
fn plain_of(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut chars = md.chars().peekable();
    let mut skip_url = false;
    while let Some(c) = chars.next() {
        if skip_url {
            if c == ')' {
                skip_url = false;
            }
            continue;
        }
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            '*' | '~' | '`' | '[' => {}
            ']' if chars.peek() == Some(&'(') => {
                chars.next();
                skip_url = true;
            }
            _ => out.push(c),
        }
    }
    out
}

fn to_slint(row: &Row) -> ChatRow {
    let rich = if row.md.is_empty() { StyledText::default() } else { StyledText::from_markdown(&row.md).unwrap_or_else(|_| StyledText::from_plain_text(&unescape(&row.md))) };
    let (img_id, imgs) = shots::tiles(row);
    ChatRow {
        kind: row.kind.into(),
        depth: row.depth,
        key: row.key.clone().into(),
        text: row.text.clone().into(),
        rich,
        plain: plain_of(&row.md).into(),
        label: row.label.clone().into(),
        extra: row.extra.clone().into(),
        status: row.status.clone().into(),
        state: row.state.into(),
        detail: row.detail.clone().into(),
        open: row.open,
        badge: row.badge.clone().into(),
        add: row.add,
        del: row.del,
        indent: row.indent,
        img_id,
        imgs,
        trows: ModelRc::from(Rc::new(VecModel::from(row.grid.iter().enumerate().map(|(i, cells)| TRow { cells: ModelRc::from(Rc::new(VecModel::from(cells.iter().map(|c| SharedString::from(c.as_str())).collect::<Vec<_>>()))), head: i == 0 }).collect::<Vec<_>>()))),
    }
}

/// Changes only the rows that differ from what is on screen (a stream changes the last few).
fn apply_rows(new: Vec<Row>, app: &Arc<App>) {
    ROWS.with(|r| {
        let rows = r.borrow();
        let Some(model) = rows.as_ref() else { return };
        SHOWN.with(|s| {
            let mut shown = s.borrow_mut();
            let same = shown.iter().zip(new.iter()).take_while(|(a, b)| a == b).count();
            let common = shown.len().min(new.len());
            for (i, row) in new.iter().enumerate().take(common).skip(same) {
                model.set_row_data(i, to_slint(row));
            }
            if new.len() > shown.len() {
                for row in &new[shown.len()..] {
                    model.push(to_slint(row));
                }
            } else {
                for i in (new.len()..shown.len()).rev() {
                    model.remove(i);
                }
            }
            *shown = new;
        });
    });
    SHOWN.with(|s| shots::sync(&s.borrow(), app));
}

/// The thumbnails of a message were decoded: its rows are drawn again (the rows themselves did not change, so the diff would not).
fn refresh_rows_of(id: u32) {
    ROWS.with(|r| {
        let rows = r.borrow();
        let Some(model) = rows.as_ref() else { return };
        SHOWN.with(|s| {
            for (i, row) in s.borrow().iter().enumerate() {
                if row.images.as_ref().is_some_and(|x| x.id == id) {
                    model.set_row_data(i, to_slint(row));
                }
            }
        });
    });
}

/// The chat list, every 1.5 s and whenever the app says something changed.
async fn sessions_loop(app: Arc<App>) {
    let mut events = app.registry.events.subscribe();
    loop {
        refresh_sessions(&app).await;
        tokio::select! {
            _ = events.recv() => {}
            _ = tokio::time::sleep(Duration::from_millis(1500)) => {}
        }
    }
}

/// One chat of the list: the row, what the open chat's header shows (when it is the open one) and what the account badges need.
type Listed = (SessionItem, Option<(String, String, String, String)>, Mark);

async fn refresh_sessions(app: &Arc<App>) {
    let list = app.registry.list().await;
    let current = app.current.lock().expect("current").sid.clone();
    notify::check(&app.data, app.lang, &list.iter().map(|i| (i.meta.sid.clone(), i.pending as u32)).collect::<Vec<_>>(), current.as_deref());
    let active = list.iter().find(|i| current.as_deref() == Some(i.meta.sid.as_str()));
    *app.cwd.lock().expect("cwd") = active.map(|i| i.meta.cwd.to_string_lossy().into_owned());
    let chat_acct = active.and_then(|i| i.meta.account.as_ref().map(|a| a.id.clone()));
    let active_csid = active.and_then(|i| i.meta.claude_session_id.clone());
    // The open chat's permission mode when it is not the default: shown in the header, so a chat that never asks is never a surprise.
    let chat_model = active.and_then(|i| i.meta.model.clone());
    let chat_perm = active.map(|i| i.meta.permission_mode).filter(|m| !m.is_default());
    // The account's name for the header chip ("" for the default account: nothing to tell apart).
    let chat_acct_name = active
        .and_then(|i| i.meta.account.as_ref().map(|a| a.name.clone()))
        .map(|name| if name.chars().count() > 14 { format!("{}…", name.chars().take(13).collect::<String>()) } else { name })
        .unwrap_or_default();
    *app.chat_acct.lock().expect("chat account") = chat_acct.clone();
    let mut chats: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for info in &list {
        *chats.entry(info.meta.account.as_ref().map(|a| a.id.clone()).unwrap_or_default()).or_default() += 1;
    }
    let wt_info: wt::Info = active.and_then(|i| i.meta.worktree.as_ref().map(|w| (w.branch.clone(), w.base_branch.clone(), w.repo.display().to_string(), w.removed)));
    // The branch and the working folders come from small files in `.git`: read here, off the window's thread.
    let cwd_now = active.map(|i| i.meta.cwd.clone());
    let (branch, wt_rows) = match cwd_now {
        Some(cwd) => tokio::task::spawn_blocking(move || (tenrec::view::git::head_branch(&cwd).unwrap_or_default(), tenrec::view::git::list_worktrees(&cwd))).await.unwrap_or_default(),
        None => (String::new(), Vec::new()),
    };
    let followed = current.clone();
    let read_for = current.clone();
    let b = app.clone();
    let now = tenrec::now_ms();
    let lang = app.lang;
    let items: Vec<Listed> = list
        .into_iter()
        .map(|info| {
            let m = &info.meta;
            let state = serde_json::to_value(&info.state).ok().and_then(|v| v["state"].as_str().map(str::to_owned)).unwrap_or_else(|| "idle".into());
            // A chat in a worktree is listed under the project the worktree was made from.
            let home_dir = m.worktree.as_ref().map(|w| w.repo.clone()).unwrap_or_else(|| m.cwd.clone());
            let group = home_dir.display().to_string();
            let folder = home_dir.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| group.clone());
            let mut tags = Vec::new();
            if let Some(p) = &m.provider {
                tags.push(format!("→ {}", p.host));
            }
            if let Some(a) = &m.account {
                tags.push(a.name.clone());
            }
            if !m.permission_mode.is_default() {
                tags.push(format!("{}: {}", tr(lang, "perm_tag"), m.permission_mode.name()));
            }
            let active = current.as_deref() == Some(m.sid.as_str());
            let title = if m.title.is_empty() { tr(lang, "untitled") } else { m.title.clone() };
            let head = active.then(|| {
                let prov = match (&m.provider, &m.account) {
                    (Some(p), _) => format!("→ {}", p.host),
                    (None, Some(a)) => a.name.clone(),
                    _ => tr(lang, "prov_anthropic"),
                };
                (title.clone(), m.cwd.display().to_string(), m.model.clone().unwrap_or_default(), prov)
            });
            let mark = Mark { account: m.account.as_ref().map(|a| a.id.clone()), running: state == "running", pending: info.pending };
            let item = SessionItem { sid: m.sid.clone().into(), folder: folder.into(), title: title.into(), when: ago(m.created, now, lang).into(), state: state.into(), active, tags: tags.join(" · ").into(), pending: info.pending as i32, head: false, group: group.into(), open: false, count: 0, wt: m.worktree.is_some(), sub: SharedString::new() };
            (item, head, mark)
        })
        .collect();
    let _ = app.weak.upgrade_in_event_loop(move |ui| {
        // Stored before anything follows the open chat's account: the list shown is filtered from this.
        let (items, marks): (Vec<_>, Vec<_>) = items.into_iter().map(|(item, head, mark)| ((item, head), mark)).unzip();
        ALL.with(|all| *all.borrow_mut() = items.iter().map(|(item, _)| item.clone()).zip(marks).collect());
        accounts::set_chats(&ui, &b, chats);
        wt::set_chat(&ui, &b, wt_info);
        // What was read belongs to the chat that was on screen then: another one since means the next round sets them.
        if b.current.lock().expect("current").sid == read_for {
            wt::set_list(&ui, &b, &wt_rows);
            if ui.get_crumb_branch() != branch.as_str() {
                ui.set_crumb_branch(branch.into());
            }
        }
        let alias = ["default", "sonnet", "opus", "haiku", "fable", "sonnet[1m]", "opus[1m]"].iter().position(|m| Some(*m) == chat_model.as_deref().or(Some("default"))).unwrap_or(0);
        ui.set_model_idx(alias as i32);
        let perm_text = chat_perm.map(|m| format!("{}: {}", tr(lang, "perm_tag"), m.name())).unwrap_or_default();
        if ui.get_chat_perm() != perm_text.as_str() {
            ui.set_chat_perm(perm_text.into());
        }
        ui.set_chat_perm_danger(chat_perm == Some(tenrec::claude::perm::PermMode::Bypass));
        if ui.get_chat_account() != chat_acct_name.as_str() {
            ui.set_chat_account(chat_acct_name.into());
        }
        // The chat on screen has a Claude session id now (its first turn gave it one): the plug-in's snapshot is read for it once.
        if let (Some(sid), Some(csid)) = (followed.as_deref(), active_csid.as_deref()) {
            if b.current.lock().expect("current").sid.as_deref() == Some(sid) {
                team::session_known(&ui, &b, sid, csid);
            }
        }
        // The panels follow a newly opened chat's account, if that chat is still the one on screen.
        if let Some(sid) = followed.filter(|sid| b.current.lock().expect("current").sid.as_deref() == Some(sid.as_str())) {
            accounts::follow_chat(&ui, &b, &sid, chat_acct);
        }
        for (_, head) in items {
            if let Some((title, cwd, model, prov)) = head {
                ui.set_chat_title(title.into());
                ui.set_pwd_path(tail_path(&cwd, 40).into());
                ui.set_pwd_full(cwd.clone().into());
                // workspace / project: the folder's own name and the one above it.
                let trimmed = cwd.trim_end_matches(['/', '\\']);
                let mut parts = trimmed.rsplit(['/', '\\']).filter(|p| !p.is_empty());
                ui.set_crumb_proj(parts.next().unwrap_or_default().into());
                ui.set_crumb_ws(parts.next().unwrap_or_default().into());
                if !model.is_empty() && ui.get_model_name() == SharedString::from(tr(lang, "model_default")) {
                    ui.set_model_name(model_label(&model).into());
                }
                ui.set_foot_prov(prov.into());
            }
        }
        show_sessions(&ui, &b);
    });
}

/// The chat list on screen: the chats under the account shown (see `in_context`), from the last list the host sent. Called when that
/// list came and when the account shown changed, so a switch needs no round trip. It never switches the account itself.
/// The chats as the list shows them: a row per project folder (in the order of its newest chat) with its chats under it unless it is closed.
fn grouped(chats: &[SessionItem]) -> Vec<SessionItem> {
    let mut order: Vec<String> = Vec::new();
    for c in chats {
        if !order.iter().any(|g| g.as_str() == c.group.as_str()) {
            order.push(c.group.to_string());
        }
    }
    let mut out = Vec::new();
    for key in order {
        let members: Vec<&SessionItem> = chats.iter().filter(|c| c.group.as_str() == key).collect();
        let open = !COLLAPSED.with(|c| c.borrow().contains(&key));
        let parent = std::path::Path::new(&key).parent().map(|p| tail_path(&p.display().to_string(), 28)).unwrap_or_default();
        out.push(SessionItem {
            sid: SharedString::new(),
            folder: members[0].folder.clone(),
            title: SharedString::new(),
            when: SharedString::new(),
            state: SharedString::new(),
            active: false,
            tags: SharedString::new(),
            // What waits under a closed folder shows on its row.
            pending: if open { 0 } else { members.iter().map(|m| m.pending).sum() },
            head: true,
            group: key.clone().into(),
            open,
            count: members.len() as i32,
            wt: false,
            sub: parent.into(),
        });
        if open {
            out.extend(members.into_iter().cloned());
        }
    }
    out
}

pub(crate) fn show_sessions(ui: &AppWindow, app: &App) {
    let view = app.bus.account();
    let known = accounts::known();
    let chats: Vec<SessionItem> = ALL.with(|all| all.borrow().iter().filter(|(_, m)| in_context(m.account.as_deref(), view.as_deref(), known.as_deref())).map(|(item, _)| item.clone()).collect());
    let list = grouped(&chats);
    ui.set_no_chats(chats.is_empty() && known.as_ref().is_some_and(|k| !k.is_empty()));
    if !ui.get_sessions().iter().eq(list.iter().cloned()) {
        ui.set_sessions(ModelRc::from(Rc::new(VecModel::from(list))));
    }
    accounts::set_labels(ui, app);
}

/// What waits or runs under the accounts that are not shown.
pub(crate) fn attention_now(view: Option<&str>, known: Option<&[String]>) -> std::collections::HashMap<String, (usize, usize)> {
    ALL.with(|all| {
        let marks: Vec<Mark> = all.borrow().iter().map(|(_, m)| Mark { account: m.account.clone(), running: m.running, pending: m.pending }).collect();
        attention(&marks, view, known)
    })
}

#[allow(dead_code)]
fn words(lang: Lang, key: &str, n: usize) -> String {
    fill(&tr(lang, key), &[("n", n.to_string())])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(sid: &str, group: &str, wt: bool) -> SessionItem {
        SessionItem { sid: sid.into(), folder: "proj".into(), title: sid.into(), when: SharedString::new(), state: "idle".into(), active: false, tags: SharedString::new(), pending: 1, head: false, group: group.into(), open: false, count: 0, wt, sub: SharedString::new() }
    }

    #[test]
    fn chats_of_one_folder_sit_under_one_row_that_can_be_closed() {
        let chats = vec![chat("a", "/w/proj", false), chat("b", "/w/other", false), chat("c", "/w/proj", true)];
        let rows = grouped(&chats);
        let kinds: Vec<(bool, String)> = rows.iter().map(|r| (r.head, r.sid.to_string())).collect();
        assert_eq!(kinds, vec![(true, String::new()), (false, "a".into()), (false, "c".into()), (true, String::new()), (false, "b".into())]);
        assert_eq!((rows[0].count, rows[0].open, rows[0].pending), (2, true, 0));
        assert!(rows[2].wt, "a worktree chat keeps its flag");
        COLLAPSED.with(|c| c.borrow_mut().insert("/w/proj".into()));
        let rows = grouped(&chats);
        assert_eq!(rows.len(), 3, "a closed folder lists no chats");
        assert!(!rows[0].open && rows[0].pending == 2, "what waits under it shows on its row");
        COLLAPSED.with(|c| c.borrow_mut().clear());
    }
}
