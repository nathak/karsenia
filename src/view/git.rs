//! What the header and the worktree list show of a project's git state, read straight from the files in `.git` (no git process, so
//! it is cheap and safe to ask every second). Only plain reads with a size limit; what a file says is shown, never run.
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Opens a regular file for reading and nothing else. The folders these readers look in belong to whatever project is open, so a
/// file there may be a symbolic link, a named pipe or a device: none of those is opened (a pipe would block the reader for good).
/// The kind is checked on the open descriptor, not on the name, so it cannot be swapped between the check and the open.
pub fn open_regular(path: &Path) -> Option<fs::File> {
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW).open(path).ok()?
    };
    #[cfg(not(unix))]
    let file = {
        if !fs::symlink_metadata(path).ok()?.is_file() {
            return None;
        }
        fs::File::open(path).ok()?
    };
    file.metadata().ok()?.is_file().then_some(file)
}

/// At most `limit` bytes from the start of a regular file.
pub fn read_limited(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    open_regular(path)?.take(limit).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// A file's text, at most 4 KiB; None when it cannot be read.
fn read_small(path: &Path) -> Option<String> {
    Some(String::from_utf8_lossy(&read_limited(path, 4096)?).into_owned())
}

/// The entries of a folder, at most `cap` of them (a folder with millions of names is not listed whole).
pub fn list_dir(path: &Path, cap: usize) -> Vec<PathBuf> {
    fs::read_dir(path).map(|d| d.filter_map(Result::ok).take(cap).map(|e| e.path()).collect()).unwrap_or_default()
}

/// Where a project's git data is.
#[derive(Debug, Clone, PartialEq)]
pub struct GitDirs {
    /// The working folder (the one holding `.git`).
    pub top: PathBuf,
    /// This working folder's own git folder (`.git`, or `.git/worktrees/x` for a linked worktree).
    pub gitdir: PathBuf,
    /// The shared git folder (the same as `gitdir` for the main working folder).
    pub common: PathBuf,
}

fn resolve(base: &Path, value: &str) -> PathBuf {
    let p = Path::new(value.trim());
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// Looks for `.git` in `cwd` and the folders above it. A `.git` file (`gitdir: path`) is followed, and so is `commondir`.
pub fn find_git(cwd: &Path) -> Option<GitDirs> {
    for dir in cwd.ancestors().take(40) {
        let dot = dir.join(".git");
        // A `.git` folder that is a link to another is followed (some setups keep it elsewhere); a file is read bounded below.
        let Ok(meta) = fs::metadata(&dot) else { continue };
        if meta.is_dir() {
            return Some(GitDirs { top: dir.to_path_buf(), gitdir: dot.clone(), common: dot });
        }
        if meta.is_file() {
            let text = read_small(&dot)?;
            let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?;
            let gitdir = resolve(dir, target);
            let common = match read_small(&gitdir.join("commondir")) {
                // `../..` is the usual spelling; the folder is named as it really is so `.git` can be told from a bare folder.
                Some(c) if !c.trim().is_empty() => {
                    let path = resolve(&gitdir, &c);
                    fs::canonicalize(&path).unwrap_or(path)
                }
                _ => gitdir.clone(),
            };
            return Some(GitDirs { top: dir.to_path_buf(), gitdir, common });
        }
    }
    None
}

/// The branch a HEAD file names, or the first 7 characters of a detached commit; None for anything else.
pub fn parse_head(text: &str) -> Option<String> {
    let line = text.lines().next()?.trim();
    if let Some(reference) = line.strip_prefix("ref:") {
        let reference = reference.trim();
        let name = reference.strip_prefix("refs/heads/").unwrap_or(reference);
        let name: String = name.chars().filter(|c| !c.is_control()).take(80).collect();
        return (!name.is_empty()).then_some(name);
    }
    ((line.len() == 40 || line.len() == 64) && line.chars().all(|c| c.is_ascii_hexdigit())).then(|| line[..7].to_owned())
}

/// The current branch of the project that `cwd` is in.
pub fn head_branch(cwd: &Path) -> Option<String> {
    let git = find_git(cwd)?;
    parse_head(&read_small(&git.gitdir.join("HEAD"))?)
}

/// One row of the worktree list.
#[derive(Debug, Clone, PartialEq)]
pub struct WtEntry {
    pub path: PathBuf,
    pub branch: String,
    /// The folder the chat on screen works in.
    pub current: bool,
    /// The folder is gone (git still lists it until it is pruned).
    pub missing: bool,
    pub main: bool,
}

const MAX_ROWS: usize = 50;

/// The project's working folders: the main one, then the linked worktrees.
pub fn list_worktrees(cwd: &Path) -> Vec<WtEntry> {
    let Some(git) = find_git(cwd) else { return Vec::new() };
    let here = fs::canonicalize(&git.top).unwrap_or_else(|_| git.top.clone());
    let mut rows = Vec::new();
    // The main working folder is the parent of a `.git` folder; a bare repository has none.
    if git.common.file_name().is_some_and(|n| n == ".git") {
        if let Some(top) = git.common.parent() {
            let branch = read_small(&git.common.join("HEAD")).and_then(|t| parse_head(&t)).unwrap_or_default();
            let same = fs::canonicalize(top).unwrap_or_else(|_| top.to_path_buf()) == here;
            rows.push(WtEntry { path: top.to_path_buf(), branch, current: same, missing: !top.is_dir(), main: true });
        }
    }
    let mut entries = list_dir(&git.common.join("worktrees"), 200);
    entries.sort();
    for entry in entries.into_iter().take(MAX_ROWS) {
        // `gitdir` holds the path of the linked folder's `.git` file; its parent is the folder.
        let Some(text) = read_small(&entry.join("gitdir")) else { continue };
        let Some(path) = resolve(&entry, &text).parent().map(Path::to_path_buf) else { continue };
        let branch = read_small(&entry.join("HEAD")).and_then(|t| parse_head(&t)).unwrap_or_default();
        let missing = !path.is_dir();
        let current = !missing && fs::canonicalize(&path).map(|p| p == here).unwrap_or(false);
        rows.push(WtEntry { path, branch, current, missing, main: false });
    }
    rows.truncate(MAX_ROWS);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tenrec-git-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&dir).expect("scratch folder");
        dir
    }

    #[test]
    fn head_text_gives_a_branch_a_short_hash_or_nothing() {
        assert_eq!(parse_head("ref: refs/heads/feature/x\n").as_deref(), Some("feature/x"));
        assert_eq!(parse_head(&format!("{}\n", "ab12".repeat(10))).as_deref(), Some("ab12ab1"));
        assert_eq!(parse_head(&"f".repeat(64)).as_deref(), Some("fffffff"));
        assert_eq!(parse_head("garbage"), None);
        assert_eq!(parse_head(""), None);
        assert_eq!(parse_head("ref: refs/heads/\n"), None);
        assert_eq!(parse_head("ref: refs/heads/a\u{1b}b").as_deref(), Some("ab"));
    }

    #[test]
    fn a_plain_repository_names_its_branch_from_a_folder_below() {
        let root = scratch();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir_all(root.join("src/deep")).unwrap();
        assert_eq!(head_branch(&root.join("src/deep")).as_deref(), Some("main"));
        assert_eq!(head_branch(&root).as_deref(), Some("main"));
    }

    #[test]
    fn a_folder_outside_any_repository_or_with_a_broken_head_gives_nothing() {
        let root = scratch();
        assert_eq!(head_branch(&root), None);
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), "nonsense").unwrap();
        assert_eq!(head_branch(&root), None);
    }

    /// A repository with two linked worktrees (one gone) the way git lays them out.
    fn repo_with_worktrees(relative: bool) -> (PathBuf, PathBuf, PathBuf) {
        let root = scratch();
        let main = root.join("proj");
        let linked = root.join("wt-a");
        let gone = root.join("wt-gone");
        fs::create_dir_all(main.join(".git/worktrees/a")).unwrap();
        fs::create_dir_all(main.join(".git/worktrees/gone")).unwrap();
        fs::create_dir_all(&linked).unwrap();
        fs::write(main.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(main.join(".git/worktrees/a/HEAD"), "ref: refs/heads/tenrec/a\n").unwrap();
        fs::write(main.join(".git/worktrees/gone/HEAD"), format!("{}\n", "0".repeat(40))).unwrap();
        fs::write(main.join(".git/worktrees/a/gitdir"), format!("{}\n", linked.join(".git").display())).unwrap();
        fs::write(main.join(".git/worktrees/gone/gitdir"), format!("{}\n", gone.join(".git").display())).unwrap();
        let target = main.join(".git/worktrees/a");
        fs::write(main.join(".git/worktrees/a/commondir"), if relative { "../..\n".to_owned() } else { format!("{}\n", main.join(".git").display()) }).unwrap();
        fs::write(linked.join(".git"), format!("gitdir: {}\n", target.display())).unwrap();
        (main, linked, gone)
    }

    #[test]
    fn a_linked_worktree_reads_its_own_head_and_lists_the_shared_folders() {
        for relative in [true, false] {
            let (main, linked, gone) = repo_with_worktrees(relative);
            assert_eq!(head_branch(&linked).as_deref(), Some("tenrec/a"), "relative={relative}");
            assert_eq!(head_branch(&main).as_deref(), Some("main"));
            let rows = list_worktrees(&linked);
            assert_eq!(rows.len(), 3, "{rows:?}");
            assert!(rows[0].main && rows[0].branch == "main" && !rows[0].current && rows[0].path == main);
            let a = rows.iter().find(|r| r.path == linked).expect("linked row");
            assert!(a.current && !a.missing && a.branch == "tenrec/a");
            let g = rows.iter().find(|r| r.path == gone).expect("gone row");
            assert!(g.missing && !g.current && g.branch == "0000000");
            let from_main = list_worktrees(&main);
            assert!(from_main[0].current && from_main.iter().filter(|r| r.current).count() == 1);
        }
    }

    #[test]
    fn a_bare_repository_lists_only_its_linked_folders_and_a_plain_folder_nothing() {
        let root = scratch();
        assert!(list_worktrees(&root).is_empty());
        let bare = root.join("bare.git");
        fs::create_dir_all(bare.join("worktrees")).unwrap();
        let top = root.join("work");
        fs::create_dir_all(&top).unwrap();
        fs::write(top.join(".git"), format!("gitdir: {}\n", bare.display())).unwrap();
        assert!(list_worktrees(&top).is_empty(), "no main folder, no linked ones listed");
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_or_a_link_in_place_of_a_git_file_is_not_opened() {
        let root = scratch();
        fs::create_dir_all(root.join(".git")).unwrap();
        let head = root.join(".git/HEAD");
        let c = std::ffi::CString::new(head.to_str().unwrap()).unwrap();
        // SAFETY: mkfifo with a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        // Would block for good if it were opened for reading.
        assert_eq!(head_branch(&root), None);
        fs::remove_file(&head).unwrap();
        fs::write(root.join("real"), "ref: refs/heads/linked\n").unwrap();
        std::os::unix::fs::symlink(root.join("real"), &head).unwrap();
        assert_eq!(head_branch(&root), None, "a link is not followed");
        assert!(open_regular(&root.join("real")).is_some());
        assert!(open_regular(Path::new("/dev/null")).is_none(), "a device is not a regular file");
    }

    #[test]
    fn a_big_file_is_read_only_up_to_the_limit() {
        let root = scratch();
        let big = root.join("big");
        fs::write(&big, vec![b'x'; 100_000]).unwrap();
        assert_eq!(read_limited(&big, 4096).map(|b| b.len()), Some(4096));
        fs::create_dir_all(root.join("many")).unwrap();
        for i in 0..30 {
            fs::write(root.join(format!("many/f{i}")), "").unwrap();
        }
        assert_eq!(list_dir(&root.join("many"), 10).len(), 10);
        assert!(list_dir(&root.join("missing"), 10).is_empty());
    }
}
