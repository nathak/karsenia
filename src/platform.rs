//! What differs between Linux/macOS and Windows, in one place.
use std::path::{Path, PathBuf};

/// The user's home folder: `HOME` where it exists, `USERPROFILE` on Windows.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// A canonical path in the form other programs take. On Windows `canonicalize` answers `\\?\C:\...`; git and the shell want
/// `C:\...`, so the prefix is left out for a plain drive path that fits the usual length (a UNC or longer path keeps it).
pub fn canon(path: &Path) -> std::io::Result<PathBuf> {
    Ok(plain(path.canonicalize()?))
}

/// An already canonical path in the form other programs take (see `canon`); unchanged except on Windows.
pub fn plain(real: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(rest) = real.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        let bytes = rest.as_bytes();
        if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' && rest.len() < 260 {
            return PathBuf::from(rest);
        }
    }
    real
}

/// Whether a candidate for the `claude` program is one this build will run. On Windows only a real executable is run:
/// a `.cmd` or `.bat` file goes through the command interpreter's quoting, and the settings and prompt text passed as
/// arguments must not.
pub fn runnable_claude(path: &Path) -> bool {
    if cfg!(windows) {
        path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    } else {
        true
    }
}

/// The first `claude` program on `path_var` (a PATH-style list) this build will run.
pub fn find_claude(path_var: &std::ffi::OsStr) -> Option<PathBuf> {
    find_program(path_var, "claude")
}

/// The first program called `stem` on `path_var` this build will run (on Windows only an `.exe`, for the reason given at `runnable_claude`).
pub fn find_program(path_var: &std::ffi::OsStr, stem: &str) -> Option<PathBuf> {
    let name = if cfg!(windows) { format!("{stem}.exe") } else { stem.to_owned() };
    std::env::split_paths(path_var).map(|dir| dir.join(&name)).find(|p| p.is_file() && runnable_claude(p))
}

/// Windows only: the data folder must sit inside the user's profile, whose default access list names the user, the
/// system and the administrators. No mode bits are set there, so a folder elsewhere would hold whatever its parent says.
pub fn data_dir_in_profile(data: &Path, profile: &Path) -> bool {
    let Ok(profile) = profile.canonicalize() else { return false };
    // The nearest folder that exists is resolved; what lies below it is still to be made and must be plain names (no `..`).
    let mut existing = data;
    loop {
        if let Ok(real) = existing.canonicalize() {
            return real.starts_with(&profile);
        }
        match (existing.file_name(), existing.parent()) {
            (Some(_), Some(parent)) => existing = parent,
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_folder_must_be_inside_the_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let profile = tmp.path().join("profile");
        std::fs::create_dir_all(profile.join("data")).unwrap();
        std::fs::create_dir_all(tmp.path().join("elsewhere")).unwrap();
        assert!(data_dir_in_profile(&profile.join("data"), &profile));
        assert!(data_dir_in_profile(&profile.join("not-yet-made"), &profile), "a folder about to be made counts by its parent");
        assert!(data_dir_in_profile(&profile.join(".local").join("share").join("tenrec"), &profile), "so does one several levels down");
        assert!(!data_dir_in_profile(&profile.join("a").join("..").join("..").join("elsewhere"), &profile), "a `..` below a missing folder is refused");
        assert!(!data_dir_in_profile(&tmp.path().join("elsewhere"), &profile));
        assert!(!data_dir_in_profile(&profile.join("data"), &tmp.path().join("missing")));
    }

    #[test]
    fn find_claude_takes_the_first_program_on_the_path() {
        let tmp = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) { "claude.exe" } else { "claude" };
        std::fs::create_dir_all(tmp.path().join("a")).unwrap();
        std::fs::create_dir_all(tmp.path().join("b")).unwrap();
        std::fs::write(tmp.path().join("b").join(name), b"").unwrap();
        let path = std::env::join_paths([tmp.path().join("a"), tmp.path().join("b")]).unwrap();
        assert_eq!(find_claude(&path), Some(tmp.path().join("b").join(name)));
        assert_eq!(find_claude(std::ffi::OsStr::new("")), None);
    }

    #[test]
    fn only_an_executable_runs_on_windows() {
        assert!(runnable_claude(Path::new("claude.exe")));
        assert_eq!(runnable_claude(Path::new("claude.cmd")), !cfg!(windows));
    }
}
