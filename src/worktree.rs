//! A chat in its own git worktree: made from the project's current commit on a new branch, checked, merged back and removed
//! here. Everything the host runs is `git`, with the environment cleaned (see `git`), and the folder a chat works in is only ever
//! removed by `git worktree remove`, never by deleting a path.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::session::Meta;

/// What a chat remembers of its worktree. The worktree's folder is the chat's own `cwd`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorktreeRef {
    /// The project's main folder (a canonical path) the worktree belongs to.
    pub repo: PathBuf,
    /// Always `tenrec/<sid>`.
    pub branch: String,
    /// The branch the main folder was on when the chat was made, and where the work is merged back to.
    pub base_branch: String,
    pub base_commit: String,
    /// The folder and, when it could, the branch were removed: the chat stays readable but cannot go on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
    /// The worktree root this chat's folder was made under (the setting may change later).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    /// What the project's git settings that can run programs looked like when the chat was made, or when the person last accepted them.
    #[serde(default)]
    pub config_hash: String,
}

pub fn branch_for(sid: &str) -> String {
    format!("tenrec/{sid}")
}

/// A name for the project's folder under the worktree root: its own name (plain characters only) and a number made from its path.
pub fn repo_key(repo: &Path) -> String {
    let name: String = repo.file_name().and_then(|n| n.to_str()).unwrap_or("repo").chars().filter(|c| c.is_ascii_alphanumeric() || "._-".contains(*c)).take(40).collect();
    let name = name.trim_matches('.');
    let hash = crate::claude::container::data_hash(repo);
    format!("{}-{}", if name.is_empty() { "repo" } else { name }, &hash[..8])
}

fn is_sid(sid: &str) -> bool {
    sid.len() == 16 && sid.bytes().all(|b| b.is_ascii_hexdigit())
}

// ---------------------------------------------------------------------------------------------------------------- running git

pub struct Out {
    pub ok: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Switched off for every call: hooks, the file-system monitor, signing, rerere (it would make a check and the merge differ), automatic
/// gc and maintenance (they would prune worktrees of other chats), and recursion into submodules.
const SAFE_FLAGS: [&str; 16] = [
    "-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false", "-c", "commit.gpgSign=false", "-c", "merge.verifySignatures=false", "-c", "rerere.enabled=false", "-c", "gc.auto=0", "-c", "maintenance.auto=false", "-c",
    "submodule.recurse=false",
];

/// Names of the environment variables that must not reach git: `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and
/// `GIT_CONFIG_PARAMETERS` and the rest would point it somewhere else or add settings.
pub fn git_vars_to_drop(names: impl IntoIterator<Item = String>) -> Vec<String> {
    names.into_iter().filter(|n| n.starts_with("GIT_")).collect()
}

fn short(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= 2000 {
        text.to_owned()
    } else {
        let cut: String = text.chars().take(2000).collect();
        format!("{cut}...")
    }
}

/// Runs `git` in `dir`: no terminal, no editor, no pager, no prompt, the English messages, the user's own git settings kept
/// (a merge commit needs the name and address) except the ones in `SAFE_FLAGS`.
pub async fn git(dir: &Path, args: &[&str], limit: Duration) -> Result<Out, String> {
    let mut flags: Vec<String> = SAFE_FLAGS.iter().map(|f| (*f).to_owned()).collect();
    if cfg!(windows) {
        // `/dev/null` is a path under C:\ for Git for Windows, where anyone may make `C:\dev\null\pre-commit`; `NUL` is a device.
        // An empty folder of the app's own, looked at on every call, holds no hooks.
        flags[1] = format!("core.hooksPath={}", no_hooks_dir()?.display());
    }
    let mut command = tokio::process::Command::new("git");
    crate::proc_group::no_window(&mut command);
    command.current_dir(dir).args(&flags).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    for name in git_vars_to_drop(std::env::vars_os().filter_map(|(k, _)| k.into_string().ok())) {
        command.env_remove(name);
    }
    command.env("GIT_TERMINAL_PROMPT", "0").env("GIT_EDITOR", "true").env("GIT_PAGER", "cat").env("GIT_OPTIONAL_LOCKS", "0").env("LC_ALL", "C");
    let output = match tokio::time::timeout(limit, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Err(format!("could not run git: {error}")),
        Err(_) => return Err("git took too long and was stopped".to_owned()),
    };
    Ok(Out { ok: output.status.success(), code: output.status.code(), stdout: String::from_utf8_lossy(&output.stdout).into_owned(), stderr: String::from_utf8_lossy(&output.stderr).into_owned() })
}

static NO_HOOKS: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Windows: the empty folder git is pointed at for hooks (`<data>/run/no-hooks`), made when the app opens its data folder.
pub fn set_no_hooks_dir(dir: PathBuf) -> std::io::Result<()> {
    std::fs::create_dir_all(&dir)?;
    let _ = NO_HOOKS.set(dir);
    Ok(())
}

fn no_hooks_dir() -> Result<PathBuf, String> {
    let dir = NO_HOOKS.get().ok_or("the folder that switches git hooks off is not set")?;
    let meta = std::fs::symlink_metadata(dir).map_err(|_| "the folder that switches git hooks off is missing".to_owned())?;
    let empty = std::fs::read_dir(dir).map(|mut d| d.next().is_none()).unwrap_or(false);
    if !meta.file_type().is_dir() || !empty {
        return Err("the folder that switches git hooks off is not an empty plain folder".to_owned());
    }
    Ok(dir.clone())
}

const QUICK: Duration = Duration::from_secs(30);
const SLOW: Duration = Duration::from_secs(120);
/// A merge or a removal is never cut off half way (a killed git leaves a lock and a half-done checkout): it is waited for.
const LONG: Duration = Duration::from_secs(30 * 60);

/// The trimmed output of a git command that must succeed; its own message otherwise.
async fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = git(dir, args, QUICK).await?;
    if out.ok {
        Ok(out.stdout.trim().to_owned())
    } else {
        Err(short(if out.stderr.trim().is_empty() { &out.stdout } else { &out.stderr }))
    }
}

async fn succeeds(dir: &Path, args: &[&str]) -> bool {
    git(dir, args, QUICK).await.map(|o| o.ok).unwrap_or(false)
}

type Locks = Mutex<Option<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>>;

/// One lock per project: making, merging and removing worktrees of one project never run at the same time.
pub fn repo_lock(repo: &Path) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: Locks = Mutex::new(None);
    let mut guard = LOCKS.lock().unwrap_or_else(|p| p.into_inner());
    guard.get_or_insert_with(HashMap::new).entry(repo.to_path_buf()).or_default().clone()
}

// ---------------------------------------------------------------------------------------------------------------- where and from what

