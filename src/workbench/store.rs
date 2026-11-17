//! A chat's workbench file: `<data>/workbench/chat/<sid>.json`. Written whole beside the file and renamed into place, so a reader (the
//! next message, a hook) never sees half of it. A file of a newer version is refused and left alone.
use std::path::{Path, PathBuf};

use super::{Workbench, VERSION};

const MAX_BYTES: u64 = 256 * 1024;

pub fn path_of(data: &Path, sid: &str) -> PathBuf {
    data.join("workbench").join("chat").join(format!("{sid}.json"))
}

fn valid_sid(sid: &str) -> bool {
    !sid.is_empty() && sid.len() <= 64 && sid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The saved workbench, or a new one when the chat has none. An Err is an i18n key.
pub fn load(data: &Path, sid: &str) -> Result<Workbench, &'static str> {
    if !valid_sid(sid) {
        return Err("wb_err_read");
    }
    let path = path_of(data, sid);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Workbench::default()),
        Err(_) => return Err("wb_err_read"),
        Ok(m) if !m.is_file() || m.len() > MAX_BYTES => return Err("wb_err_read"),
        Ok(_) => {}
    }
    let text = std::fs::read_to_string(&path).map_err(|_| "wb_err_read")?;
    let wb: Workbench = serde_json::from_str(&text).map_err(|_| "wb_err_read")?;
    if wb.version > VERSION {
        return Err("wb_err_version");
    }
    Ok(wb)
}

pub fn save(data: &Path, sid: &str, wb: &Workbench) -> Result<(), &'static str> {
    if !valid_sid(sid) {
        return Err("wb_err_write");
    }
    let text = serde_json::to_string_pretty(wb).map_err(|_| "wb_err_write")?;
    let path = path_of(data, sid);
    crate::private::create_dir(path.parent().ok_or("wb_err_write")?).map_err(|_| "wb_err_write")?;
    crate::private::write_atomic(&path, &text).map_err(|_| "wb_err_write")
}

/// The chat is deleted: so is its workbench.
pub fn remove(data: &Path, sid: &str) {
    if valid_sid(sid) {
        let _ = std::fs::remove_file(path_of(data, sid));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::Op;

    #[test]
    fn a_workbench_is_saved_loaded_removed_and_a_newer_one_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path(), "abc").unwrap(), Workbench::default());
        let mut w = Workbench::default();
        w.apply(Op::SetGoal { text: "goal".into() }).unwrap();
        save(dir.path(), "abc", &w).unwrap();
        assert_eq!(load(dir.path(), "abc").unwrap(), w);
        assert!(load(dir.path(), "../x").is_err() && save(dir.path(), "a/b", &w).is_err());
        let mut newer = w.clone();
        newer.version = VERSION + 1;
        save(dir.path(), "new", &newer).unwrap();
        assert_eq!(load(dir.path(), "new"), Err("wb_err_version"));
        std::fs::write(path_of(dir.path(), "bad"), "{ not json").unwrap();
        assert_eq!(load(dir.path(), "bad"), Err("wb_err_read"));
        remove(dir.path(), "abc");
        assert_eq!(load(dir.path(), "abc").unwrap(), Workbench::default());
    }
}
