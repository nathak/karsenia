//! Continuing a chat under another login account. An account keeps its own settings folder, and `claude --resume` looks for a
//! session only in the folder it runs with, so the session file is copied to the other account's folder first. The line that names the
//! account the session belonged to (`session-owner`) is left out of the copy: the CLI writes its own for the account that continues it.
//! Nothing here reads or copies a login (credentials stay in their folder); only the one `<id>.jsonl` of the session is copied.
use std::path::{Path, PathBuf};

use crate::cli_sessions::encode_dir;

/// The largest session file that is copied.
pub const MAX_COPY_BYTES: u64 = 256 * 1024 * 1024;

fn is_session_id(id: &str) -> bool {
    id.len() == 36 && id.bytes().all(|b| b == b'-' || b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A real folder (not a link) that exists.
fn real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

/// The session file as `claude` keeps it: `<config>/projects/<folder of cwd>/<id>.jsonl`.
pub fn session_path(config: &Path, cwd: &str, id: &str) -> PathBuf {
    config.join("projects").join(encode_dir(cwd)).join(format!("{id}.jsonl"))
}

/// A fingerprint of a session file's conversation (its lines without the `session-owner` one): the same for the copy made for another
/// account as for the original, and different as soon as a line is added. A change detector, not a security check.
pub fn fingerprint(path: &Path) -> Option<u64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_COPY_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for line in text.lines() {
        if serde_json::from_str::<serde_json::Value>(line).map(|v| v["type"] == "session-owner").unwrap_or(false) {
            continue;
        }
        for byte in line.bytes().chain(std::iter::once(b'\n')) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    Some(hash)
}

/// Copies the session `id` of `cwd` from one config folder to another, without its `session-owner` line. Returns the new file. A session
/// the other account already has is never replaced.
pub fn copy_session(from: &Path, to: &Path, cwd: &str, id: &str) -> Result<PathBuf, String> {
    place(from, to, cwd, id, false)
}

/// As `copy_session`, but the file the other account has is replaced: only for the one case that was checked (that account's copy is the
/// ancestor of this one and has not changed since, see `fingerprint`).
pub fn replace_session(from: &Path, to: &Path, cwd: &str, id: &str) -> Result<PathBuf, String> {
    place(from, to, cwd, id, true)
}

fn place(from: &Path, to: &Path, cwd: &str, id: &str, overwrite: bool) -> Result<PathBuf, String> {
    if !is_session_id(id) {
        return Err("not a session id".into());
    }
    let src = session_path(from, cwd, id);
    let meta = std::fs::symlink_metadata(&src).map_err(|_| "the chat's session file was not found in its account".to_owned())?;
    if !meta.is_file() {
        return Err("the chat's session file is not a plain file".into());
    }
    if meta.len() > MAX_COPY_BYTES {
        return Err("the chat's session file is too big to copy".into());
    }
    let text = std::fs::read_to_string(&src).map_err(|e| format!("could not read the session file: {e}"))?;
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if serde_json::from_str::<serde_json::Value>(line).map(|v| v["type"] == "session-owner").unwrap_or(false) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    let dst = session_path(to, cwd, id);
    let folder = dst.parent().ok_or("no folder")?;
    let projects = to.join("projects");
    if std::fs::symlink_metadata(&projects).is_ok() && !real_dir(&projects) {
        return Err("the other account's projects folder is not a plain folder".into());
    }
    crate::private::create_dir(&projects).map_err(|e| e.to_string())?;
    if std::fs::symlink_metadata(folder).is_ok() && !real_dir(folder) {
        return Err("the other account's project folder is not a plain folder".into());
    }
    crate::private::create_dir(folder).map_err(|e| e.to_string())?;
    // Written whole beside the target, then linked into place: a reader never sees half a file, and a file that is already there (the other
    // account has this session, with turns of its own) is never replaced: the link is refused when the name exists, in one step.
    let tmp = folder.join(format!("{id}.jsonl.tenrec-tmp"));
    crate::private::write(&tmp, &out).map_err(|e| e.to_string())?;
    if overwrite {
        return std::fs::rename(&tmp, &dst).map(|()| dst).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        });
    }
    let linked = std::fs::hard_link(&tmp, &dst);
    let _ = std::fs::remove_file(&tmp);
    linked.map_err(|e| if e.kind() == std::io::ErrorKind::AlreadyExists { "the other account already has this session; open it there".to_owned() } else { e.to_string() })?;
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "748aa02c-6299-4224-8d60-1131a940bf14";

    #[test]
    fn the_session_is_copied_without_its_owner_line_and_never_replaced_once_the_other_account_has_it() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("a"), dir.path().join("b"));
        let cwd = "/work/my project";
        let src = session_path(&from, cwd, ID);
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, format!("{{\"type\":\"session-owner\",\"accountUuid\":\"x\",\"sessionId\":\"{ID}\"}}\n{{\"type\":\"user\",\"message\":{{\"content\":\"hi\"}}}}\n")).unwrap();
        let dst = copy_session(&from, &to, cwd, ID).unwrap();
        assert_eq!(dst, session_path(&to, cwd, ID));
        let text = std::fs::read_to_string(&dst).unwrap();
        assert!(!text.contains("session-owner") && text.contains("\"hi\""), "{text}");
        // A session the other account already has is never replaced, even by a newer copy.
        std::fs::write(&src, "{\"type\":\"user\",\"message\":{\"content\":\"later\"}}\n").unwrap();
        assert!(copy_session(&from, &to, cwd, ID).unwrap_err().contains("already has this session"));
        assert!(std::fs::read_to_string(&dst).unwrap().contains("kiwi") || std::fs::read_to_string(&dst).unwrap().contains("hi"));
        assert!(!dst.with_file_name(format!("{ID}.jsonl.tenrec-tmp")).exists());
    }

    #[test]
    fn a_bad_id_a_missing_file_and_a_link_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("a"), dir.path().join("b"));
        assert!(copy_session(&from, &to, "/w", "../x").is_err());
        assert!(copy_session(&from, &to, "/w", ID).is_err());
        #[cfg(unix)]
        {
            let src = session_path(&from, "/w", ID);
            std::fs::create_dir_all(src.parent().unwrap()).unwrap();
            let real = dir.path().join("real.jsonl");
            std::fs::write(&real, "{}\n").unwrap();
            std::os::unix::fs::symlink(&real, &src).unwrap();
            assert!(copy_session(&from, &to, "/w", ID).is_err(), "a link in place of the file is not followed");
        }
    }
}
