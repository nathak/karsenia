//! The file tree and editor operations on the WebSocket: `fs_list`, `fs_read`, `fs_write`, `fs_check` (the version of a file on disk, no text), `fs_create`, `fs_delete` (into the project's trash), `fs_restore`, `fs_purge`, `fs_empty_trash` and `fs_rename`. The project folder always comes from the
//! session on the server (never from the page), an answer goes only to the socket that asked, and file text is never logged or stored.
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::project_fs::{self, Changed, Purged, Saved};
use super::{plain, AppState};
use crate::claude::container::{holds_data, holds_home};

/// Whether the editor may be used on this project folder at all: not the app's data folder, a place that holds it, the home folder
/// or a folder that holds it, nor a place in the home folder that keeps credentials and settings. Without a known home folder the
/// last rules cannot be applied, so the folder may be read but not written. Returns the real path that was checked: the operations
/// that follow use that path and do not resolve the project's name again (a folder swapped for a link in between would be taken
/// for the project).
pub(super) fn check_project(project: &Path, data_dir: &Path, home: Option<&Path>, write: bool) -> Result<PathBuf, String> {
    let real = project.canonicalize().map_err(|_| "the project folder cannot be opened".to_owned())?;
    // Compared in the same form as the data folder's (on Windows `canonicalize` gives a `\\?\` path and `platform::canon` does not).
    holds_data(&crate::platform::canon(&real).unwrap_or_else(|_| real.clone()), data_dir).map_err(|_| "the editor is not available for the app's data folder or a folder that holds it".to_owned())?;
    match home.and_then(|h| h.canonicalize().ok()) {
        Some(home) => holds_home(&real, &home).map(|_| real).map_err(|_| "the editor is not available for the home folder, a folder that holds it, or a place where credentials and settings are kept".to_owned()),
        None if write => Err("the home folder is not known, so saving cannot be checked".to_owned()),
        None => Ok(real),
    }
}