/// The folder the worktrees go under: the person's setting, else `~/tenrec-worktrees`. It is made if needed and must be none of: the
/// app's data folder (or what holds it), the home folder (or what holds it), a place in the home folder that keeps credentials
/// (`~/.local` among them), the project itself or a folder holding it or held by it. Returns the canonical path.
pub fn resolve_root(configured: Option<&Path>, home: Option<&Path>, data: &Path, repo: &Path) -> Result<PathBuf, String> {
    let home = home.and_then(|h| crate::platform::canon(&h).ok()).ok_or("the home folder is not known, so a worktree cannot be placed safely")?;
    let root = match configured {
        Some(path) if !path.is_absolute() => return Err("the worktree folder in the settings must be an absolute path".to_owned()),
        Some(path) => path.to_path_buf(),
        None => home.join("tenrec-worktrees"),
    };
    std::fs::create_dir_all(&root).map_err(|e| format!("could not make the worktree folder {}: {e}", root.display()))?;
    check_root(&root, &home, data, repo)
}

/// A root a chat was made under, looked at again with the same rules and nothing made (the settings may have changed since).
pub fn trusted_root(stored: &Path, home: Option<&Path>, data: &Path, repo: &Path) -> Result<PathBuf, String> {
    let home = home.and_then(|h| crate::platform::canon(&h).ok()).ok_or("the home folder is not known, so a worktree cannot be placed safely")?;
    check_root(stored, &home, data, repo)
}

fn check_root(root: &Path, home: &Path, data: &Path, repo: &Path) -> Result<PathBuf, String> {
    let root = crate::platform::canon(&root).map_err(|e| e.to_string())?;
    let bad = "the worktree folder must not be the app's data folder, the home folder, a place that keeps credentials, or the project itself";
    crate::claude::container::holds_data(&root, data).map_err(|_| bad.to_owned())?;
    crate::claude::container::holds_home(&root, home).map_err(|_| bad.to_owned())?;
    if root.starts_with(repo) || repo.starts_with(&root) {
        return Err(bad.to_owned());
    }
    Ok(root)
}

/// What a folder given for a new chat is, as a project.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// The main folder (canonical).
    pub repo: PathBuf,
    /// The folder given was inside the project, not its top: the chat starts at the worktree's top.
    pub below_top: bool,
    pub branch: String,
    pub commit: String,
}

pub async fn inspect_source(cwd: &Path) -> Result<Source, String> {
    let layout = git(cwd, &["rev-parse", "--path-format=absolute", "--show-toplevel", "--absolute-git-dir", "--git-common-dir"], QUICK).await?;
    if !layout.ok {
        return Err("that folder is not a git project (or is a bare repository)".to_owned());
    }
    let lines: Vec<&str> = layout.stdout.lines().collect();
    let [top, git_dir, common] = lines[..] else { return Err("git gave an answer that could not be read".to_owned()) };
    let real = |p: &str| crate::platform::canon(Path::new(p)).map_err(|e| e.to_string());
    if real(git_dir)? != real(common)? {
        return Err("that folder is already a git worktree; start from the project's own folder".to_owned());
    }
    let repo = real(top)?;
    let branch = git(&repo, &["symbolic-ref", "-q", "--short", "HEAD"], QUICK).await?;
    if !branch.ok || branch.stdout.trim().is_empty() {
        return Err("the project is not on a branch (detached HEAD); switch to a branch first".to_owned());
    }
    let commit = git(&repo, &["rev-parse", "--verify", "-q", "HEAD^{commit}"], QUICK).await?;
    if !commit.ok {
        return Err("the project has no commit yet; make one first".to_owned());
    }
    Ok(Source { below_top: real(cwd.to_str().unwrap_or_default())? != repo, repo, branch: branch.stdout.trim().to_owned(), commit: commit.stdout.trim().to_owned() })
}

/// Makes the worktree `<root>/<repo key>/<sid>` on a new branch `tenrec/<sid>` at `base_commit` (a commit, so nothing is tracked
/// upstream and nothing is fetched). Returns its path. A failure leaves nothing of it behind.
pub async fn create(root: &Path, repo: &Path, sid: &str, base_commit: &str) -> Result<PathBuf, String> {
    if !is_sid(sid) {
        return Err("not a chat id".to_owned());
    }
    let parent = root.join(repo_key(repo));
    std::fs::create_dir_all(&parent).map_err(|e| format!("could not make {}: {e}", parent.display()))?;
    let path = parent.join(sid);
    if path.exists() {
        return Err("that worktree folder already exists".to_owned());
    }
    let text = path.to_str().ok_or("the worktree path is not text")?.to_owned();
    let branch = branch_for(sid);
    let made = git(repo, &["worktree", "add", "-b", &branch, &text, base_commit], SLOW).await;
    if let Ok(out) = &made {
        if out.ok {
            return Ok(path);
        }
    }
    // A failed attempt (a refused checkout, a timeout) gives back what it made, and only that: its own worktree and its own branch.
    forget_attempt(repo, &path, &branch, Some(base_commit)).await;
    let _ = std::fs::remove_dir(&path);
    let _ = std::fs::remove_dir(&parent);
    Err(match made {
        Ok(out) => short(&out.stderr),
        Err(message) => message,
    })
}

/// Removes the worktree at `path` and the branch `branch` that one attempt made. With `only_at` the branch goes only while it still
/// points at that commit (the attempt made it there), so a branch someone else made under the same name is never deleted. No
/// repository-wide `worktree prune`: it would also forget other worktrees whose folder is only temporarily missing.
async fn forget_attempt(repo: &Path, path: &Path, branch: &str, only_at: Option<&str>) {
    if let Some(text) = path.to_str() {
        // Twice: a worktree whose creation was cut short (a timeout) stays locked, and git removes a locked one only then. The
        // path is this attempt's own.
        let _ = git(repo, &["worktree", "remove", "--force", "--force", text], QUICK).await;
    }
    let reference = format!("refs/heads/{branch}");
    let Ok(now) = git(repo, &["rev-parse", "--verify", "--quiet", &reference], QUICK).await else { return };
    if now.ok && only_at.is_none_or(|commit| now.stdout.trim() == commit) {
        let _ = git(repo, &["branch", "-D", branch], QUICK).await;
    }
}

/// Undoes `create` after something later failed: the worktree and the branch (made a moment ago, equal to the base) are removed.
pub async fn rollback(repo: &Path, path: &Path, branch: &str) {
    forget_attempt(repo, path, branch, None).await;
    let _ = std::fs::remove_dir(path.parent().unwrap_or(path));
}

// ---------------------------------------------------------------------------------------------------------------- ownership

/// A worktree this host made for a chat, checked again against git just now.
#[derive(Debug, Clone)]
pub struct Owned {
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    pub base_branch: String,
}

/// The worktrees git lists for a project, as (folder, branch). Read with `-z`, so a folder name with a newline in it is one name.
pub fn parse_worktrees(text: &str) -> Vec<(PathBuf, Option<String>)> {
    let mut out = Vec::new();
    let mut current: Option<(PathBuf, Option<String>)> = None;
    for field in text.split('\0') {
        if field.is_empty() {
            out.extend(current.take());
        } else if let Some(path) = field.strip_prefix("worktree ") {
            out.extend(current.take());
            // Git for Windows prints `C:/x/y`; the paths it is compared with are `C:\x\y`.
            let path = if cfg!(windows) { path.replace('/', "\\") } else { path.to_owned() };
            current = Some((PathBuf::from(path), None));
        } else if let (Some(branch), Some(entry)) = (field.strip_prefix("branch refs/heads/"), current.as_mut()) {
            entry.1 = Some(branch.to_owned());
        }
    }
    out.extend(current);
    out
}

