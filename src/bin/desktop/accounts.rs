//! The Accounts card in Settings and the account the panels show. The card speaks the web page's `account_*` operations;
//! the panels follow the open chat's account (set when another chat is opened) and can be switched by hand.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{json, Value};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tenrec::view::i18n::{fill, tr};
use tenrec::view::panels::{accounts_view, presets, tab_list, tab_label, Ctx, Known};

use crate::{Acct, AcctRow, App, AppWindow, PlugRow};

thread_local! {
    static LIST: RefCell<Option<Value>> = const { RefCell::new(None) };
    // Per account ("" for the default one): the number of the last status asked and its answer (None while asking).
    static STATUS: RefCell<HashMap<String, (u64, Option<Value>)>> = RefCell::new(HashMap::new());
    static CHATS: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
    // The picker's entries and the id behind each (None: the default account).
    static IDS: RefCell<Vec<Option<String>>> = const { RefCell::new(Vec::new()) };
    // The header's tabs: each account (the picker's `IDS`) and then each provider, with the text on it. The provider shown (a provider tab
    // is chosen), if one is: the account the panels show is then the default one (a provider chat has no account of its own).
    static TABS: RefCell<Vec<(Ctx, String)>> = const { RefCell::new(Vec::new()) };
    static PROVIDER: RefCell<Option<String>> = const { RefCell::new(None) };
    // What the header's tabs say (the names with what waits or runs under each).
    static LABELS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static NAMES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    // The chat whose account the panels were last switched to.
    static APPLIED_SID: RefCell<Option<String>> = const { RefCell::new(None) };
    // The login command of each account, for the copy button.
    static LOGINS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    // The ids of the Codex accounts: they are not in the picker (`IDS`), but their login is checked like the others'.
    static CODEX_IDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    // Plug-ins: the cards whose section is open, the host's last answer per account, and the installs under way ("account/plugin").
    static PLUG_OPEN: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static PLUGS: RefCell<HashMap<String, Value>> = RefCell::new(HashMap::new());
    static PLUG_BUSY: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// The gateways of an account: the providers that use its login (the default account is the id ""), for its card.
fn gateways_of(account: &str) -> ModelRc<crate::GwRow> {
    let rows: Vec<crate::GwRow> = crate::prov::login_rows()
        .into_iter()
        .filter(|(_, _, _, login_account)| login_account.as_deref().unwrap_or_default() == account)
        .map(|(id, name, host, _)| crate::GwRow { id: id.into(), name: name.into(), host: host.into() })
        .collect();
    ModelRc::from(std::rc::Rc::new(VecModel::from(rows)))
}

/// What an account card shows of its plug-ins section: whether it is open, a line of text (checking, nothing, an error) and the offer.
fn plug_view(id: &str, lang: tenrec::view::i18n::Lang) -> (bool, String, Vec<PlugRow>) {
    let open = PLUG_OPEN.with(|o| o.borrow().contains(id));
    if !open {
        return (false, String::new(), Vec::new());
    }
    let Some(info) = PLUGS.with(|p| p.borrow().get(id).cloned()) else { return (true, tr(lang, "plug_checking"), Vec::new()) };
    if let Some(error) = info["error"].as_str() {
        return (true, error.to_owned(), Vec::new());
    }
    let offer: Vec<PlugRow> = info["offer"].as_array().map(|a| a.iter().filter_map(|p| p["id"].as_str()).map(|p| PlugRow { id: p.into(), busy: PLUG_BUSY.with(|b| b.borrow().contains(&format!("{id}/{p}"))) }).collect()).unwrap_or_default();
    let note = if offer.is_empty() { tr(lang, "plug_none") } else { String::new() };
    (true, note, offer)
}

pub fn render(ui: &AppWindow, app: &App) {
    let Some(list) = LIST.with(|l| l.borrow().clone()) else { return };
    let status: HashMap<String, Option<Value>> = STATUS.with(|s| s.borrow().iter().map(|(k, (_, v))| (k.clone(), v.clone())).collect());
    let (head, rows) = CHATS.with(|c| accounts_view(&list, &status, &c.borrow(), app.lang));
    let acct = ui.global::<Acct>();
    acct.set_notice(head.notice.into());
    acct.set_can_add(head.can_add);
    LOGINS.with(|l| *l.borrow_mut() = rows.iter().map(|r| (r.id.clone(), r.login.clone())).collect());
    CODEX_IDS.with(|c| *c.borrow_mut() = rows.iter().filter(|r| r.kind == "codex").map(|r| r.id.clone()).collect());
    let slint_rows: Vec<AcctRow> = rows
        .into_iter()
        .map(|r| {
            let (usage, usage_level) = crate::usage::compact(&r.id, app);
            let (plug_open, plug_note, plugs) = plug_view(&r.id, app.lang);
            AcctRow { gws: gateways_of(&r.id), id: r.id.into(), name: r.name.into(), sub: r.sub.into(), state: r.state.into(), chats: r.chats as i32, open: r.open, is_default: r.is_default, usage: usage.into(), usage_level: usage_level.into(), kind: r.kind.into(), login: r.login.into(), dir: r.dir.into(), plug_open, plug_note: plug_note.into(), plugs: ModelRc::from(std::rc::Rc::new(VecModel::from(plugs))) }
        })
        .collect();
    crate::panels::set_rows(acct.get_rows(), slint_rows, |m| acct.set_rows(m));
    let names: Vec<SharedString> = head.names.iter().map(|n| n.into()).collect();
    if NAMES.with(|n| *n.borrow() != head.names) {
        acct.set_names(ModelRc::from(std::rc::Rc::new(VecModel::from(names))));
    }
    NAMES.with(|n| *n.borrow_mut() = head.names);
    IDS.with(|i| *i.borrow_mut() = head.ids.clone());
    // An account that is gone is not shown any more.
    let current = app.bus.account();
    if current.is_some() && !head.ids.contains(&current) {
        set_account(ui, app, None);
    }
    refresh_tabs(ui, app);
    crate::usage::render(ui, app);
    // The new chat's picker stays inside the list.
    if ui.get_new_acct() as usize >= head.ids.len() {
        ui.set_new_acct(0);
    }
}

/// The accounts whose login is checked: the default one (""), the Claude ones in the picker and the Codex ones.
fn status_keys() -> Vec<String> {
    std::iter::once(String::new()).chain(IDS.with(|i| i.borrow().iter().flatten().cloned().collect::<Vec<_>>())).chain(CODEX_IDS.with(|c| c.borrow().clone())).collect()
}

/// What kind of account this id is (the default account, and one that is not listed, are Claude ones).
fn kind_of(id: &str) -> tenrec::accounts::AccountKind {
    LIST.with(|l| l.borrow().as_ref().and_then(|l| l["accounts"].as_array().and_then(|list| list.iter().find(|x| x["id"] == id).and_then(|x| x["kind"].as_str().and_then(|k| tenrec::accounts::AccountKind::parse(k).ok()))))).unwrap_or_default()
}

/// Every account id the host listed, or None before the list came (an extra account's chat is then only under its own account).
pub fn known() -> Option<Vec<String>> {
    IDS.with(|i| {
        let ids = i.borrow();
        (!ids.is_empty()).then(|| ids.iter().flatten().cloned().collect())
    })
}

/// The accounts and the providers the host listed (each None before its list came): what a chat is told apart from a deleted one by.
pub fn known_all() -> (Option<Vec<String>>, Option<Vec<String>>) {
    (known(), crate::prov::tabs().map(|t| t.into_iter().map(|(id, _)| id).collect()))
}

/// The tab shown: a provider when one was chosen, else the account the panels show.
pub fn ctx(app: &App) -> Ctx {
    match PROVIDER.with(|p| p.borrow().clone()) {
        Some(p) => Ctx::Provider(p),
        None => Ctx::Account(app.bus.account()),
    }
}

/// The tabs again from the accounts and the providers listed (either list changed), then what hangs on them: a provider tab whose provider
/// is gone is left for the default account, the picker and the list follow.
pub fn refresh_tabs(ui: &AppWindow, app: &App) {
    let providers = crate::prov::tabs();
    let tabs = tab_list(&IDS.with(|i| i.borrow().clone()), &NAMES.with(|n| n.borrow().clone()), providers.as_deref().unwrap_or_default());
    TABS.with(|t| *t.borrow_mut() = tabs);
    let gone = PROVIDER.with(|p| p.borrow().as_ref().is_some_and(|id| providers.as_ref().is_some_and(|list| !list.iter().any(|(pid, _)| pid == id))));
    if gone {
        set_ctx(ui, app, Ctx::Account(None));
    }
    sync_view(ui, app);
    crate::show_sessions(ui, app);
}

/// The header's tab names, each with "⚠ n" (a chat there waits for the person) or "●" (one is working) when it is not the tab shown.
pub fn set_labels(ui: &AppWindow, app: &App) {
    let view = ctx(app);
    let (accounts, providers) = known_all();
    let seen = crate::attention_now(&view, Known { accounts: accounts.as_deref(), providers: providers.as_deref() });
    let labels: Vec<String> = TABS.with(|t| t.borrow().iter().map(|(c, name)| tab_label(name, seen.get(c))).collect());
    if LABELS.with(|l| *l.borrow() == labels) {
        return;
    }
    ui.global::<Acct>().set_labels(ModelRc::from(std::rc::Rc::new(VecModel::from(labels.iter().map(|l| l.into()).collect::<Vec<SharedString>>()))));
    LABELS.with(|l| *l.borrow_mut() = labels);
    // A picker that lost its model may have lost its place.
    sync_view(ui, app);
}

/// Points the header's picker at the tab shown, and the new-chat form's presets (account, provider) at what that tab means.
fn sync_view(ui: &AppWindow, app: &App) {
    let view = ctx(app);
    let (index, (pre_acct, pre_prov)) = TABS.with(|t| {
        let tabs = t.borrow();
        (tabs.iter().position(|(c, _)| *c == view).unwrap_or(0), presets(&tabs, &view))
    });
    let acct = ui.global::<Acct>();
    acct.set_view(index as i32);
    acct.set_pre_acct(pre_acct as i32);
    acct.set_pre_prov(pre_prov as i32);
}

/// Switches the tab shown. An account (or the default one) changes what the panels show: what was shown of the old one goes, and the open
/// panel asks again. A provider is shown with the default account's panels (a provider chat has none of its own); only the list changes.
pub fn set_ctx(ui: &AppWindow, app: &App, view: Ctx) {
    let (account, provider) = match view {
        Ctx::Account(id) => (id, None),
        Ctx::Provider(p) => (None, Some(p)),
    };
    let provider_changed = PROVIDER.with(|p| p.replace(provider.clone())) != provider;
    let account_changed = app.bus.set_account(account);
    sync_view(ui, app);
    if provider_changed || account_changed {
        crate::show_sessions(ui, app);
    }
    if account_changed {
        crate::usage::render(ui, app);
        crate::panels::account_changed(ui, app);
    }
}

/// Switches the panels to another account (see `set_ctx`).
pub fn set_account(ui: &AppWindow, app: &App, id: Option<String>) {
    set_ctx(ui, app, Ctx::Account(id));
}

pub fn ask_status(ui: &AppWindow, app: &App, key: &str) {
    let id = (!key.is_empty()).then(|| key.to_owned());
    let n = app.bus.send(json!({"op": "account_status", "id": id}));
    STATUS.with(|s| s.borrow_mut().insert(key.to_owned(), (n, None)));
    render(ui, app);
}

/// Settings opened: the accounts never checked in this run are checked once.
pub fn panel_open(ui: &AppWindow, app: &App) {
    app.bus.post_quiet(json!({"op": "account_list"}));
    for key in status_keys() {
        if STATUS.with(|s| !s.borrow().contains_key(&key)) {
            ask_status(ui, app, &key);
        }
    }
}

/// The chats of each account changed (from the session list).
pub fn set_chats(ui: &AppWindow, app: &App, chats: HashMap<String, usize>) {
    if CHATS.with(|c| *c.borrow() == chats) {
        return;
    }
    CHATS.with(|c| *c.borrow_mut() = chats);
    render(ui, app);
}

/// Another chat was opened: the tab follows it (once; a hand-picked tab stays until the next chat). A provider chat goes to its provider's
/// tab; a chat of an account to that account's.
pub fn follow_chat(ui: &AppWindow, app: &App, sid: &str, account: Option<String>, provider: Option<String>) {
    if APPLIED_SID.with(|a| a.borrow().as_deref() == Some(sid)) {
        return;
    }
    APPLIED_SID.with(|a| *a.borrow_mut() = Some(sid.to_owned()));
    // A provider that was deleted leaves its chat to the default account (the provider list may not have come yet: it is then taken as it is).
    if let Some(p) = provider {
        if crate::prov::tabs().is_none_or(|list| list.iter().any(|(id, _)| *id == p)) {
            set_ctx(ui, app, Ctx::Provider(p));
            return;
        }
    }
    // A chat of an account that was deleted: the panels show the default account, not one that is gone (the list may not have
    // come yet, then the account is taken as it is and `render` corrects it).
    let account = account.filter(|id| IDS.with(|ids| ids.borrow().is_empty() || ids.borrow().contains(&Some(id.clone()))));
    set_account(ui, app, account);
}

/// The account id at an index of the account picker (None outside the list; Some(None) is the default account), with the name it showed.
pub fn id_at(index: i32, name: &str) -> Option<Option<String>> {
    let i = index.max(0) as usize;
    let same = NAMES.with(|n| n.borrow().get(i).map(String::as_str) == Some(name));
    same.then(|| IDS.with(|ids| ids.borrow().get(i).cloned().flatten()))
}

/// The index of an account in the picker (the default account, None, is 0; an account that is gone is 0 too).
pub fn index_of(id: Option<&str>) -> i32 {
    IDS.with(|ids| ids.borrow().iter().position(|one| one.as_deref() == id).unwrap_or(0)) as i32
}

/// The new chat's account from the picker: the index and the name it showed must still agree.
pub fn chosen(index: i32, name: &str) -> Result<Option<String>, ()> {
    if name.is_empty() {
        return Ok(None);
    }
    let i = index.max(0) as usize;
    let same = NAMES.with(|n| n.borrow().get(i).map(String::as_str) == Some(name));
    if !same {
        return Err(());
    }
    Ok(IDS.with(|ids| ids.borrow().get(i).cloned().flatten()))
}

pub fn on_reply(ui: &AppWindow, app: &Arc<App>, kind: &str, data: Value) {
    let lang = app.lang;
    match kind {
        "accounts" => {
            LIST.with(|l| *l.borrow_mut() = Some(data));
            render(ui, app);
            // Settings is open and an account has not been checked yet (the list came late, or an account was just added).
            if ui.get_side() == 5 || ui.get_new_open() {
                for key in status_keys().into_iter().filter(|k| !k.is_empty()) {
                    if STATUS.with(|s| !s.borrow().contains_key(&key)) {
                        ask_status(ui, app, &key);
                    }
                }
            }
        }
        "accounts_changed" => app.bus.post_quiet(json!({"op": "account_list"})),
        "account_status" => {
            let key = data["id"].as_str().unwrap_or_default().to_owned();
            let req = data["req"].as_u64().unwrap_or(0);
            let fresh = STATUS.with(|s| s.borrow().get(&key).is_some_and(|(n, _)| *n == req));
            if fresh {
                STATUS.with(|s| s.borrow_mut().insert(key, (req, Some(data))));
                render(ui, app);
            }
        }
        "account_plugins" => {
            if let Some(id) = data["account"].as_str().map(str::to_owned) {
                PLUGS.with(|p| p.borrow_mut().insert(id, data));
                render(ui, app);
            }
        }
        "account_result" => {
            let op = data["rid"].as_u64().and_then(|rid| app.bus.take(rid)).unwrap_or(Value::Null);
            if op["op"] == "account_plugin_install" {
                let (id, plugin) = (op["id"].as_str().unwrap_or_default().to_owned(), op["plugin"].as_str().unwrap_or_default().to_owned());
                PLUG_BUSY.with(|b| b.borrow_mut().remove(&format!("{id}/{plugin}")));
                let text = if data["ok"] == true { format!("{}: {plugin}", tr(lang, "plug_done")) } else { format!("{}: {}", tr(lang, "plug_failed"), data["error"].as_str().unwrap_or_default()) };
                crate::toast(ui, text);
                // What the account has now: the offer is asked for again.
                app.bus.post_quiet(json!({"op": "account_plugins", "id": id}));
                render(ui, app);
                return;
            }
            if data["ok"] != true {
                if let Some(error) = data["error"].as_str() {
                    crate::toast(ui, error.to_owned());
                }
                return;
            }
            let acct = ui.global::<Acct>();
            match op["op"].as_str() {
                Some("account_add") => {
                    acct.set_new_name(SharedString::new());
                    crate::toast(ui, fill(&tr(lang, "acct_added"), &[("name", op["name"].as_str().unwrap_or_default().to_owned())]));
                }
                Some("account_rename") => acct.set_editing(SharedString::new()),
                Some("account_delete") => {
                    let warnings: Vec<String> = data["warnings"].as_array().map(|w| w.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                    let mut text = tr(lang, "acct_deleted");
                    if !warnings.is_empty() {
                        text = format!("{text}: {}", warnings.join("; "));
                    }
                    crate::toast(ui, text);
                }
                _ => {}
            }
        }
        _ => {}
    }
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let acct = ui.global::<Acct>();
    acct.set_console(cfg!(windows));
    app.bus.post_quiet(json!({"op": "account_list"}));

    let (a, weak) = (app.clone(), ui.as_weak());
    acct.on_status(move |id| {
        if let Some(ui) = weak.upgrade() {
            ask_status(&ui, &a, &id);
        }
    });
    let a = app.clone();
    acct.on_add(move |name, kind| a.bus.post(json!({"op": "account_add", "name": name.trim(), "kind": kind.as_str()})));
    let a = app.clone();
    acct.on_rename(move |id, name| a.bus.post(json!({"op": "account_rename", "id": id.as_str(), "name": name.trim()})));
    let (a, weak) = (app.clone(), ui.as_weak());
    acct.on_delete(move |id| {
        let Some(ui) = weak.upgrade() else { return };
        let name = LIST.with(|l| l.borrow().as_ref().and_then(|l| l["accounts"].as_array().and_then(|list| list.iter().find(|x| x["id"] == id.as_str()).map(|x| x["name"].as_str().unwrap_or_default().to_owned())))).unwrap_or_default();
        let n = CHATS.with(|c| c.borrow().get(id.as_str()).copied().unwrap_or(0));
        let body = match kind_of(id.as_str()) {
            tenrec::accounts::AccountKind::Claude => fill(&tr(a.lang, "acct_delete_confirm"), &[("name", name), ("n", n.to_string())]),
            tenrec::accounts::AccountKind::Codex => fill(&tr(a.lang, "acct_delete_confirm_codex"), &[("name", name)]),
        };
        let (b, id) = (a.clone(), id.to_string());
        crate::panels::ask(&ui, body, tr(a.lang, "acct_delete"), true, Box::new(move |ok| {
            if ok {
                b.bus.post(json!({"op": "account_delete", "id": id, "confirm": true}));
            }
        }));
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    acct.on_plugins_toggle(move |id| {
        let Some(ui) = weak.upgrade() else { return };
        let id = id.to_string();
        let opened = PLUG_OPEN.with(|o| {
            let mut o = o.borrow_mut();
            if o.remove(&id) { false } else { o.insert(id.clone()) }
        });
        if opened {
            a.bus.post_quiet(json!({"op": "account_plugins", "id": id}));
        }
        render(&ui, &a);
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    acct.on_plugin_install(move |id, plugin| {
        let Some(ui) = weak.upgrade() else { return };
        let (id, plugin) = (id.to_string(), plugin.to_string());
        let body = fill(&tr(a.lang, "plug_confirm"), &[("p", plugin.clone())]);
        let b = a.clone();
        crate::panels::ask(&ui, body, tr(a.lang, "plug_install"), false, Box::new(move |ok| {
            if ok {
                PLUG_BUSY.with(|s| s.borrow_mut().insert(format!("{id}/{plugin}")));
                b.bus.post(json!({"op": "account_plugin_install", "id": id, "plugin": plugin}));
                let weak = b.weak.clone();
                let c = b.clone();
                let _ = weak.upgrade_in_event_loop(move |ui| render(&ui, &c));
            }
        }));
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    acct.on_copy_login(move |id| {
        let Some(login) = LOGINS.with(|l| l.borrow().get(id.as_str()).cloned()) else { return };
        if crate::copy_quiet(login) {
            if let Some(ui) = weak.upgrade() {
                crate::toast(&ui, tr(a.lang, "acct_copied"));
            }
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    acct.on_copy_path(move |path| {
        if crate::copy_quiet(path.to_string()) {
            if let Some(ui) = weak.upgrade() {
                crate::toast(&ui, tr(a.lang, "acct_copied"));
            }
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    acct.on_login(move |id| {
        let Some(ui) = weak.upgrade() else { return };
        login(&ui, &a, id.to_string());
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    acct.on_view_chosen(move |i| {
        let Some(ui) = weak.upgrade() else { return };
        let Some(view) = TABS.with(|t| t.borrow().get(i.max(0) as usize).map(|(c, _)| c.clone())) else { return };
        set_ctx(&ui, &a, view);
    });
}

/// "Open login window": a console of its own runs `claude auth login` (or `codex login`) for the account; when it closes, the status is
/// checked again. Nothing it prints passes through the app.
#[cfg(windows)]
fn login(ui: &AppWindow, app: &Arc<App>, key: String) {
    use tenrec::accounts::AccountKind;
    let kind = if key.is_empty() { AccountKind::Claude } else { kind_of(&key) };
    let failed = |error: String| crate::toast(ui, fill(&tr(app.lang, "acct_login_failed"), &[("e", error)]));
    let (bin, account) = match (kind, key.is_empty()) {
        (AccountKind::Claude, true) => (app.registry.bin().to_owned(), None),
        (AccountKind::Claude, false) => match app.registry.account_login(&key) {
            Ok(found) => (app.registry.bin().to_owned(), Some(found)),
            Err(error) => return failed(error),
        },
        (AccountKind::Codex, _) => match app.registry.codex_home(&key) {
            Ok(home) => {
                let env = tenrec::codex::plan(&tenrec::claude::env::inherited_names(), &home);
                // The full path of the program (a `.cmd` shim is named by it, so the shell does not pick a `.ps1` that a policy refuses); the
                // name only when none was found.
                let bin = tenrec::codex::bin(std::env::var_os("PATH").as_deref()).and_then(|b| b.to_str().map(str::to_owned)).unwrap_or_else(|| "codex".to_owned());
                (bin, Some((home, env)))
            }
            Err(error) => return failed(error),
        },
    };
    match open_login(&bin, kind, &tr(app.lang, "acct_login_wait"), account) {
        Ok(mut child) => {
            crate::toast(ui, tr(app.lang, "acct_login_started"));
            let a = app.clone();
            std::thread::spawn(move || {
                let _ = child.wait();
                let b = a.clone();
                let _ = a.weak.upgrade_in_event_loop(move |ui| ask_status(&ui, &b, &key));
            });
        }
        Err(error) => crate::toast(ui, fill(&tr(app.lang, "acct_login_failed"), &[("e", error.to_string())])),
    }
}

#[cfg(not(windows))]
fn login(ui: &AppWindow, app: &Arc<App>, _key: String) {
    crate::toast(ui, fill(&tr(app.lang, "acct_login_failed"), &[("e", "Windows only".to_owned())]));
}

/// The console's command line is fixed: the program and the account's folder go in as environment variables, never into the
/// text PowerShell reads, so no path needs quoting.
#[cfg(windows)]
fn open_login(bin: &str, kind: tenrec::accounts::AccountKind, wait: &str, account: Option<(std::path::PathBuf, tenrec::claude::env::EnvPlan)>) -> std::io::Result<std::process::Child> {
    use tenrec::accounts::AccountKind;
    use std::os::windows::process::CommandExt;
    let ps = std::env::var_os("SystemRoot").map(|r| std::path::PathBuf::from(r).join(r"System32\WindowsPowerShell\v1.0\powershell.exe")).unwrap_or_else(|| "powershell.exe".into());
    let mut cmd = std::process::Command::new(ps);
    // A fixed line for each kind: nothing of the account goes into it.
    let script = match kind {
        AccountKind::Claude => "& $env:TENREC_LOGIN_BIN auth login; if (-not $?) { Read-Host $env:TENREC_LOGIN_WAIT }",
        AccountKind::Codex => "& $env:TENREC_LOGIN_BIN login; if (-not $?) { Read-Host $env:TENREC_LOGIN_WAIT }",
    };
    cmd.args(["-NoLogo", "-NoProfile", "-Command", script]).env("TENREC_LOGIN_BIN", bin).env("TENREC_LOGIN_WAIT", wait);
    if let Some((dir, env)) = account {
        for name in &env.remove {
            cmd.env_remove(name);
        }
        for (name, value) in &env.set {
            cmd.env(name, value);
        }
        cmd.current_dir(dir);
    }
    cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NEW_CONSOLE);
    cmd.spawn()
}
