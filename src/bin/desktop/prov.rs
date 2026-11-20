//! The model providers card in Settings. A key goes in and is never shown again: the form's key field is read once when the
//! operation is made, sent with `Bus::send` (not `ask`, which keeps the operation until its reply), and emptied as soon as the
//! form closes, on a save and when Settings is left. Nothing here logs an operation or puts a key in a message.
use std::cell::{Cell, RefCell};
use std::sync::Arc;

use serde_json::{json, Value};
use slint::{ComponentHandle, SharedString};
use tenrec::view::i18n::{fill, tr};
use tenrec::view::sched::{prov_op, prov_view, ProvForm};

use crate::{App, AppWindow, Prov, ProvRowV};

thread_local! {
    static LIST: RefCell<Option<Value>> = const { RefCell::new(None) };
    // The number of the save in flight, to tell its reply from another's.
    static SAVE_RID: Cell<u64> = const { Cell::new(0) };
    // The new-chat picker's entries and the id behind each (None: no provider).
    static IDS: RefCell<Vec<Option<String>>> = const { RefCell::new(Vec::new()) };
    static NAMES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    // The container fields were filled from the listing: later listings leave what is being typed alone.
    static CTR_FILLED: Cell<bool> = const { Cell::new(false) };
}

fn render(ui: &AppWindow, app: &App) {
    let Some(data) = LIST.with(|l| l.borrow().clone()) else { return };
    let (head, rows) = prov_view(&data, app.lang);
    let prov = ui.global::<Prov>();
    prov.set_notice(head.notice.into());
    prov.set_ctr_ready(head.ctr_ready);
    let rows: Vec<ProvRowV> = rows.into_iter().map(|r| ProvRowV { id: r.id.into(), name: r.name.into(), sub: r.sub.into(), has_key: r.has_key, chats: r.chats }).collect();
    crate::panels::set_rows(prov.get_rows(), rows, |m| prov.set_rows(m));
    // The new-chat picker: "none", then each provider.
    let names: Vec<String> = std::iter::once(tr(app.lang, "new_provider_none")).chain(data["providers"].as_array().into_iter().flatten().map(|p| p["name"].as_str().unwrap_or_default().to_owned())).collect();
    let ids: Vec<Option<String>> = std::iter::once(None).chain(data["providers"].as_array().into_iter().flatten().map(|p| p["id"].as_str().map(str::to_owned))).collect();
    if NAMES.with(|n| *n.borrow() != names) {
        let model: Vec<SharedString> = names.iter().map(|n| n.as_str().into()).collect();
        prov.set_new_names(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(model))));
    }
    NAMES.with(|n| *n.borrow_mut() = names);
    IDS.with(|i| *i.borrow_mut() = ids);
    if ui.get_new_prov() as usize >= IDS.with(|i| i.borrow().len()) {
        ui.set_new_prov(0);
    }
    // A provider is a tab of the header: the tabs follow the list (a new one, a new name, one that is gone).
    crate::accounts::refresh_tabs(ui, app);
    if !CTR_FILLED.with(Cell::get) {
        prov.set_ctr_runtime(head.ctr_runtime.into());
        prov.set_ctr_image(head.ctr_image.into());
        CTR_FILLED.with(|c| c.set(true));
    }
}

/// The providers as the header's tabs list them: `(id, name)`, in the order of the new-chat picker (after "none"). None before the list came.
pub fn tabs() -> Option<Vec<(String, String)>> {
    LIST.with(|l| {
        l.borrow().as_ref().map(|d| d["providers"].as_array().into_iter().flatten().filter_map(|p| Some((p["id"].as_str()?.to_owned(), p["name"].as_str()?.to_owned()))).collect())
    })
}

/// The new chat's provider from the picker: the index and the name it showed must still agree ("" means none was shown).
pub fn chosen(index: i32, name: &str) -> Result<Option<String>, ()> {
    if name.is_empty() {
        return Ok(None);
    }
    let i = index.max(0) as usize;
    if !NAMES.with(|n| n.borrow().get(i).is_some_and(|shown| shown == name)) {
        return Err(());
    }
    Ok(IDS.with(|ids| ids.borrow().get(i).cloned().flatten()))
}

/// The key field is emptied and the form closes.
pub fn leave(ui: &AppWindow) {
    let prov = ui.global::<Prov>();
    prov.set_f_key(SharedString::new());
    prov.set_f_clear(false);
    prov.set_editing(false);
}

pub fn on_reply(ui: &AppWindow, app: &Arc<App>, kind: &str, data: Value) {
    let lang = app.lang;
    match kind {
        "providers" => {
            LIST.with(|l| *l.borrow_mut() = Some(data));
            render(ui, app);
        }
        "providers_changed" => app.bus.post_quiet(json!({"op": "provider_list"})),
        "provider_result" => {
            let rid = data["rid"].as_u64().unwrap_or(0);
            let op = app.bus.take(rid).unwrap_or(Value::Null);
            if data["ok"] != true {
                // The server's reason only; the form keeps what was typed (the key included) so a typo can be fixed.
                if let Some(error) = data["error"].as_str().filter(|e| !e.is_empty()) {
                    crate::toast(ui, error.to_owned());
                }
                return;
            }
            if rid != 0 && rid == SAVE_RID.with(Cell::get) {
                leave(ui);
                crate::toast(ui, tr(lang, "pv_saved"));
            } else if op["op"] == "provider_container" {
                CTR_FILLED.with(|c| c.set(false));
                app.bus.post_quiet(json!({"op": "provider_list"}));
            }
        }
        _ => {}
    }
}