async fn listed(repo: &Path) -> Result<Vec<(PathBuf, Option<String>)>, String> {
    Ok(parse_worktrees(&git(repo, &["worktree", "list", "--porcelain", "-z"], QUICK).await.and_then(|o| if o.ok { Ok(o.stdout) } else { Err(short(&o.stderr)) })?))
}

/// The settings of a project's git that can make git run a program of someone's choosing when it only looks (a content filter, a
/// merge or diff driver, an included file, the attributes files, `branch.<name>.mergeOptions`, …), as they are now. The worktree
/// shares `.git` with the project, so a command a chat ran can have changed them.
#[derive(Debug, Clone, PartialEq)]
pub struct Fingerprint {
    pub hash: String,
    /// The settings that count, `key=value`, sorted, cut short (to show the person what changed).
    pub entries: Vec<String>,
}

fn risky_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    let runs = [".clean", ".smudge", ".process", ".driver", ".textconv", ".command", ".required"];
    ["filter.", "merge.", "diff."].iter().any(|p| k.starts_with(p) && runs.iter().any(|r| k.ends_with(r)))
        || k == "diff.external"
        || k.starts_with("include.")
        || k.starts_with("includeif.")
        || k.starts_with("pager.")
        || k.starts_with("credential.")
        || k.starts_with("gpg.")
        || (k.starts_with("branch.") && k.ends_with(".mergeoptions"))
        || ["core.attributesfile", "core.fsmonitor", "core.hookspath", "core.sshcommand", "core.pager", "core.editor", "core.askpass", "core.worktree", "core.gitproxy"].contains(&k.as_str())
}

fn fnv(bytes: &[u8]) -> String {
    format!("{:016x}", bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)))
}

pub async fn fingerprint(repo: &Path, worktree: &Path) -> Result<Fingerprint, String> {
    let mut entries: Vec<String> = Vec::new();
    let mut add = |text: &str| {
        for item in text.split('\0').filter(|i| !i.is_empty()) {
            let (key, value) = item.split_once('\n').unwrap_or((item, ""));
            if risky_key(key) {
                entries.push(format!("{key}={value}"));
            }
        }
    };
    add(&run_raw(repo, &["config", "--local", "--list", "--includes", "-z"]).await?);
    if worktree.exists() {
        // Settings of this worktree alone (when the project keeps such a file): empty, or an error, when it does not.
        add(&run_raw(worktree, &["config", "--worktree", "--list", "--includes", "-z"]).await.unwrap_or_default());
    }
    entries.sort();
    entries.dedup();
    let mut material = entries.join("\n").into_bytes();
    // Files the settings point at, or that git reads by itself, which can say which program a path is run through.
    let info = run(repo, &["rev-parse", "--path-format=absolute", "--git-path", "info/attributes"]).await?;
    material.extend(std::fs::read(&info).unwrap_or_default());
    if let Ok(path) = run(repo, &["config", "--type=path", "--get", "core.attributesfile"]).await {
        material.extend(std::fs::read(path).unwrap_or_default());
    }
    let shown = entries.iter().take(20).map(|e| e.chars().take(200).collect()).collect();
    Ok(Fingerprint { hash: fnv(&material), entries: shown })
}

async fn run_raw(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = git(dir, args, QUICK).await?;
    if out.ok {
        Ok(out.stdout)
    } else {
        Err(short(&out.stderr))
    }
}

/// Every operation on a chat's worktree begins here. What the meta file says is not believed: the branch, the folder and the
/// project are worked out again from the chat's id and compared, and git must list that folder on that branch. A meta file
/// changed to point somewhere else is refused, so nothing outside the worktree root is merged into or removed. With `check_config`,
/// the project's git settings that can run programs must also be as they were (`config_changed` otherwise: a command run in the chat
/// can write them, and merging or even looking would then run what it wrote).
pub async fn verify(meta: &Meta, root: &Path, check_config: bool) -> Result<Owned, String> {
    let wt = meta.worktree.as_ref().ok_or("this chat has no worktree")?;
    if !is_sid(&meta.sid) || wt.branch != branch_for(&meta.sid) {
        return Err("not_owned".to_owned());
    }
    let repo = crate::platform::canon(&wt.repo).map_err(|_| "not_owned".to_owned())?;
    if repo != wt.repo || root.starts_with(&repo) || repo.starts_with(root) {
        return Err("not_owned".to_owned());
    }
    let layout = git(&repo, &["rev-parse", "--path-format=absolute", "--absolute-git-dir", "--git-common-dir"], QUICK).await?;
    let lines: Vec<&str> = layout.stdout.lines().collect();
    let [git_dir, common] = lines[..] else { return Err("not_owned".to_owned()) };
    if !layout.ok || crate::platform::canon(Path::new(git_dir)).ok() != crate::platform::canon(Path::new(common)).ok() {
        return Err("not_owned".to_owned());
    }
    let expected = root.join(repo_key(&repo)).join(&meta.sid);
    if meta.cwd != expected || (expected.exists() && crate::platform::canon(&expected).ok().as_deref() != Some(expected.as_path())) {
        return Err("not_owned".to_owned());
    }
    if !listed(&repo).await?.iter().any(|(path, branch)| *path == expected && branch.as_deref() == Some(wt.branch.as_str())) {
        return Err("not_owned".to_owned());
    }
    if check_config && fingerprint(&repo, &expected).await?.hash != wt.config_hash {
        return Err("config_changed".to_owned());
    }
    Ok(Owned { repo, path: expected, branch: wt.branch.clone(), base_branch: wt.base_branch.clone() })
}

/// A chat whose worktree is gone from disk and from git's list (a folder removed by hand and pruned): nothing is left to remove, so the
/// chat may be marked as cleaned up without running anything.
pub async fn can_forget(meta: &Meta, root: &Path) -> bool {
    let Some(wt) = meta.worktree.as_ref() else { return false };
    if !is_sid(&meta.sid) || wt.branch != branch_for(&meta.sid) || wt.removed {
        return false;
    }
    let Ok(repo) = crate::platform::canon(&wt.repo) else { return false };
    let expected = root.join(repo_key(&repo)).join(&meta.sid);
    if repo != wt.repo || meta.cwd != expected || expected.symlink_metadata().is_ok() {
        return false;
    }
    matches!(listed(&repo).await, Ok(list) if !list.iter().any(|(path, _)| *path == expected))
}

// ---------------------------------------------------------------------------------------------------------------- the check

