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

pub fn write(path: &Path, text: &str) -> std::io::Result<()> {
    unlink_if_link(path);
    let mut file = options().write(true).truncate(true).open(path)?;
    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(text.as_bytes())
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