pub fn handle(state: &AppState, name: &str, op: &Value, out: &UnboundedSender<String>) {
    let sid = op["sid"].as_str().unwrap_or_default().to_owned();
    let req = op["req"].clone();
    let path = op["path"].as_str().unwrap_or_default().to_owned();
    let failure = {
        let (out, sid, req, path) = (out.clone(), sid.clone(), req.clone(), path.clone());
        move |message: String| {
            let _ = out.send(plain("fs_error", Some(&sid), json!({"req": req, "path": path, "message": message})));
        }
    };
    let name = name.to_owned();
    let (registry, out) = (state.registry.clone(), out.clone());
    let text = op["text"].as_str().map(str::to_owned);
    let base = op["base"].as_str().unwrap_or_default().to_owned();
    let confirm = op["confirm"].as_bool() == Some(true);
    let kind_arg = op["kind"].as_str().unwrap_or_default().to_owned();
    let to = op["to"].as_str().unwrap_or_default().to_owned();
    let delete_base = op["base"].as_str().map(str::to_owned);
    // Disk work runs beside the socket's other messages and never on its own task.
    tokio::spawn(async move {
        let Some(handle) = registry.get(&sid) else { return failure("no such session".to_owned()) };
        let Some(info) = handle.info().await else { return failure("no such session".to_owned()) };
        let project: PathBuf = info.meta.cwd.clone();
        let data_dir = registry.dir().to_path_buf();
        let home = crate::platform::home_dir();
        let write = matches!(name.as_str(), "fs_write" | "fs_create" | "fs_delete" | "fs_rename" | "fs_restore" | "fs_purge" | "fs_empty_trash");
        let reply = {
            let path = path.clone();
            tokio::task::spawn_blocking(move || -> Result<(&'static str, Value), String> {
                let root = check_project(&project, &data_dir, home.as_deref(), write)?;
                let (kind, mut data) = match name.as_str() {
                    "fs_list" => ("fs_listing", serde_json::to_value(project_fs::list_dir_in(&root, &path)?).unwrap_or(Value::Null)),
                    "fs_read" => ("fs_text", serde_json::to_value(project_fs::read_text_in(&root, &path)?).unwrap_or(Value::Null)),
                    "fs_check" => ("fs_checked", json!({"version": project_fs::version_in(&root, &path)?})),
                    "fs_create" => match project_fs::create_in(&root, &path, &kind_arg, confirm)? {
                        Changed::Done | Changed::Moved(_) => ("fs_created", json!({"kind": kind_arg})),
                        Changed::NeedsConfirm(reason) => ("fs_needs_confirm", json!({"reason": reason})),
                    },
                    "fs_delete" => match project_fs::delete_in(&root, &path, delete_base.as_deref(), confirm)? {
                        Changed::Done => ("fs_deleted", json!({})),
                        Changed::Moved(trashed) => ("fs_deleted", json!({"trashed": trashed})),
                        Changed::NeedsConfirm(reason) => ("fs_needs_confirm", json!({"reason": reason})),
                    },
                    "fs_restore" => match project_fs::restore_in_to(&root, &path, confirm)? {
                        (Changed::Moved(to), _) => ("fs_restored", json!({"to": to})),
                        (Changed::Done, _) => ("fs_restored", json!({"to": path})),
                        // The place it would go to is told: it is what the person is asked about.
                        (Changed::NeedsConfirm(reason), to) => ("fs_needs_confirm", json!({"reason": reason, "to": to})),
                    },
                    "fs_purge" | "fs_empty_trash" => {
                        let purged = if name == "fs_purge" { project_fs::purge_in(&root, &path, confirm)? } else { project_fs::empty_trash_in(&root, &path, confirm)? };
                        match purged {
                            Purged::Summary { entries, items, bytes, left } => ("fs_purge_summary", json!({"entries": entries, "items": items, "bytes": bytes, "left": left})),
                            Purged::Done { removed, left } => ("fs_purged", json!({"removed": removed, "left": left})),
                        }
                    }
                    "fs_rename" => match project_fs::rename_in(&root, &path, &to, confirm)? {
                        Changed::Done | Changed::Moved(_) => ("fs_renamed", json!({"to": to})),
                        Changed::NeedsConfirm(reason) => ("fs_needs_confirm", json!({"reason": reason})),
                    },
                    "fs_write" => {
                        let text = text.ok_or_else(|| "no text to save".to_owned())?;
                        match project_fs::write_text_in(&root, &path, &text, &base, confirm)? {
                            Saved::Saved { version } => ("fs_saved", json!({"version": version})),
                            Saved::Conflict { text, version } => ("fs_conflict", json!({"text": text, "version": version})),
                            Saved::NeedsConfirm(reason) => ("fs_needs_confirm", json!({"reason": reason})),
                        }
                    }
                    _ => return Err("unknown op".to_owned()),
                };
                data["req"] = req;
                data["path"] = Value::String(path);
                Ok((kind, data))
            })
            .await
        };
        match reply {
            Ok(Ok((kind, data))) => {
                let _ = out.send(plain(kind, Some(&sid), data));
            }
            Ok(Err(message)) => failure(message),
            Err(_) => failure("the file could not be handled".to_owned()),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_editor_refuses_the_places_a_container_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home").join("me");
        let data = home.join(".local").join("share").join("app");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(home.join("work")).unwrap();
        for write in [false, true] {
            assert!(check_project(&home.join("work"), &data, Some(&home), write).is_ok());
            for bad in [&data, data.parent().unwrap(), &home, home.parent().unwrap(), &home.join(".ssh")] {
                assert!(check_project(bad, &data, Some(&home), write).is_err(), "{}", bad.display());
            }
        }
        assert!(check_project(&home.join("work"), &data, None, false).is_ok(), "reading needs no home folder");
        assert!(check_project(&home.join("work"), &data, None, true).is_err(), "saving does");
    }
}