/// Everything a merge or a removal is decided by, read without changing anything.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Facts {
    /// The worktree's folder is gone (deleted by hand).
    pub missing: bool,
    pub main_branch: Option<String>,
    pub base_branch: String,
    pub main_head: String,
    pub tip: String,
    /// Commits of the chat's branch the main folder's branch does not have, and the reverse.
    pub ahead: usize,
    pub behind: usize,
    pub files: usize,
    pub insertions: usize,
    pub deletions: usize,
    /// Files with changes (or new) in the worktree, not committed: they would not come along.
    pub uncommitted: usize,
    pub main_dirty: bool,
    pub mid_merge: bool,
    pub conflicts: Vec<String>,
    pub conflicts_more: usize,
    /// Every commit of the branch is in the base branch's history: the branch can go without losing anything.
    pub in_base: bool,
    /// Not in its history, but merging it would change nothing (squashed, picked, or a merge driver that keeps "ours"): looks done,
    /// and is never enough to delete the branch without being asked.
    pub same_content: bool,
    pub blockers: Vec<&'static str>,
}

const MAX_CONFLICTS: usize = 50;

/// What stands in the way of a merge, most basic first.
pub fn blockers(f: &Facts) -> Vec<&'static str> {
    let mut out = Vec::new();
    if f.missing {
        out.push("missing");
        return out;
    }
    if f.uncommitted > 0 {
        out.push("uncommitted");
    }
    if f.ahead == 0 {
        out.push("nothing");
    }
    if f.main_branch.as_deref() != Some(f.base_branch.as_str()) {
        out.push("wrong_branch");
    }
    if f.main_dirty {
        out.push("main_dirty");
    }
    if f.mid_merge {
        out.push("mid_merge");
    }
    if !f.conflicts.is_empty() {
        out.push("conflict");
    }
    out
}

async fn count(dir: &Path, range: &str) -> usize {
    run(dir, &["rev-list", "--count", range]).await.ok().and_then(|t| t.parse().ok()).unwrap_or(0)
}

/// `merge-tree` writes the merged tree and, when it conflicts (exit 1), the files; nothing in the folders changes.
/// With `-z` the output is the tree id, then each conflicted file, each ended by a NUL.
async fn merged_tree(dir: &Path, into: &str, from: &str) -> Result<(String, Vec<String>), String> {
    let out = git(dir, &["merge-tree", "--write-tree", "--name-only", "--no-messages", "-z", into, from], QUICK).await?;
    if !out.ok && out.code != Some(1) {
        return Err(short(&out.stderr));
    }
    let mut parts = out.stdout.split('\0').filter(|p| !p.is_empty()).map(str::to_owned);
    let tree = parts.next().unwrap_or_default();
    let mut files: Vec<String> = if out.ok { Vec::new() } else { parts.collect() };
    files.sort();
    files.dedup();
    Ok((tree, files))
}

