//! Files this app writes hold prompts, images and tool results: folders are 0700 and files 0600 on Unix.
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub fn create_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
}

/// A link in the place of one of our files is removed, not followed: nothing else gets written through it.
fn unlink_if_link(path: &Path) {
    if std::fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
        let _ = std::fs::remove_file(path);
    }
}

/// Bytes written into a file that must not exist yet (0600): a name taken, or a link in its place, is an error and nothing is written
/// through it.
pub fn write_bytes_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)?.write_all(bytes)
}

pub fn write(path: &Path, text: &str) -> std::io::Result<()> {
    unlink_if_link(path);
    let mut file = options().write(true).truncate(true).open(path)?;
    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(text.as_bytes())
}

/// Written whole into a new file beside `path` and renamed over it: a reader sees the old text or the new, never half. The name of the
/// new file cannot be guessed and is made only if it does not exist (a link left there is not written through).
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{:x}-{:x}.tmp", std::process::id(), nonce));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&tmp)?;
    let done = file.write_all(text.as_bytes()).and_then(|()| file.sync_all());
    drop(file);
    match done.and_then(|()| std::fs::rename(&tmp, path)) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// How old a file `write_atomic` left behind must be before it is removed: another instance writing now is milliseconds old.
pub const STALE_TMP_AGE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Files `write_atomic` left beside `path` when the app stopped between writing and renaming (they hold the whole text, keys perhaps)
/// are removed: only regular files named exactly as it names them for this `path`, older than `min_age`. Links, folders and every
/// other name are left; nothing fails (on Windows a file another process holds open is skipped).
pub fn remove_stale_tmp(path: &Path, min_age: std::time::Duration) {
    if let Some(cutoff) = std::time::SystemTime::now().checked_sub(min_age) {
        remove_tmp_older_than(path, cutoff);
    }
}

fn remove_tmp_older_than(path: &Path, cutoff: std::time::SystemTime) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else { return };
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let prefix = format!(".{name}.");
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        if !file_name.to_str().is_some_and(|n| is_tmp_name(n, &prefix)) {
            continue;
        }
        let tmp = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&tmp) else { continue };
        if meta.file_type().is_file() && meta.modified().is_ok_and(|m| m <= cutoff) {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// `.{name}.{pid:x}-{nonce:x}.tmp` as `write_atomic` makes it: lowercase hex only, so no other store's name (it has a dot) matches.
fn is_tmp_name(file_name: &str, prefix: &str) -> bool {
    let Some((pid, nonce)) = file_name.strip_prefix(prefix).and_then(|r| r.strip_suffix(".tmp")).and_then(|r| r.split_once('-')) else { return false };
    let hex = |s: &str, max: usize| !s.is_empty() && s.len() <= max && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    hex(pid, 8) && hex(nonce, 32)
}

pub fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    unlink_if_link(path);
    let mut file = options().append(true).open(path)?;
    writeln!(file, "{line}")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn folders_and_files_are_private() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        create_dir(&data).unwrap();
        write(&data.join("a.meta.json"), "{}").unwrap();
        append_line(&data.join("a.jsonl"), "x").unwrap();
        append_line(&data.join("a.jsonl"), "y").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&data), 0o700);
        assert_eq!(mode(&data.join("a.meta.json")), 0o600);
        assert_eq!(mode(&data.join("a.jsonl")), 0o600);
        assert_eq!(std::fs::read_to_string(data.join("a.jsonl")).unwrap(), "x\ny\n");

        let target = dir.path().join("victim.txt");
        std::fs::write(&target, "keep").unwrap();
        let config = data.join("s.mcp.json");
        std::os::unix::fs::symlink(&target, &config).unwrap();
        write(&config, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep", "a link is replaced, not written through");
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "new");
    }
}

#[cfg(test)]
mod stale_tmp_tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn age(path: &Path, by: Duration) {
        std::fs::File::options().write(true).open(path).unwrap().set_modified(SystemTime::now() - by).unwrap();
    }

    #[test]
    fn only_an_old_tmp_of_this_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.json");
        let stale = dir.path().join(".providers.json.1a2b-17f0c3d2e4a5b6c7.tmp");
        let fresh = dir.path().join(".providers.json.1a2c-17f0c3d2e4a5b6c8.tmp");
        let others = [".providers.json.notes", "providers.json.tmp", ".other.json.1-2.tmp", ".providers.json.1-2.tmp.bak", ".providers.json.1A-2.tmp", ".providers.json.1-.tmp", ".providers.json.1-2-3.tmp", "providers.json"];
        for name in others {
            std::fs::write(dir.path().join(name), "keep").unwrap();
            age(&dir.path().join(name), Duration::from_secs(3600));
        }
        std::fs::write(&stale, "secret").unwrap();
        age(&stale, Duration::from_secs(3600));
        std::fs::write(&fresh, "being written").unwrap();
        remove_stale_tmp(&path, STALE_TMP_AGE);
        assert!(!stale.exists(), "an old tmp is removed");
        assert!(fresh.exists(), "another instance's tmp being written is kept");
        for name in others {
            assert_eq!(std::fs::read_to_string(dir.path().join(name)).unwrap(), "keep", "{name}");
        }
    }

    #[test]
    fn a_folder_or_link_named_like_a_tmp_is_left_however_old() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.json");
        let control = dir.path().join(".providers.json.1-1.tmp");
        std::fs::write(&control, "x").unwrap();
        let folder = dir.path().join(".providers.json.1-2.tmp");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("inside"), "keep").unwrap();
        #[cfg(unix)]
        let (link, victim) = (dir.path().join(".providers.json.1-3.tmp"), dir.path().join("victim.txt"));
        #[cfg(unix)]
        {
            std::fs::write(&victim, "keep").unwrap();
            std::os::unix::fs::symlink(&victim, &link).unwrap();
        }
        // Everything here counts as old: only its kind keeps it.
        remove_tmp_older_than(&path, SystemTime::now() + Duration::from_secs(3600));
        assert!(!control.exists(), "the cutoff did reach");
        assert_eq!(std::fs::read_to_string(folder.join("inside")).unwrap(), "keep");
        #[cfg(unix)]
        {
            assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
            assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        }
    }
}