pub fn wire(ui: &AppWindow, app: &Arc<App>) {
    let prov = ui.global::<Prov>();
    // Asked at the start: the new-chat form offers the providers before Settings is ever opened.
    app.bus.post_quiet(json!({"op": "provider_list"}));
    let weak = ui.as_weak();
    prov.on_new(move || {
        let Some(ui) = weak.upgrade() else { return };
        let p = ui.global::<Prov>();
        p.set_edit_id(SharedString::new());
        for set in [Prov::set_f_name, Prov::set_f_url, Prov::set_f_key, Prov::set_f_model, Prov::set_f_opus, Prov::set_f_sonnet, Prov::set_f_haiku, Prov::set_f_headers, Prov::set_f_oauth] {
            set(&p, SharedString::new());
        }
        p.set_f_auth(0);
        p.set_f_clear(false);
        p.set_f_has_key(false);
        p.set_f_clear_oauth(false);
        p.set_f_has_oauth(false);
        p.set_editing(true);
    });
    let weak = ui.as_weak();
    prov.on_edit(move |id| {
        let Some(ui) = weak.upgrade() else { return };
        let one = LIST.with(|l| l.borrow().as_ref().and_then(|d| d["providers"].as_array().and_then(|list| list.iter().find(|p| p["id"] == id.as_str()).cloned())));
        let Some(one) = one else { return };
        let text = |v: &Value| SharedString::from(v.as_str().unwrap_or_default());
        let p = ui.global::<Prov>();
        p.set_edit_id(id);
        p.set_f_name(text(&one["name"]));
        p.set_f_url(text(&one["base_url"]));
        p.set_f_auth(match one["auth"].as_str() { Some("x_api_key") => 1, Some("headers") => 2, Some("oauth_token") => 3, _ => 0 });
        // The key is never sent back: the field starts empty and an empty field keeps the saved key.
        p.set_f_key(SharedString::new());
        p.set_f_clear(false);
        p.set_f_has_key(one["has_token"] == true);
        // The subscription token is never sent back either.
        p.set_f_oauth(SharedString::new());
        p.set_f_clear_oauth(false);
        p.set_f_has_oauth(one["has_oauth"] == true);
        p.set_f_model(text(&one["model"]));
        p.set_f_opus(text(&one["models"]["opus"]));
        p.set_f_sonnet(text(&one["models"]["sonnet"]));
        p.set_f_haiku(text(&one["models"]["haiku"]));
        p.set_f_headers(text(&one["headers"]));
        p.set_editing(true);
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    prov.on_save(move || {
        let Some(ui) = weak.upgrade() else { return };
        let p = ui.global::<Prov>();
        let (id, name, url, key, model) = (p.get_edit_id().to_string(), p.get_f_name().to_string(), p.get_f_url().to_string(), p.get_f_key().to_string(), p.get_f_model().to_string());
        let aliases = [p.get_f_opus().to_string(), p.get_f_sonnet().to_string(), p.get_f_haiku().to_string()];
        let op = prov_op(&ProvForm { id: Some(id.as_str()), name: &name, url: &url, auth: p.get_f_auth(), key: &key, clear: p.get_f_clear(), model: &model, aliases: [&aliases[0], &aliases[1], &aliases[2]], headers: &p.get_f_headers().to_string(), oauth: &p.get_f_oauth().to_string(), clear_oauth: p.get_f_clear_oauth() });
        // `send`, not `ask`: the operation (the key in it) is not kept waiting for its reply.
        let rid = a.bus.send(op);
        SAVE_RID.with(|r| r.set(rid));
    });
    let weak = ui.as_weak();
    prov.on_cancel(move || {
        if let Some(ui) = weak.upgrade() {
            leave(&ui);
        }
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    prov.on_delete(move |id| {
        let Some(ui) = weak.upgrade() else { return };
        let chats = LIST.with(|l| l.borrow().as_ref().and_then(|d| d["providers"].as_array().and_then(|list| list.iter().find(|p| p["id"] == id.as_str()).and_then(|p| p["chats"].as_u64())))).unwrap_or(0);
        let (b, id) = (a.clone(), id.to_string());
        crate::panels::ask(&ui, fill(&tr(a.lang, "pv_delete_confirm"), &[("n", chats.to_string())]), tr(a.lang, "pv_delete"), true, Box::new(move |ok| {
            if ok {
                b.bus.post(json!({"op": "provider_delete", "id": id}));
            }
        }));
    });
    let (a, weak) = (app.clone(), ui.as_weak());
    prov.on_save_container(move || {
        let Some(ui) = weak.upgrade() else { return };
        let p = ui.global::<Prov>();
        a.bus.post(json!({"op": "provider_container", "runtime": p.get_ctr_runtime().trim(), "image": p.get_ctr_image().trim()}));
    });
}