pub async fn facts(o: &Owned) -> Result<Facts, String> {
    let repo = &o.repo;
    let main_branch = git(repo, &["symbolic-ref", "-q", "--short", "HEAD"], QUICK).await?;
    let main_branch = main_branch.ok.then(|| main_branch.stdout.trim().to_owned()).filter(|b| !b.is_empty());
    let main_head = run(repo, &["rev-parse", "--verify", "-q", "HEAD^{commit}"]).await?;
    let tip_ref = format!("refs/heads/{}^{{commit}}", o.branch);
    let tip = run(repo, &["rev-parse", "--verify", "-q", &tip_ref]).await.map_err(|_| "the branch of this chat is gone".to_owned())?;
    let missing = !o.path.exists();
    let uncommitted = if missing { 0 } else { run(&o.path, &["status", "--porcelain"]).await.map(|t| t.lines().count()).unwrap_or(0) };
    let main_dirty = !run(repo, &["status", "--porcelain", "--untracked-files=no"]).await?.is_empty();
    let mid_merge = succeeds(repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await;
    let ahead = count(repo, &format!("{main_head}..{tip}")).await;
    let behind = count(repo, &format!("{tip}..{main_head}")).await;
    let (mut files, mut insertions, mut deletions) = (0, 0, 0);
    if let Ok(numstat) = run(repo, &["diff", "--no-ext-diff", "--no-textconv", "--numstat", &format!("{main_head}...{tip}")]).await {
        for line in numstat.lines() {
            let mut cols = line.split('\t');
            files += 1;
            insertions += cols.next().and_then(|n| n.parse::<usize>().ok()).unwrap_or(0);
            deletions += cols.next().and_then(|n| n.parse::<usize>().ok()).unwrap_or(0);
        }
    }
    let mut conflicts = if ahead > 0 { merged_tree(repo, &main_head, &tip).await.map(|(_, files)| files).unwrap_or_default() } else { Vec::new() };
    let conflicts_more = conflicts.len().saturating_sub(MAX_CONFLICTS);
    conflicts.truncate(MAX_CONFLICTS);
    let base_ref = format!("refs/heads/{}", o.base_branch);
    let target = if succeeds(repo, &["rev-parse", "--verify", "-q", &format!("{base_ref}^{{commit}}")]).await { base_ref } else { main_head.clone() };
    let in_base = succeeds(repo, &["merge-base", "--is-ancestor", &tip, &target]).await;
    let mut same_content = false;
    if !in_base {
        if let (Ok((tree, files)), Ok(want)) = (merged_tree(repo, &target, &tip).await, run(repo, &["rev-parse", &format!("{target}^{{tree}}")]).await) {
            same_content = files.is_empty() && tree == want;
        }
    }
    let mut facts = Facts { missing, main_branch, base_branch: o.base_branch.clone(), main_head, tip, ahead, behind, files, insertions, deletions, uncommitted, main_dirty, mid_merge, conflicts, conflicts_more, in_base, same_content, blockers: Vec::new() };
    facts.blockers = blockers(&facts);
    Ok(facts)
}

// ---------------------------------------------------------------------------------------------------------------- merge and remove

#[derive(Debug)]
pub struct Refused {
    /// `stale`, `blocked`, `conflict`, `abort_failed`, `unexpected_result`, or `git` (git's own message).
    pub code: &'static str,
    pub message: String,
    pub conflicts: Vec<String>,
    pub blockers: Vec<&'static str>,
}

fn refused(code: &'static str, message: impl Into<String>) -> Refused {
    Refused { code, message: message.into(), conflicts: Vec::new(), blockers: Vec::new() }
}

/// Undoes a merge this host started and stopped before committing, but only when it is ours (its `MERGE_HEAD` is the commit we merge):
/// a merge a person started in the project is theirs and is never touched.
async fn abort_ours(repo: &Path, tip: &str) -> Result<(), Refused> {
    let current = run(repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await.ok();
    if current.as_deref() == Some(tip) && !succeeds(repo, &["merge", "--abort"]).await {
        return Err(refused("abort_failed", "the merge could not be undone; run `git merge --abort` in the project folder"));
    }
    Ok(())
}

/// Merges exactly the commit the person was shown (`tip`) into the main folder's branch, which must still be at `main_head`. The
/// caller holds the project's lock. The merge is made without committing and looked at first: its result must be the tree `merge-tree`
/// predicted (a setting such as `branch.<name>.mergeOptions = -s ours` would leave the work out), and only then is it committed. A
/// conflict, or anything unexpected, aborts it and leaves the project as it was.
pub async fn merge(o: &Owned, tip: &str, main_head: &str) -> Result<String, Refused> {
    let f = facts(o).await.map_err(|m| refused("git", m))?;
    if f.tip != tip || f.main_head != main_head {
        return Err(refused("stale", "the branches changed since they were checked; check again"));
    }
    if !f.blockers.is_empty() {
        return Err(Refused { code: "blocked", message: "the merge is blocked".to_owned(), conflicts: f.conflicts, blockers: f.blockers });
    }
    let (expected_tree, _) = merged_tree(&o.repo, main_head, tip).await.map_err(|m| refused("git", m))?;
    let no_options = format!("branch.{}.mergeoptions=", o.base_branch);
    let out = git(&o.repo, &["-c", &no_options, "merge", "--no-ff", "--no-commit", "--no-edit", "--no-autostash", tip], LONG).await.map_err(|m| refused("git", m))?;
    if !out.ok {
        let conflicts: Vec<String> = run(&o.repo, &["diff", "--name-only", "--diff-filter=U"]).await.map(|t| t.lines().take(MAX_CONFLICTS).map(str::to_owned).collect()).unwrap_or_default();
        abort_ours(&o.repo, tip).await?;
        if conflicts.is_empty() {
            return Err(refused("git", short(&out.stderr)));
        }
        return Err(Refused { code: "conflict", message: "the merge conflicts".to_owned(), conflicts, blockers: Vec::new() });
    }
    let merging = run(&o.repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await.ok();
    let result = run(&o.repo, &["write-tree"]).await.ok();
    if merging.as_deref() != Some(tip) || result.as_deref() != Some(expected_tree.as_str()) {
        abort_ours(&o.repo, tip).await?;
        return Err(refused("unexpected_result", "the merge did not give the result that was checked, so it was undone"));
    }
    let message = format!("Merge {} into {}", o.branch, o.base_branch);
    let commit = git(&o.repo, &["commit", "--no-verify", "--no-edit", "-m", &message], LONG).await.map_err(|m| refused("git", m))?;
    if !commit.ok {
        abort_ours(&o.repo, tip).await?;
        return Err(refused("git", short(&commit.stderr)));
    }
    let head = run(&o.repo, &["rev-parse", "HEAD"]).await.map_err(|m| refused("git", m))?;
    let parents = run(&o.repo, &["rev-list", "--parents", "-n", "1", "HEAD"]).await.unwrap_or_default();
    if parents.split(' ').collect::<Vec<_>>() != [head.as_str(), main_head, tip] {
        return Err(refused("unexpected_result", format!("the merge commit {head} does not have the parents that were checked; look at it before going on")));
    }
    Ok(head)
}

/// What a removal did besides what was asked.
#[derive(Debug, PartialEq)]
pub struct Removed {
    pub branch_kept: bool,
    /// `folder_left` (git could not delete all of the folder) or `branch_not_deleted` (git refused to delete the branch).
    pub warning: Option<&'static str>,
}

/// Removes the worktree and, when every commit of its branch is in the base branch's history or `discard` says so, the branch. Without
/// `force` a worktree with uncommitted changes is refused; with `expect_tip` a branch that moved since the person looked is refused.
/// A removal git began is judged by git's list afterwards: gone from it means removed, whatever else went wrong (that is `warning`).
/// The caller holds the project's lock.
pub async fn remove(o: &Owned, force: bool, discard: bool, expect_tip: Option<&str>) -> Result<Removed, String> {
    let known = facts(o).await.ok();
    if let Some(f) = &known {
        if expect_tip.is_some_and(|t| t != f.tip) {
            return Err("stale".to_owned());
        }
        if f.uncommitted > 0 && !force {
            return Err("uncommitted".to_owned());
        }
    }
    let text = o.path.to_str().ok_or("the worktree path is not text")?;
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(text);
    let out = git(&o.repo, &args, LONG).await?;
    if listed(&o.repo).await?.iter().any(|(path, _)| *path == o.path) {
        return Err(short(&out.stderr));
    }
    let mut warning = o.path.exists().then_some("folder_left");
    if known.as_ref().is_some_and(|f| f.in_base || discard) && !succeeds(&o.repo, &["branch", "-D", &o.branch]).await {
        warning = warning.or(Some("branch_not_deleted"));
    }
    let branch_kept = succeeds(&o.repo, &["rev-parse", "--verify", "-q", &format!("refs/heads/{}", o.branch)]).await;
    Ok(Removed { branch_kept, warning })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git").current_dir(dir).args(args).env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// A project with one commit on `main`, and a home folder, data folder and worktree root beside it.
    struct World {
        _tmp: tempfile::TempDir,
        repo: PathBuf,
        home: PathBuf,
        data: PathBuf,
        root: PathBuf,
    }

    fn world() -> World {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (repo, home, data) = (base.join("proj"), base.join("home"), base.join("home").join(".local").join("share").join("tenrec"));
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        sh(&repo, &["-c", "init.defaultBranch=main", "init"]);
        sh(&repo, &["config", "user.name", "t"]);
        sh(&repo, &["config", "user.email", "t@t"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        sh(&repo, &["add", "."]);
        sh(&repo, &["commit", "-m", "first"]);
        let root = resolve_root(Some(&base.join("wts")), Some(&home), &data, &repo).unwrap();
        World { _tmp: tmp, repo, home, data, root }
    }

    async fn owned_in(w: &World, sid: &str) -> (Owned, Meta) {
        let source = inspect_source(&w.repo).await.unwrap();
        let path = create(&w.root, &source.repo, sid, &source.commit).await.unwrap();
        let config_hash = fingerprint(&source.repo, &path).await.unwrap().hash;
        let wt = WorktreeRef { repo: source.repo.clone(), branch: branch_for(sid), base_branch: source.branch.clone(), base_commit: source.commit.clone(), removed: false, root: Some(w.root.clone()), config_hash };
        let mut meta = crate::session::new_meta(path, None, None, false, None);
        meta.sid = sid.to_owned();
        meta.worktree = Some(wt);
        (verify(&meta, &w.root, true).await.unwrap(), meta)
    }

    const SID: &str = "00112233aabbccdd";

    #[test]
    fn git_gets_none_of_the_variables_that_would_aim_or_configure_it() {
        let names = ["PATH", "GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_CONFIG_PARAMETERS", "HOME", "GIT_SSH_COMMAND"].map(str::to_owned);
        assert_eq!(git_vars_to_drop(names), ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_CONFIG_PARAMETERS", "GIT_SSH_COMMAND"]);
    }

    #[test]
    fn a_key_is_made_of_plain_characters_and_the_path() {
        let a = repo_key(Path::new("/x/my proj;rm"));
        assert!(a.starts_with("myprojrm-") && a.len() == "myprojrm-".len() + 8, "{a}");
        assert_ne!(repo_key(Path::new("/x/p")), repo_key(Path::new("/y/p")), "the same name in two places is two keys");
        assert!(repo_key(Path::new("/x/..")).starts_with("repo-"));
    }

    #[test]
    fn what_blocks_a_merge_is_listed_in_order() {
        let ok = Facts { main_branch: Some("main".into()), base_branch: "main".into(), ahead: 2, ..Default::default() };
        assert!(blockers(&ok).is_empty());
        let bad = Facts { uncommitted: 1, ahead: 0, main_branch: Some("dev".into()), base_branch: "main".into(), main_dirty: true, mid_merge: true, conflicts: vec!["a".into()], ..Default::default() };
        assert_eq!(blockers(&bad), ["uncommitted", "nothing", "wrong_branch", "main_dirty", "mid_merge", "conflict"]);
        assert_eq!(blockers(&Facts { missing: true, ..bad }), ["missing"]);
    }

    #[test]
    fn the_worktree_root_keeps_away_from_the_data_folder_the_credentials_and_the_project() {
        let w = world();
        let ok = |p: &Path| resolve_root(Some(p), Some(&w.home), &w.data, &w.repo);
        assert!(ok(&w._tmp.path().join("fine")).is_ok());
        assert!(ok(&w.data.join("wts")).is_err(), "inside the data folder");
        assert!(ok(&w.home.join(".local").join("x")).is_err(), "inside ~/.local");
        assert!(ok(&w.home.join(".ssh").join("x")).is_err(), "inside ~/.ssh");
        assert!(ok(&w.repo.join("wts")).is_err(), "inside the project");
        assert!(ok(w.repo.parent().unwrap()).is_err(), "holding the project");
        assert!(ok(Path::new("relative")).is_err());
        assert!(resolve_root(Some(&w._tmp.path().join("fine")), None, &w.data, &w.repo).is_err(), "no home folder known");
        assert_eq!(resolve_root(None, Some(&w.home), &w.data, &w.repo).unwrap(), w.home.canonicalize().unwrap().join("tenrec-worktrees"), "the default");
    }

    #[tokio::test]
    async fn a_failed_attempt_gives_back_only_what_it_made() {
        let w = world();
        let base = sh(&w.repo, &["rev-parse", "HEAD"]);
        // A branch of the same name that someone else made elsewhere is not the attempt's: it stays.
        sh(&w.repo, &["branch", "tenrec/other"]);
        std::fs::write(w.repo.join("b.txt"), "two\n").unwrap();
        sh(&w.repo, &["add", "."]);
        sh(&w.repo, &["commit", "-m", "second"]);
        sh(&w.repo, &["branch", "-f", "tenrec/other", "HEAD"]);
        forget_attempt(&w.repo, &w.root.join("none"), "tenrec/other", Some(&base)).await;
        assert!(sh(&w.repo, &["branch", "--list", "tenrec/other"]).contains("tenrec/other"), "a branch that is not at the attempt's commit is kept");
        // Another worktree whose folder is missing is not forgotten by an unrelated failure.
        let (owned, _) = owned_in(&w, SID).await;
        std::fs::remove_dir_all(&owned.path).unwrap();
        forget_attempt(&w.repo, &w.root.join("none"), "tenrec/nothing", Some(&base)).await;
        assert!(sh(&w.repo, &["worktree", "list", "--porcelain"]).contains(&owned.path.display().to_string()), "no repository-wide prune");
        // Its own branch, still at the attempt's commit, goes.
        sh(&w.repo, &["branch", "tenrec/mine", &base]);
        forget_attempt(&w.repo, &w.root.join("none"), "tenrec/mine", Some(&base)).await;
        assert!(sh(&w.repo, &["branch", "--list", "tenrec/mine"]).is_empty());
    }

    #[tokio::test]
    async fn a_folder_that_is_not_a_plain_project_is_refused() {
        let w = world();
        let plain = w._tmp.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(inspect_source(&plain).await.is_err(), "not a repository");
        let sub = w.repo.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let from_sub = inspect_source(&sub).await.unwrap();
        assert!(from_sub.below_top && from_sub.repo == w.repo && from_sub.branch == "main");
        let fresh = w._tmp.path().join("fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        sh(&fresh, &["-c", "init.defaultBranch=main", "init"]);
        assert!(inspect_source(&fresh).await.unwrap_err().contains("no commit"));
        let (owned, _) = owned_in(&w, SID).await;
        assert!(inspect_source(&owned.path).await.unwrap_err().contains("already a git worktree"));
        sh(&w.repo, &["checkout", "--detach"]);
        assert!(inspect_source(&w.repo).await.unwrap_err().contains("detached"));
    }

    #[tokio::test]
    async fn a_worktree_is_made_checked_merged_and_removed() {
        let w = world();
        let (owned, _) = owned_in(&w, SID).await;
        assert_eq!(owned.path, w.root.join(repo_key(&w.repo)).join(SID));
        assert_eq!(sh(&owned.path, &["rev-parse", "--abbrev-ref", "HEAD"]), format!("tenrec/{SID}"));
        let f = facts(&owned).await.unwrap();
        assert_eq!(f.blockers, ["nothing"], "nothing done yet");
        std::fs::write(owned.path.join("b.txt"), "two\n").unwrap();
        assert_eq!(facts(&owned).await.unwrap().blockers, ["uncommitted", "nothing"]);
        sh(&owned.path, &["add", "."]);
        sh(&owned.path, &["commit", "-m", "second"]);
        let f = facts(&owned).await.unwrap();
        assert!(f.blockers.is_empty(), "{:?}", f.blockers);
        assert_eq!((f.ahead, f.behind, f.files, f.insertions), (1, 0, 1, 1));
        assert!(!f.in_base);
        // A merge of something other than what was shown is refused.
        let stale = merge(&owned, &f.tip, "0000000000000000000000000000000000000000").await.unwrap_err();
        assert_eq!(stale.code, "stale");
        let merged = merge(&owned, &f.tip, &f.main_head).await.unwrap();
        assert_eq!(sh(&w.repo, &["rev-parse", "HEAD"]), merged);
        assert_eq!(sh(&w.repo, &["rev-parse", "HEAD^2"]), f.tip, "a merge commit whose second parent is what was shown");
        assert!(w.repo.join("b.txt").exists());
        let after = facts(&owned).await.unwrap();
        assert!(after.in_base && after.blockers == ["nothing"]);
        assert_eq!(remove(&owned, false, false, None).await, Ok(Removed { branch_kept: false, warning: None }), "merged: the branch goes too");
        assert!(!owned.path.exists());
        assert!(!succeeds(&w.repo, &["rev-parse", "--verify", "-q", &format!("refs/heads/tenrec/{SID}")]).await);
    }

    #[tokio::test]
    async fn a_conflict_is_reported_and_leaves_the_project_as_it_was() {
        let w = world();
        let (owned, _) = owned_in(&w, SID).await;
        std::fs::write(owned.path.join("a.txt"), "mine\n").unwrap();
        sh(&owned.path, &["commit", "-am", "mine"]);
        std::fs::write(w.repo.join("a.txt"), "theirs\n").unwrap();
        sh(&w.repo, &["commit", "-am", "theirs"]);
        let f = facts(&owned).await.unwrap();
        assert_eq!(f.blockers, ["conflict"]);
        assert_eq!(f.conflicts, ["a.txt"]);
        let head = sh(&w.repo, &["rev-parse", "HEAD"]);
        let refused = merge(&owned, &f.tip, &f.main_head).await.unwrap_err();
        assert_eq!(refused.code, "blocked");
        assert_eq!(sh(&w.repo, &["rev-parse", "HEAD"]), head);
        assert!(!succeeds(&w.repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await);
    }

    #[tokio::test]
    async fn the_main_folder_being_elsewhere_or_dirty_blocks() {
        let w = world();
        let (owned, _) = owned_in(&w, SID).await;
        std::fs::write(owned.path.join("b.txt"), "x\n").unwrap();
        sh(&owned.path, &["add", "."]);
        sh(&owned.path, &["commit", "-m", "b"]);
        std::fs::write(w.repo.join("a.txt"), "changed\n").unwrap();
        assert_eq!(facts(&owned).await.unwrap().blockers, ["main_dirty"]);
        sh(&w.repo, &["checkout", "--", "a.txt"]);
        sh(&w.repo, &["checkout", "-b", "other"]);
        assert_eq!(facts(&owned).await.unwrap().blockers, ["wrong_branch"]);
    }

    #[tokio::test]
    async fn work_squashed_into_the_base_looks_done_by_content_and_unmerged_work_keeps_its_branch() {
        let w = world();
        let (owned, _) = owned_in(&w, SID).await;
        std::fs::write(owned.path.join("b.txt"), "x\n").unwrap();
        sh(&owned.path, &["add", "."]);
        sh(&owned.path, &["commit", "-m", "b"]);
        let f = facts(&owned).await.unwrap();
        assert!(!f.in_base);
        // Kept: the commit is only on its branch.
        assert_eq!(remove(&owned, false, false, None).await, Ok(Removed { branch_kept: true, warning: None }));
        assert!(succeeds(&w.repo, &["rev-parse", "--verify", "-q", &format!("refs/heads/tenrec/{SID}")]).await);
        // Work that landed on main by a squash is "in" the base.
        let (second, _) = owned_in(&w, "ffeeddcc00112233").await;
        std::fs::write(second.path.join("c.txt"), "y\n").unwrap();
        sh(&second.path, &["add", "."]);
        sh(&second.path, &["commit", "-m", "c"]);
        sh(&w.repo, &["merge", "--squash", "tenrec/ffeeddcc00112233"]);
        sh(&w.repo, &["commit", "-m", "squashed"]);
        let f = facts(&second).await.unwrap();
        assert!(f.same_content && !f.in_base, "a squash merge looks done by content, but its commits are not in the history");
        assert_eq!(remove(&second, false, false, None).await, Ok(Removed { branch_kept: true, warning: None }), "content alone never deletes a branch unasked");
    }

    #[tokio::test]
    async fn uncommitted_work_needs_force_and_a_folder_deleted_by_hand_is_forgotten() {
        let w = world();
        let (owned, _) = owned_in(&w, SID).await;
        std::fs::write(owned.path.join("new.txt"), "x\n").unwrap();
        assert_eq!(remove(&owned, false, false, None).await, Err("uncommitted".to_owned()));
        assert!(owned.path.exists());
        std::fs::remove_dir_all(&owned.path).unwrap();
        let f = facts(&owned).await.unwrap();
        assert!(f.missing && f.blockers == ["missing"]);
        assert_eq!(remove(&owned, false, false, None).await, Ok(Removed { branch_kept: false, warning: None }), "nothing of its own on the branch, so it goes too");
        assert!(sh(&w.repo, &["worktree", "list"]).lines().count() == 1);
    }

    #[tokio::test]
    async fn hooks_and_the_file_system_monitor_of_the_project_do_not_run() {
        let w = world();
        let mark = w._tmp.path().join("ran");
        let hook = w.repo.join(".git").join("hooks").join("post-checkout");
        std::fs::write(&hook, format!("#!/bin/sh\ntouch {}\n", mark.display())).unwrap();
        std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let monitor = w._tmp.path().join("monitor.sh");
        std::fs::write(&monitor, format!("#!/bin/sh\ntouch {}\n", mark.display())).unwrap();
        std::fs::set_permissions(&monitor, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        sh(&w.repo, &["config", "core.fsmonitor", monitor.to_str().unwrap()]);
        let (owned, _) = owned_in(&w, SID).await;
        facts(&owned).await.unwrap();
        assert!(!mark.exists(), "neither ran");
    }

    #[tokio::test]
    async fn a_meta_file_that_points_elsewhere_is_not_believed() {
        let w = world();
        let (_, meta) = owned_in(&w, SID).await;
        let try_with = |edit: &dyn Fn(&mut Meta)| {
            let mut m = meta.clone();
            edit(&mut m);
            m
        };
        let outside = w._tmp.path().join("victim");
        std::fs::create_dir_all(&outside).unwrap();
        let other = w._tmp.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        sh(&other, &["-c", "init.defaultBranch=main", "init"]);
        for bad in [
            try_with(&|m| m.cwd = outside.clone()),
            try_with(&|m| m.worktree.as_mut().unwrap().branch = "main".into()),
            try_with(&|m| m.worktree.as_mut().unwrap().repo = other.canonicalize().unwrap()),
            try_with(&|m| m.sid = "ffffffffffffffff".into()),
            try_with(&|m| m.worktree = None),
        ] {
            assert!(verify(&bad, &w.root, true).await.is_err());
        }
        // A folder under the root that git never listed.
        let stray = w.root.join(repo_key(&w.repo)).join("aaaaaaaaaaaaaaaa");
        std::fs::create_dir_all(&stray).unwrap();
        let mut m = meta.clone();
        m.sid = "aaaaaaaaaaaaaaaa".into();
        m.cwd = stray.clone();
        m.worktree.as_mut().unwrap().branch = branch_for("aaaaaaaaaaaaaaaa");
        assert_eq!(verify(&m, &w.root, true).await.unwrap_err(), "not_owned");
        assert!(stray.exists());
        // The folder swapped for a link to another folder.
        let real = w.root.join(repo_key(&w.repo)).join(SID);
        std::fs::remove_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&outside, &real).unwrap();
        assert_eq!(verify(&meta, &w.root, true).await.unwrap_err(), "not_owned");
        assert!(outside.exists());
    }

    #[tokio::test]
    async fn a_failed_creation_leaves_nothing() {
        let w = world();
        sh(&w.repo, &["branch", "tenrec"]);
        let source = inspect_source(&w.repo).await.unwrap();
        let err = create(&w.root, &source.repo, SID, &source.commit).await.unwrap_err();
        assert!(!err.is_empty());
        assert!(!w.root.join(repo_key(&w.repo)).exists(), "the empty folders are gone");
        assert_eq!(sh(&w.repo, &["worktree", "list"]).lines().count(), 1);
    }

    #[test]
    fn the_worktree_list_is_read_with_nul_so_odd_names_stay_whole() {
        let text = "worktree /a/proj\0HEAD abc\0branch refs/heads/main\0\0worktree /a/we\nird\0HEAD def\0branch refs/heads/tenrec/x\0\0worktree /a/det\0HEAD 1\0detached\0\0";
        assert_eq!(parse_worktrees(text), vec![(PathBuf::from("/a/proj"), Some("main".into())), (PathBuf::from("/a/we\nird"), Some("tenrec/x".into())), (PathBuf::from("/a/det"), None)]);
    }

    #[tokio::test]
    async fn a_setting_that_can_run_a_program_is_noticed_and_an_ordinary_one_is_not() {
        let w = world();
        let (owned, meta) = owned_in(&w, SID).await;
        // Ordinary settings changing (a remote, a tracking branch, a name) are none of its business.
        sh(&w.repo, &["config", "branch.main.remote", "origin"]);
        sh(&w.repo, &["config", "user.name", "someone"]);
        assert!(verify(&meta, &w.root, true).await.is_ok());
        // A content filter written into the shared settings, as a command run in the chat could.
        let mark = w._tmp.path().join("ran");
        sh(&owned.path, &["config", "filter.x.clean", &format!("touch {}", mark.display())]);
        assert_eq!(verify(&meta, &w.root, true).await.unwrap_err(), "config_changed");
        assert!(!mark.exists(), "nothing ran while looking");
        assert!(verify(&meta, &w.root, false).await.is_ok(), "accepting is possible without the check");
        let now = fingerprint(&w.repo, &owned.path).await.unwrap();
        assert!(now.entries.iter().any(|e| e.starts_with("filter.x.clean=")));
        let mut accepted = meta.clone();
        accepted.worktree.as_mut().unwrap().config_hash = now.hash;
        assert!(verify(&accepted, &w.root, true).await.is_ok());
        // The attributes file of the project counts too.
        std::fs::write(w.repo.join(".git").join("info").join("attributes"), "* filter=x\n").unwrap();
        assert_eq!(verify(&accepted, &w.root, true).await.unwrap_err(), "config_changed");
    }

    #[tokio::test]
    async fn a_merge_setting_that_would_leave_the_work_out_does_not_get_its_way() {
        let w = world();
        let (owned, _) = owned_in(&w, SID).await;
        commit_in_test(&owned.path, "b.txt", "two\n");
        sh(&w.repo, &["config", "branch.main.mergeOptions", "-s ours"]);
        let f = facts(&owned).await.unwrap();
        let head = merge(&owned, &f.tip, &f.main_head).await.unwrap();
        assert!(w.repo.join("b.txt").exists(), "the work is in the project, not skipped");
        assert_eq!(sh(&w.repo, &["rev-list", "--parents", "-n", "1", &head]), format!("{head} {} {}", f.main_head, f.tip));
    }

    #[tokio::test]
    async fn a_merge_a_person_started_is_never_aborted_by_the_app() {
        let w = world();
        let (owned, _) = owned_in(&w, SID).await;
        std::fs::write(w.repo.join("a.txt"), "main\n").unwrap();
        sh(&w.repo, &["commit", "-am", "main"]);
        sh(&w.repo, &["checkout", "-q", "-b", "side", "HEAD~1"]);
        std::fs::write(w.repo.join("a.txt"), "side\n").unwrap();
        sh(&w.repo, &["commit", "-am", "side"]);
        sh(&w.repo, &["checkout", "-q", "main"]);
        let out = std::process::Command::new("git").current_dir(&w.repo).args(["merge", "side"]).output().unwrap();
        assert!(!out.status.success(), "a conflicting merge by hand");
        assert!(succeeds(&w.repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await);
        assert!(abort_ours(&w.repo, "0000000000000000000000000000000000000000").await.is_ok());
        assert!(succeeds(&w.repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await, "theirs is left alone");
        let f = facts(&owned).await.unwrap();
        assert!(f.mid_merge && f.blockers.contains(&"mid_merge"));
    }

    #[tokio::test]
    async fn a_merge_driver_that_keeps_ours_does_not_make_the_branch_deletable() {
        let w = world();
        std::fs::write(w.repo.join(".gitattributes"), "CHANGELOG merge=keep\n").unwrap();
        std::fs::write(w.repo.join("CHANGELOG"), "base\n").unwrap();
        sh(&w.repo, &["add", "."]);
        sh(&w.repo, &["commit", "-m", "changelog"]);
        sh(&w.repo, &["config", "merge.keep.driver", "true"]);
        let (owned, _) = owned_in(&w, SID).await;
        commit_in_test_file(&owned.path, "CHANGELOG", "branch\n");
        commit_in_test_file(&w.repo, "CHANGELOG", "main\n");
        let f = facts(&owned).await.unwrap();
        assert!(f.same_content && !f.in_base, "merging would change nothing, yet the commit is not in the history");
        let done = remove(&owned, false, false, None).await.unwrap();
        assert!(done.branch_kept, "the only copy of the commit stays");
    }

    #[tokio::test]
    async fn removing_one_chat_does_not_make_another_one_unremovable() {
        let w = world();
        let (a, _) = owned_in(&w, SID).await;
        let (b, meta_b) = owned_in(&w, "ffeeddcc00112233").await;
        std::fs::remove_dir_all(&a.path).unwrap();
        std::fs::remove_dir_all(&b.path).unwrap();
        assert_eq!(remove(&a, false, false, None).await.unwrap().warning, None);
        assert!(verify(&meta_b, &w.root, true).await.is_ok(), "the other registration was not pruned");
        let removed = remove(&b, false, false, None).await.unwrap();
        assert!(!removed.branch_kept);
        // Both are gone from git's list and from the disk: a chat may be marked without running anything.
        assert!(can_forget(&meta_b, &w.root).await);
        let (c, meta_c) = owned_in(&w, "0123456789abcdef").await;
        assert!(!can_forget(&meta_c, &w.root).await, "a worktree that exists is not forgotten");
        let _ = c;
    }

    #[tokio::test]
    async fn the_branch_is_judged_by_what_git_lists_after_a_removal_and_a_moved_branch_is_refused() {
        let w = world();
        let (owned, _) = owned_in(&w, SID).await;
        commit_in_test(&owned.path, "b.txt", "two\n");
        let f = facts(&owned).await.unwrap();
        commit_in_test(&owned.path, "c.txt", "three\n");
        assert_eq!(remove(&owned, false, true, Some(&f.tip)).await, Err("stale".to_owned()), "the person did not see the second commit");
        assert!(owned.path.exists());
        let done = remove(&owned, false, true, None).await.unwrap();
        assert_eq!(done, Removed { branch_kept: false, warning: None });
    }

    fn commit_in_test(dir: &Path, file: &str, text: &str) {
        commit_in_test_file(dir, file, text);
    }

    fn commit_in_test_file(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        sh(dir, &["add", "."]);
        sh(dir, &["commit", "-m", "change"]);
    }
}
