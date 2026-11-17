//! The file tree and the text editor's disk access: only inside one chat's project folder, only plain UTF-8 text files, links
//! resolved and checked again through an open directory (Linux: `/proc/self/fd`), a save written to a temporary file and renamed.
//!
//! It also creates an empty file or folder, deletes a file, link or empty folder and renames or moves an entry inside the project, all
//! through the folder's open handle with the last name never followed (a link stays a link). Writes under `.git` are refused; a few places that a later account chat
//! reads or runs (`.claude/`, `.mcp.json`, `CLAUDE.md`, `.env*`, ...) need a confirmation. That list is a pattern list for the
//! person's attention, not a wall: whoever holds the token can send the confirmation.
#[cfg(unix)]
use std::ffi::CString;
use std::hash::Hasher;
#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// The most text a file may hold to be opened or saved here (bytes).
pub const MAX_TEXT: usize = 1024 * 1024;
/// The project folder's trash: a delete moves the entry here (and a restore moves it back), nothing is copied.
pub const TRASH: &str = ".tenrec-trash";
/// The most entries one folder listing carries.
pub const MAX_ENTRIES: usize = 2000;

const HEAVY: [&str; 5] = [".git", "node_modules", "target", ".venv", "__pycache__"];

// ---- paths ----

/// A path relative to the project, as the page writes it: `a/b.txt`, or the empty text for the project folder itself.
/// No absolute path, no `.` or `..`, no empty part, no NUL.
pub fn rel_path(text: &str) -> Result<PathBuf, &'static str> {
    if text.is_empty() {
        return Ok(PathBuf::new());
    }
    if text.contains('\0') || text.len() > 4096 {
        return Err("the path is not valid");
    }
    let mut out = PathBuf::new();
    for part in text.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err("the path must be relative to the project, with no '.', '..' or empty part");
        }
        out.push(part);
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    Free,
    /// A save needs the person's confirmation: a later chat reads or runs what is written here.
    Confirm(&'static str),
    /// A save is refused.
    Block(&'static str),
}

/// What a save to this project-relative path means (ASCII case ignored, every part of the path looked at).
pub fn guard(rel: &Path) -> Guard {
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase()).collect();
    if parts.iter().any(|p| p == ".git") {
        return Guard::Block("a file inside .git can make git run a program (hooks, core.hooksPath, core.fsmonitor); use git itself");
    }
    if parts.iter().any(|p| p == ".claude") {
        return Guard::Confirm("settings, hooks, commands and skills of Claude Code: a later chat in this folder reads and may run them");
    }
    if parts.iter().any(|p| matches!(p.as_str(), ".vscode" | ".husky" | ".githooks")) {
        return Guard::Confirm("editor and git-hook settings that a program runs");
    }
    match parts.last().map(String::as_str) {
        Some(".mcp.json" | "claude.md" | "claude.local.md") => Guard::Confirm("a file Claude Code reads as its own instructions or MCP servers"),
        Some(".envrc" | ".pre-commit-config.yaml") => Guard::Confirm("a file a program runs when you enter this folder or commit"),
        Some(name) if name == ".env" || name.starts_with(".env.") => Guard::Confirm("a file that holds environment variables and often secrets"),
        _ => Guard::Free,
    }
}

fn is_secret_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == ".env" || n.starts_with(".env.") || n == ".envrc" || n == ".npmrc" || n == ".netrc" || n.starts_with("id_rsa") || n.starts_with("id_ed25519") || n.ends_with(".pem") || n.ends_with(".key")
}

// ---- listing ----

#[derive(Debug, Serialize)]
pub struct Entry {
    pub name: String,
    /// `dir`, `file` or `other` (a socket, a FIFO, a link that leaves the project, a broken link).
    pub kind: &'static str,
    pub link: bool,
    pub size: u64,
    pub flags: Vec<&'static str>,
    /// For an entry directly in the trash: the place it was deleted from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Listing {
    pub path: String,
    pub entries: Vec<Entry>,
    pub truncated: bool,
    /// Entries whose names are not UTF-8 (left out: a name that cannot go through JSON cannot be opened either).
    pub unreadable: usize,
}

fn flags_for(rel: &Path, name: &str) -> Vec<&'static str> {
    let mut flags = Vec::new();
    match guard(rel) {
        Guard::Block(_) => flags.push("blocked"),
        Guard::Confirm(_) => flags.push("guarded"),
        Guard::Free => {}
    }
    if is_secret_name(name) {
        flags.push("secret");
    }
    if HEAVY.contains(&name) {
        flags.push("heavy");
    }
    if name == TRASH && rel.components().count() == 1 {
        flags.push("trash");
    }
    flags
}

/// Where an entry of the trash came from (read from its record), for the listing of the trash itself.
#[cfg(unix)]
fn trash_origin(rel: &Path, real: &Path, name: &str) -> Option<String> {
    use std::os::unix::fs::OpenOptionsExt;
    if rel != Path::new(TRASH) {
        return None;
    }
    let id = trash_id(name)?;
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(real.join(".from").join(id)).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes).ok()?;
    String::from_utf8(bytes).ok().filter(|f| rel_path(f).is_ok_and(|r| !r.as_os_str().is_empty() && !r.starts_with(TRASH)))
}

#[cfg(not(unix))]
fn trash_origin(_rel: &Path, _real: &Path, _name: &str) -> Option<String> {
    None
}

fn canonical_root(project: &Path) -> Result<PathBuf, String> {
    project.canonicalize().map_err(|_| "the project folder cannot be opened".to_owned())
}

/// The real path of `rel` inside `root`, links resolved, or an error when it leaves the project.
fn inside(root: &Path, rel: &Path) -> Result<PathBuf, String> {
    let real = root.join(rel).canonicalize().map_err(|_| "no such file or folder".to_owned())?;
    if !real.starts_with(root) {
        return Err("that path leaves the project folder".to_owned());
    }
    Ok(real)
}

pub fn list_dir(project: &Path, rel_text: &str) -> Result<Listing, String> {
    list_dir_in(&canonical_root(project)?, rel_text)
}

/// `list_dir` in a project folder that was already resolved and checked (`root` is its real path and is not resolved again).
pub fn list_dir_in(root: &Path, rel_text: &str) -> Result<Listing, String> {
    let rel = rel_path(rel_text).map_err(str::to_owned)?;
    let root = root.to_path_buf();
    let real = inside(&root, &rel)?;
    if !real.is_dir() {
        return Err("not a folder".to_owned());
    }
    let mut entries = Vec::new();
    let mut unreadable = 0;
    let mut seen = 0;
    let mut truncated = false;
    for item in std::fs::read_dir(&real).map_err(|_| "the folder cannot be read".to_owned())?.flatten() {
        seen += 1;
        if seen > MAX_ENTRIES * 10 {
            truncated = true;
            break;
        }
        let Some(name) = item.file_name().to_str().map(str::to_owned) else {
            unreadable += 1;
            continue;
        };
        let Ok(meta) = std::fs::symlink_metadata(item.path()) else { continue };
        let link = meta.file_type().is_symlink();
        let (kind, size) = if link {
            match item.path().canonicalize() {
                Ok(target) if target.starts_with(&root) => match std::fs::metadata(&target) {
                    Ok(m) if m.is_dir() => ("dir", 0),
                    Ok(m) if m.is_file() => ("file", m.len()),
                    _ => ("other", 0),
                },
                _ => ("other", 0),
            }
        } else if meta.is_dir() {
            ("dir", 0)
        } else if meta.is_file() {
            ("file", meta.len())
        } else {
            ("other", 0)
        };
        let flags = flags_for(&rel.join(&name), &name);
        let from = trash_origin(&rel, &real, &name);
        entries.push(Entry { name, kind, link, size, flags, from });
    }
    entries.sort_by(|a, b| (a.kind != "dir").cmp(&(b.kind != "dir")).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.name.cmp(&b.name)));
    if entries.len() > MAX_ENTRIES {
        entries.truncate(MAX_ENTRIES);
        truncated = true;
    }
    Ok(Listing { path: rel_text.to_owned(), entries, truncated, unreadable })
}

// ---- reading ----

#[derive(Debug, Serialize)]
pub struct TextFile {
    pub text: String,
    pub version: String,
    /// `lf`, `crlf` or `mixed` (a lone CR counts as mixed): a page can only keep `lf` and `crlf` faithfully.
    pub eol: &'static str,
    pub guard: Option<&'static str>,
    pub guard_kind: &'static str,
    /// Where the file really is, relative to the project and with `/` (it differs from the asked path when a link was followed).
    pub real: String,
}

/// One number for one content, only meant to be compared within one run (SipHash with the standard fixed key) plus the length.
pub fn version(bytes: &[u8]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(bytes);
    format!("{}-{:016x}", bytes.len(), hasher.finish())
}

pub fn eol(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lone_cr = text.matches('\r').count() - crlf;
    let lf = text.matches('\n').count() - crlf;
    if lone_cr > 0 || (crlf > 0 && lf > 0) {
        "mixed"
    } else if crlf > 0 {
        "crlf"
    } else {
        "lf"
    }
}

#[cfg(unix)]
fn c_name(name: &std::ffi::OsStr) -> Result<CString, String> {
    CString::new(name.as_bytes()).map_err(|_| "the file name is not valid".to_owned())
}

#[cfg(unix)]
/// The folder of `real` opened without following a link, and checked to still be inside the project through the open handle: what is
/// done to the file afterwards goes through this handle, so a link put in place of a folder after the check changes nothing.
fn open_parent(root: &Path, real: &Path) -> Result<(OwnedFd, CString, PathBuf), String> {
    let parent = real.parent().ok_or("that is not a file")?;
    let name = c_name(real.file_name().ok_or("that is not a file")?)?;
    let path = CString::new(parent.as_os_str().as_bytes()).map_err(|_| "the path is not valid".to_owned())?;
    // SAFETY: `path` is a valid NUL-terminated string; the result is checked before it is used.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err("the folder cannot be opened".to_owned());
    }
    // SAFETY: `fd` is a fresh descriptor that nothing else owns.
    let dir = unsafe { OwnedFd::from_raw_fd(fd) };
    let now = std::fs::read_link(format!("/proc/self/fd/{}", dir.as_raw_fd())).map_err(|_| "the folder cannot be verified on this system".to_owned())?;
    if !now.starts_with(root) {
        return Err("that path leaves the project folder".to_owned());
    }
    Ok((dir, name, now))
}

#[cfg(unix)]
fn stat_at(dir: &OwnedFd, name: &CString) -> Result<libc::stat, String> {
    // SAFETY: a zeroed `stat` is a valid output buffer, and `name` is NUL-terminated.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstatat(dir.as_raw_fd(), name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) != 0 {
            return Err("no such file".to_owned());
        }
        Ok(st)
    }
}

#[cfg(unix)]
fn open_at(dir: &OwnedFd, name: &CString, flags: i32, mode: u32) -> std::io::Result<std::fs::File> {
    // SAFETY: `name` is NUL-terminated; the descriptor is checked and then owned by the File.
    let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags | libc::O_CLOEXEC | libc::O_NOFOLLOW, mode) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` is fresh and owned by nothing else.
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(unix)]
/// A plain file of the project, ready to read through its folder's handle.
struct Target {
    dir: OwnedFd,
    name: CString,
    rel: PathBuf,
    st: libc::stat,
}

#[cfg(unix)]
fn target(root: &Path, rel_text: &str) -> Result<Target, String> {
    let rel = rel_path(rel_text).map_err(str::to_owned)?;
    if rel.as_os_str().is_empty() {
        return Err("that is the project folder, not a file".to_owned());
    }
    let root = root.to_path_buf();
    let real = inside(&root, &rel)?;
    if real == root {
        return Err("that is the project folder, not a file".to_owned());
    }
    let (dir, name, folder) = open_parent(&root, &real)?;
    let st = stat_at(&dir, &name)?;
    if st.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err("not a plain file".to_owned());
    }
    // The guard looks at where the open folder really is (not at the path as it was when it was first resolved).
    let real_rel = folder.strip_prefix(&root).map_err(|_| "that path leaves the project folder".to_owned())?.join(std::ffi::OsStr::from_bytes(name.as_bytes()));
    Ok(Target { dir, name, rel: real_rel, st })
}

#[cfg(unix)]
fn read_bytes(t: &Target) -> Result<Vec<u8>, String> {
    // O_NONBLOCK: a file swapped for a FIFO after the check does not stop this thread; the open handle is checked again.
    let file = open_at(&t.dir, &t.name, libc::O_RDONLY | libc::O_NONBLOCK, 0).map_err(|_| "the file cannot be opened".to_owned())?;
    let meta = file.metadata().map_err(|_| "the file cannot be read".to_owned())?;
    if !meta.is_file() {
        return Err("not a plain file".to_owned());
    }
    {
        use std::os::unix::fs::MetadataExt;
        #[allow(clippy::unnecessary_cast)] // no-ops on 64-bit Linux; `stat` is narrower on some targets
        let replaced = meta.ino() != t.st.st_ino as u64 || meta.dev() != t.st.st_dev as u64;
        if replaced {
            return Err("the file was replaced while it was being opened".to_owned());
        }
    }
    let mut bytes = Vec::new();
    file.take(MAX_TEXT as u64 + 1).read_to_end(&mut bytes).map_err(|_| "the file cannot be read".to_owned())?;
    if bytes.len() > MAX_TEXT {
        return Err(format!("the file is bigger than {} KiB, which the editor does not open", MAX_TEXT / 1024));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn decode(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.contains(&0) {
        return Err("the file is binary, not text".to_owned());
    }
    String::from_utf8(bytes).map_err(|_| "the file is not UTF-8 text".to_owned())
}

#[cfg(unix)]
pub fn read_text(project: &Path, rel_text: &str) -> Result<TextFile, String> {
    read_text_in(&canonical_root(project)?, rel_text)
}

/// The version of a file as it is on disk now, without decoding or classifying it (the editor's "did it change" check).
#[cfg(unix)]
pub fn version_in(root: &Path, rel_text: &str) -> Result<String, String> {
    let t = target(root, rel_text)?;
    Ok(version(&read_bytes(&t)?))
}

/// `read_text` in a project folder that was already resolved and checked (`root` is its real path and is not resolved again).
#[cfg(unix)]
pub fn read_text_in(root: &Path, rel_text: &str) -> Result<TextFile, String> {
    let t = target(root, rel_text)?;
    let bytes = read_bytes(&t)?;
    let version = version(&bytes);
    let text = decode(bytes)?;
    let (guard, guard_kind) = match guard(&t.rel) {
        Guard::Free => (None, ""),
        Guard::Confirm(why) => (Some(why), "confirm"),
        Guard::Block(why) => (Some(why), "block"),
    };
    let real = t.rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
    Ok(TextFile { eol: eol(&text), version, text, guard, guard_kind, real })
}

// ---- writing ----

#[derive(Debug, PartialEq, Eq)]
pub enum Saved {
    Saved { version: String },
    /// The file on disk is no longer what the page opened.
    Conflict { text: String, version: String },
    NeedsConfirm(&'static str),
}

/// One save at a time in this process: the last version check and the replacing rename are not one step, and two saves of the
/// same file from two connections could otherwise both pass the check.
#[cfg(unix)]
static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(unix)]
/// Replaces an existing plain file's text. `base` is the version the page opened; `confirm` answers a `NeedsConfirm`.
pub fn write_text(project: &Path, rel_text: &str, text: &str, base: &str, confirm: bool) -> Result<Saved, String> {
    write_text_in(&canonical_root(project)?, rel_text, text, base, confirm)
}

/// `write_text` in a project folder that was already resolved and checked (`root` is its real path and is not resolved again).
#[cfg(unix)]
pub fn write_text_in(root: &Path, rel_text: &str, text: &str, base: &str, confirm: bool) -> Result<Saved, String> {
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    if text.len() > MAX_TEXT {
        return Err(format!("the text is bigger than {} KiB", MAX_TEXT / 1024));
    }
    if text.contains('\0') {
        return Err("the text holds a NUL character".to_owned());
    }
    let t = target(root, rel_text)?;
    match guard(&t.rel) {
        Guard::Block(why) => return Err(format!("not saved: {why}")),
        Guard::Confirm(why) if !confirm => return Ok(Saved::NeedsConfirm(why)),
        _ => {}
    }
    // SAFETY: plain libc call without arguments.
    if t.st.st_uid != unsafe { libc::geteuid() } {
        return Err("the file belongs to another user".to_owned());
    }
    // A rename ignores the file's own write bit, so it is looked at here.
    if t.st.st_mode & 0o200 == 0 {
        return Err("the file is read-only".to_owned());
    }
    let current = read_bytes(&t)?;
    let current_version = version(&current);
    if current_version != base {
        return Ok(Saved::Conflict { text: decode(current)?, version: current_version });
    }

    let tmp = c_name(std::ffi::OsStr::new(&format!(".tenrec-{}.tmp", crate::random_hex(8))))?;
    let mut file = open_at(&t.dir, &tmp, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600).map_err(|_| "a temporary file cannot be made in that folder".to_owned())?;
    let remove_tmp = || {
        // SAFETY: both arguments are valid; failure leaves the temporary file, which is all that can be done.
        unsafe { libc::unlinkat(t.dir.as_raw_fd(), tmp.as_ptr(), 0) };
    };
    let written = (|| -> std::io::Result<()> {
        file.write_all(text.as_bytes())?;
        // The old permission bits, without setuid, setgid and sticky.
        // The old group where it can be kept (a shared-group project); a failure is ignored, the file is still the person's.
        // SAFETY: `file` is an open descriptor; (uid_t)-1 and gid leave the owner as it is.
        unsafe { libc::fchown(file.as_raw_fd(), libc::uid_t::MAX, t.st.st_gid) };
        // SAFETY: `file` is an open descriptor.
        if unsafe { libc::fchmod(file.as_raw_fd(), (t.st.st_mode & 0o777) as libc::mode_t) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        file.sync_all()
    })();
    drop(file);
    if written.is_err() {
        remove_tmp();
        return Err("the file could not be written".to_owned());
    }
    // The last look before the rename: whoever wrote in the time the temporary file took is not overwritten without a word.
    match read_bytes(&t) {
        Ok(now) if version(&now) == base => {}
        Ok(now) => {
            remove_tmp();
            return Ok(Saved::Conflict { version: version(&now), text: decode(now)? });
        }
        Err(e) => {
            remove_tmp();
            return Err(e);
        }
    }
    // SAFETY: all four arguments are valid; the same folder handle serves as source and target folder.
    let renamed = unsafe { libc::renameat(t.dir.as_raw_fd(), tmp.as_ptr(), t.dir.as_raw_fd(), t.name.as_ptr()) };
    if renamed != 0 {
        remove_tmp();
        return Err("the file could not be replaced".to_owned());
    }
    // So the rename survives a power cut too; the file itself was synced above.
    // SAFETY: the folder handle is open.
    unsafe { libc::fsync(t.dir.as_raw_fd()) };
    Ok(Saved::Saved { version: version(text.as_bytes()) })
}

// ---- create, delete, rename ----

/// The answer of `create_in`, `delete_in` and `rename_in`.
#[derive(Debug, PartialEq, Eq)]
pub enum Changed {
    Done,
    /// Done, and the entry is now at this path (a delete into the trash, a restore).
    Moved(String),
    /// The change touches a place a later chat reads or runs: it is made only with the person's confirmation.
    NeedsConfirm(&'static str),
}

/// A name that may be given to a new file, folder or renamed entry (one part of a path, not a path).
pub fn new_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() || name.len() > 255 {
        return Err("a name is 1 to 255 bytes long");
    }
    if name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err("that is not a name");
    }
    if name.chars().any(|c| c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')) {
        return Err("a name holds no control or direction characters");
    }
    if name.trim() != name {
        return Err("a name does not start or end with a space");
    }
    if name.starts_with(".tenrec-") && name.ends_with(".tmp") {
        return Err("that name is used for the app's temporary files");
    }
    Ok(())
}

/// What `guard` says for a change at `rel`: refused, or asks for a confirmation unless `confirm` is given.
#[cfg(unix)]
fn check_change(rel: &Path, confirm: bool) -> Result<Option<&'static str>, String> {
    match guard(rel) {
        Guard::Block(why) => Err(format!("not changed: {why}")),
        Guard::Confirm(why) if !confirm => Ok(Some(why)),
        _ => Ok(None),
    }
}

#[cfg(unix)]
/// An entry named by a path whose last part is *not* resolved (a link stays a link): its folder is opened without following a
/// link and checked to be inside the project, and the entry itself is only looked at with `fstatat(AT_SYMLINK_NOFOLLOW)`.
struct Place {
    dir: OwnedFd,
    name: CString,
    /// The path of the entry as the open folder really is (what the guard looks at).
    rel: PathBuf,
    st: Option<libc::stat>,
}

#[cfg(unix)]
fn place(root: &Path, rel_text: &str) -> Result<Place, String> {
    let rel = rel_path(rel_text).map_err(str::to_owned)?;
    let name = rel.file_name().ok_or_else(|| "that is the project folder".to_owned())?.to_owned();
    let parent = inside(root, rel.parent().unwrap_or_else(|| Path::new("")))?;
    let (dir, cname, folder) = open_parent(root, &parent.join(&name))?;
    let st = stat_at(&dir, &cname).ok();
    let real_rel = folder.strip_prefix(root).map_err(|_| "that path leaves the project folder".to_owned())?.join(&name);
    Ok(Place { dir, name: cname, rel: real_rel, st })
}

#[cfg(unix)]
fn fsync_dir(dir: &OwnedFd) {
    // SAFETY: the folder handle is open; a failure only means the change may not survive a power cut.
    unsafe { libc::fsync(dir.as_raw_fd()) };
}

#[cfg(unix)]
/// Makes an empty plain file (`kind` "file") or an empty folder ("dir") that does not exist yet, in a folder that does.
pub fn create_in(root: &Path, rel_text: &str, kind: &str, confirm: bool) -> Result<Changed, String> {
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    let rel = rel_path(rel_text).map_err(str::to_owned)?;
    new_name(&rel.file_name().ok_or_else(|| "that is the project folder".to_owned())?.to_string_lossy()).map_err(str::to_owned)?;
    if kind != "file" && kind != "dir" {
        return Err("unknown kind".to_owned());
    }
    let p = place(root, rel_text)?;
    if let Some(why) = check_change(&p.rel, confirm)? {
        return Ok(Changed::NeedsConfirm(why));
    }
    if p.st.is_some() {
        return Err("that name already exists".to_owned());
    }
    if kind == "file" {
        // O_EXCL also refuses a dangling link in that place, so nothing is made behind a link.
        let file = open_at(&p.dir, &p.name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o666).map_err(|e| io_sentence(&e, "the file could not be made"))?;
        drop(file);
    } else {
        // SAFETY: `name` is NUL-terminated and the folder handle is open.
        if unsafe { libc::mkdirat(p.dir.as_raw_fd(), p.name.as_ptr(), 0o777) } != 0 {
            return Err(io_sentence(&std::io::Error::last_os_error(), "the folder could not be made"));
        }
    }
    fsync_dir(&p.dir);
    Ok(Changed::Done)
}

#[cfg(unix)]
fn io_sentence(e: &std::io::Error, fallback: &str) -> String {
    match e.raw_os_error() {
        Some(libc::EEXIST) => "that name already exists".to_owned(),
        Some(libc::ENOENT) => "the folder does not exist".to_owned(),
        Some(libc::EACCES | libc::EPERM) => "not allowed on this folder".to_owned(),
        Some(libc::ENOTEMPTY) => "the folder is not empty".to_owned(),
        _ => fallback.to_owned(),
    }
}

#[cfg(unix)]
/// What may be deleted or renamed: a plain file, a folder or a link (the link itself, never what it points to), owned by the person.
fn changeable(st: &libc::stat) -> Result<libc::mode_t, String> {
    let kind = st.st_mode & libc::S_IFMT;
    if kind != libc::S_IFREG && kind != libc::S_IFDIR && kind != libc::S_IFLNK {
        return Err("not a plain file or folder".to_owned());
    }
    // SAFETY: plain libc call without arguments.
    if st.st_uid != unsafe { libc::geteuid() } {
        return Err("it belongs to another user".to_owned());
    }
    Ok(kind)
}

#[cfg(unix)]
/// Deletes an entry. A file, a link or a folder of any size goes to the project's trash (`TRASH`), from where `restore_in` can bring
/// it back; inside the trash a delete is for good, and only a file, a link or an empty folder can be deleted that way. `base` is the
/// version the page opened, for the file that is open in the editor: a file that changed since is not deleted.
pub fn delete_in(root: &Path, rel_text: &str, base: Option<&str>, confirm: bool) -> Result<Changed, String> {
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    let p = place(root, rel_text)?;
    let st = p.st.ok_or_else(|| "no such file or folder".to_owned())?;
    if let Some(why) = check_change(&p.rel, confirm)? {
        return Ok(Changed::NeedsConfirm(why));
    }
    let kind = changeable(&st)?;
    let for_good = p.rel.starts_with(TRASH);
    if p.rel == Path::new(TRASH) {
        return Err("the trash itself is not deleted here; use Empty trash (nothing was deleted)".to_owned());
    }
    // The person was asked in the words of the path they sent: a name that reaches the trash only through a link must not be
    // deleted for good under the words "move to the trash".
    if for_good && !rel_path(rel_text).map_err(str::to_owned)?.starts_with(TRASH) {
        return Err("it is inside the trash through a link; delete it from the trash itself (nothing was deleted)".to_owned());
    }
    if for_good && kind == libc::S_IFREG && st.st_mode & 0o200 == 0 {
        return Err("the file is read-only".to_owned());
    }
    let t = Target { dir: p.dir, name: p.name, rel: p.rel, st };
    if let (Some(base), true) = (base, kind == libc::S_IFREG) {
        if version(&read_bytes(&t)?) != base {
            return Err("the file changed since it was opened, so it was not deleted".to_owned());
        }
    }
    if !for_good {
        return trash_entry(root, &t, kind);
    }
    // Looked at once more just before: what is unlinked is what was checked. (A swap in the instant between this look and the call
    // cannot be closed on Linux, which has no unlink by descriptor.)
    let again = stat_at(&t.dir, &t.name)?;
    if again.st_ino != t.st.st_ino || again.st_dev != t.st.st_dev || again.st_mode & libc::S_IFMT != kind {
        return Err("the file was replaced meanwhile, so it was not deleted".to_owned());
    }
    let flags = if kind == libc::S_IFDIR { libc::AT_REMOVEDIR } else { 0 };
    // SAFETY: `name` is NUL-terminated and the folder handle is open.
    if unsafe { libc::unlinkat(t.dir.as_raw_fd(), t.name.as_ptr(), flags) } != 0 {
        return Err(io_sentence(&std::io::Error::last_os_error(), "it could not be deleted"));
    }
    fsync_dir(&t.dir);
    // The record of where it came from goes with it (kept out of a leftover pile).
    if t.rel.parent() == Some(Path::new(TRASH)) {
        drop_record(&t.dir, &String::from_utf8_lossy(t.name.as_bytes()));
    }
    Ok(Changed::Done)
}

#[cfg(unix)]
/// Removes the record `.from/<id>` of the trash entry `name` (the folder `trash` is open); a record that stays is harmless.
fn drop_record(trash: &OwnedFd, name: &str) {
    let Some(id) = trash_id(name) else { return };
    let (Ok(froms), Ok(record)) = (open_at(trash, &CString::new(".from").unwrap_or_default(), libc::O_RDONLY | libc::O_DIRECTORY, 0), CString::new(id)) else { return };
    // SAFETY: the folder is open and the name is NUL-terminated.
    unsafe { libc::unlinkat(froms.as_raw_fd(), record.as_ptr(), 0) };
}

#[cfg(unix)]
/// A folder of the project's trash that is made when needed, opened without following a link, and checked to be a folder of the
/// person's own.
fn trash_dir(parent: &OwnedFd, name: &str) -> Result<OwnedFd, String> {
    let cname = CString::new(name).map_err(|_| "the trash folder cannot be made".to_owned())?;
    // SAFETY: `cname` is NUL-terminated and the folder handle is open.
    if unsafe { libc::mkdirat(parent.as_raw_fd(), cname.as_ptr(), 0o700) } == 0 {
        fsync_dir(parent);
    } else if std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
        return Err("the trash folder cannot be made".to_owned());
    }
    let bad = || format!("{TRASH} is not a folder of yours, so nothing was moved");
    let file = open_at(parent, &cname, libc::O_RDONLY | libc::O_DIRECTORY, 0).map_err(|_| bad())?;
    let meta = file.metadata().map_err(|_| bad())?;
    // SAFETY: plain libc call without arguments.
    if !meta.is_dir() || std::os::unix::fs::MetadataExt::uid(&meta) != unsafe { libc::geteuid() } {
        return Err(bad());
    }
    Ok(OwnedFd::from(file))
}

#[cfg(unix)]
/// The id a trash entry carries in its name: `<unix ms>-<8 hex>` before the original name.
fn trash_id(name: &str) -> Option<&str> {
    let (ms, rest) = name.split_once('-')?;
    let hex = rest.get(..8)?;
    let ok = !ms.is_empty() && ms.len() <= 20 && ms.bytes().all(|b| b.is_ascii_digit()) && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) && rest.as_bytes().get(8) == Some(&b'-');
    ok.then(|| &name[..ms.len() + 1 + 8])
}

#[cfg(unix)]
/// Moves `t` into the trash: the place it came from is written down first (`.from/<id>`), then the entry is renamed to
/// `<id>-<name>` in the trash. Atomic, never copies and never goes into a folder.
fn trash_entry(root: &Path, t: &Target, kind: libc::mode_t) -> Result<Changed, String> {
    let from = t.rel.to_str().ok_or_else(|| "the place cannot be recorded, so it cannot go to the trash".to_owned())?.to_owned();
    let (home, _, _) = open_parent(root, &root.join(TRASH))?;
    let trash = trash_dir(&home, TRASH)?;
    let froms = trash_dir(&trash, ".from")?;
    // A `.gitignore` that ignores everything, itself included: git status stays clean and `.git` is not touched. One that is already
    // there was made or changed by the person and is left alone.
    if let Ok(mut f) = open_at(&trash, &c_name(std::ffi::OsStr::new(".gitignore"))?, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600) {
        let _ = f.write_all(b"*\n");
    }
    let mut id = String::new();
    let mut sidecar = None;
    for _ in 0..3 {
        id = format!("{}-{}", crate::now_ms(), crate::random_hex(4));
        let name = c_name(std::ffi::OsStr::new(&id))?;
        // O_EXCL: the id is unique, or the next one is tried.
        if let Ok(f) = open_at(&froms, &name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600) {
            sidecar = Some((f, name));
            break;
        }
    }
    let (mut file, sidecar_name) = sidecar.ok_or_else(|| "the trash cannot be written to".to_owned())?;
    if file.write_all(from.as_bytes()).and_then(|()| file.sync_all()).is_err() {
        // SAFETY: both arguments are valid.
        unsafe { libc::unlinkat(froms.as_raw_fd(), sidecar_name.as_ptr(), 0) };
        return Err("the trash cannot be written to".to_owned());
    }
    drop(file);
    fsync_dir(&froms);
    let original = String::from_utf8_lossy(t.name.as_bytes()).into_owned();
    let mut entry = format!("{id}-{original}");
    while entry.len() > 255 {
        entry.pop();
    }
    let entry_name = c_name(std::ffi::OsStr::new(&entry))?;
    let outcome = stat_at(&t.dir, &t.name).and_then(|again| {
        if again.st_ino != t.st.st_ino || again.st_dev != t.st.st_dev || again.st_mode & libc::S_IFMT != kind {
            Err("the file was replaced meanwhile, so it was not deleted".to_owned())
        } else {
            renameat2_noreplace(&t.dir, &t.name, &trash, &entry_name, kind).map_err(|e| e.replace("cannot be moved there", "cannot go to the trash").replace("(nothing was copied)", "(nothing was deleted)"))
        }
    });
    if let Err(e) = outcome {
        // SAFETY: both arguments are valid; a leftover record is harmless.
        unsafe { libc::unlinkat(froms.as_raw_fd(), sidecar_name.as_ptr(), 0) };
        return Err(e);
    }
    fsync_dir(&t.dir);
    fsync_dir(&trash);
    Ok(Changed::Moved(format!("{TRASH}/{entry}")))
}

#[cfg(unix)]
/// The place a trash entry came from, as `trash_entry` wrote it down (`None` when the record is missing or not usable).
fn recorded_from(froms: &OwnedFd, id: &str) -> Option<String> {
    let name = c_name(std::ffi::OsStr::new(id)).ok()?;
    let file = open_at(froms, &name, libc::O_RDONLY | libc::O_NONBLOCK, 0).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes).ok()?;
    let from = String::from_utf8(bytes).ok()?;
    let rel = rel_path(&from).ok()?;
    (!rel.as_os_str().is_empty() && !rel.starts_with(TRASH)).then_some(from)
}

#[cfg(unix)]
/// Brings an entry that is directly in the trash back to where it came from. Refused when something has that name now or the folder
/// is gone; a place a later chat reads asks first, as for any move into it.
pub fn restore_in(root: &Path, rel_text: &str, confirm: bool) -> Result<Changed, String> {
    restore_in_to(root, rel_text, confirm).map(|(changed, _)| changed)
}

#[cfg(unix)]
/// `restore_in`, and where the entry would go (given when a confirmation is asked, so the person sees the place).
pub fn restore_in_to(root: &Path, rel_text: &str, confirm: bool) -> Result<(Changed, Option<String>), String> {
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    let p = place(root, rel_text)?;
    let st = p.st.ok_or_else(|| "no such file or folder".to_owned())?;
    let name = String::from_utf8_lossy(p.name.as_bytes()).into_owned();
    let id = trash_id(&name).filter(|_| p.rel.parent() == Some(Path::new(TRASH))).ok_or_else(|| format!("only an entry directly in {TRASH} can be restored"))?.to_owned();
    let froms = open_at(&p.dir, &c_name(std::ffi::OsStr::new(".from"))?, libc::O_RDONLY | libc::O_DIRECTORY, 0).map(OwnedFd::from).map_err(|_| "where it came from is not known; move it out with rename".to_owned())?;
    let from = recorded_from(&froms, &id).ok_or_else(|| "where it came from is not known; move it out with rename".to_owned())?;
    let q = place(root, &from).map_err(|e| if e == "no such file or folder" { "the folder it came from no longer exists".to_owned() } else { e })?;
    if q.st.is_some() {
        return Err("something has that name now; move it out of the trash with rename".to_owned());
    }
    match move_place(&p, &st, &q, confirm)? {
        Changed::Done => {
            if let Ok(record) = c_name(std::ffi::OsStr::new(&id)) {
                // SAFETY: both arguments are valid; a record that stays is harmless.
                unsafe { libc::unlinkat(froms.as_raw_fd(), record.as_ptr(), 0) };
            }
            Ok((Changed::Moved(from), None))
        }
        other => Ok((other, Some(from))),
    }
}

// ---- deleting from the trash for good ----

/// The most entries (files, links and folders, counted all together) one purge walks through, and the deepest folder it goes into.
pub const MAX_PURGE: usize = if cfg!(test) { 300 } else { 100_000 };
#[cfg(unix)]
const MAX_DEPTH: usize = 64;

/// What `purge_in` and `empty_trash_in` answer: what a delete would reach (asked without `confirm`), or what was done.
#[derive(Debug, PartialEq, Eq)]
pub enum Purged {
    Summary { entries: usize, items: usize, bytes: u64, left: usize },
    Done { removed: usize, left: usize },
}

#[cfg(unix)]
#[derive(Default)]
struct Tally {
    items: usize,
    bytes: u64,
    removed: usize,
    left: usize,
}

#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Walk {
    /// Only counts; refuses what a delete would not do whole (a mount point, too deep, too many).
    Count,
    Delete,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn errno_ptr() -> *mut libc::c_int {
    // SAFETY: gives the calling thread's errno.
    unsafe { libc::__errno_location() }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn errno_ptr() -> *mut libc::c_int {
    // SAFETY: gives the calling thread's errno.
    unsafe { libc::__error() }
}

#[cfg(unix)]
/// The names in a folder (not `.` and `..`), read through a fresh descriptor of it and all collected before anything is changed.
fn read_names(dir: &OwnedFd, cap: usize) -> Result<Vec<CString>, String> {
    let bad = || "a folder cannot be read".to_owned();
    // SAFETY: "." is NUL-terminated; a new descriptor of the same folder, with its own read position, owned below.
    let fd = unsafe { libc::openat(dir.as_raw_fd(), c".".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(bad());
    }
    // SAFETY: `fd` is a folder descriptor that `fdopendir` takes over (closed by `closedir`).
    let handle = unsafe { libc::fdopendir(fd) };
    if handle.is_null() {
        // SAFETY: `fd` was not taken over.
        unsafe { libc::close(fd) };
        return Err(bad());
    }
    let mut names = Vec::new();
    loop {
        // SAFETY: `handle` is open; errno is cleared so the end of the folder and an error can be told apart.
        let entry = unsafe {
            *errno_ptr() = 0;
            libc::readdir(handle)
        };
        if entry.is_null() {
            // SAFETY: as above.
            let failed = unsafe { *errno_ptr() } != 0;
            // SAFETY: `handle` is open.
            unsafe { libc::closedir(handle) };
            return if failed { Err(bad()) } else { Ok(names) };
        }
        // SAFETY: `readdir` gives a valid entry whose name is NUL-terminated.
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_owned();
        if name.as_bytes() != b"." && name.as_bytes() != b".." {
            names.push(name);
            if names.len() > cap {
                // A folder with more names than the walk may take: not collected to the end (the caller sees it is over).
                // SAFETY: `handle` is open.
                unsafe { libc::closedir(handle) };
                return Ok(names);
            }
        }
    }
}

#[cfg(unix)]
fn fstat_fd(fd: &OwnedFd) -> Option<libc::stat> {
    // SAFETY: a zeroed `stat` is a valid output buffer and `fd` is open.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        (libc::fstat(fd.as_raw_fd(), &mut st) == 0).then_some(st)
    }
}

#[cfg(unix)]
/// One entry of a folder, walked or removed: a link or a file is unlinked (a link as a link, never followed), a folder is gone into
/// through its own descriptor (no link followed, never `..`) and removed when it is empty. What cannot be removed (another user's,
/// a folder that is not rwx for its owner, a failure) is counted and left, never changed with `chmod`.
fn purge_one(dir: &OwnedFd, name: &CString, top_dev: libc::dev_t, depth: usize, mode: Walk, tally: &mut Tally) -> Result<bool, String> {
    if tally.items >= MAX_PURGE {
        return if mode == Walk::Count { Err(format!("more than {MAX_PURGE} entries")) } else { tally.left += 1; Ok(false) };
    }
    let Ok(st) = stat_at(dir, name) else {
        tally.left += 1;
        return Ok(false);
    };
    tally.items += 1;
    // SAFETY: plain libc call without arguments.
    let mine = st.st_uid == unsafe { libc::geteuid() };
    if st.st_mode & libc::S_IFMT != libc::S_IFDIR {
        tally.bytes += if st.st_mode & libc::S_IFMT == libc::S_IFREG { st.st_size.max(0) as u64 } else { 0 };
        if !mine {
            tally.left += 1;
            return Ok(false);
        }
        if mode == Walk::Delete {
            // SAFETY: `name` is NUL-terminated and the folder is open.
            if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) } == 0 {
                tally.removed += 1;
                return Ok(true);
            }
            tally.left += 1;
        }
        return Ok(false);
    }
    if st.st_dev != top_dev {
        return if mode == Walk::Count { Err("a mount point is inside it, so nothing was deleted".to_owned()) } else { tally.left += 1; Ok(false) };
    }
    if depth >= MAX_DEPTH {
        return if mode == Walk::Count { Err(format!("folders deeper than {MAX_DEPTH}")) } else { tally.left += 1; Ok(false) };
    }
    if !mine || st.st_mode & 0o700 != 0o700 {
        tally.left += 1;
        return Ok(false);
    }
    let Ok(child) = open_at(dir, name, libc::O_RDONLY | libc::O_DIRECTORY, 0).map(OwnedFd::from) else {
        tally.left += 1;
        return Ok(false);
    };
    // What was looked at is what was opened.
    if !fstat_fd(&child).is_some_and(|now| now.st_ino == st.st_ino && now.st_dev == st.st_dev) {
        tally.left += 1;
        return Ok(false);
    }
    let names = match read_names(&child, MAX_PURGE.saturating_sub(tally.items) + 1) {
        Ok(names) => names,
        // While deleting, a folder that cannot be read is left (and counted) like any other; the count refuses it before.
        Err(_) if mode == Walk::Delete => {
            tally.left += 1;
            return Ok(false);
        }
        Err(e) => return Err(e),
    };
    if names.len() > MAX_PURGE.saturating_sub(tally.items) {
        return if mode == Walk::Count { Err(format!("more than {MAX_PURGE} entries")) } else { tally.left += 1; Ok(false) };
    }
    for inner in names {
        purge_one(&child, &inner, top_dev, depth + 1, mode, tally)?;
    }
    drop(child);
    if mode == Walk::Delete {
        // SAFETY: `name` is NUL-terminated and the folder is open; only an empty folder is removed.
        if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } == 0 {
            tally.removed += 1;
            return Ok(true);
        }
        tally.left += 1;
    }
    Ok(false)
}

#[cfg(unix)]
/// The trash folder of the project, opened without following a link and checked to be the person's own.
fn open_trash(root: &Path) -> Result<OwnedFd, String> {
    let (home, _, folder) = open_parent(root, &root.join(TRASH))?;
    if folder != root {
        return Err("that path leaves the project folder".to_owned());
    }
    let file = open_at(&home, &c_name(std::ffi::OsStr::new(TRASH))?, libc::O_RDONLY | libc::O_DIRECTORY, 0).map_err(|_| "there is no trash".to_owned())?;
    let trash = OwnedFd::from(file);
    // SAFETY: plain libc call without arguments.
    if !fstat_fd(&trash).is_some_and(|st| st.st_uid == unsafe { libc::geteuid() }) {
        return Err(format!("{TRASH} is not a folder of yours"));
    }
    Ok(trash)
}

#[cfg(unix)]
/// Deletes an entry that is directly in the trash for good, with everything in it. Without `confirm` nothing is changed: what the
/// delete would reach is counted and answered. With it the count is made again and nothing is deleted when it is over the limits
/// (`MAX_PURGE` entries, `MAX_DEPTH` folders deep, no mount point inside). Links are not followed; a folder that is not the person's
/// or not rwx for them is left, and counted as left.
pub fn purge_in(root: &Path, rel_text: &str, confirm: bool) -> Result<Purged, String> {
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    let rel = rel_path(rel_text).map_err(str::to_owned)?;
    if rel.parent() != Some(Path::new(TRASH)) {
        return Err(format!("only an entry directly in {TRASH} can be deleted for good"));
    }
    let p = place(root, rel_text)?;
    // The same path as it really is: a link that leads into the trash is not a way to delete for good under other words.
    let name = String::from_utf8_lossy(p.name.as_bytes()).into_owned();
    if p.rel.parent() != Some(Path::new(TRASH)) || name == ".from" || name == ".gitignore" || p.st.is_none() {
        return Err(format!("only an entry directly in {TRASH} can be deleted for good"));
    }
    let trash = open_trash(root)?;
    let top_dev = fstat_fd(&trash).map(|st| st.st_dev).ok_or("the trash cannot be read")?;
    let mut count = Tally::default();
    purge_one(&trash, &p.name, top_dev, 0, Walk::Count, &mut count)?;
    if !confirm {
        return Ok(Purged::Summary { entries: 1, items: count.items, bytes: count.bytes, left: count.left });
    }
    let mut done = Tally::default();
    let gone = purge_one(&trash, &p.name, top_dev, 0, Walk::Delete, &mut done)?;
    if gone {
        drop_record(&trash, &name);
    }
    Ok(Purged::Done { removed: done.removed, left: done.left })
}

#[cfg(unix)]
/// Deletes everything in the trash for good (the trash folder and its `.gitignore` stay), with the same counting, limits and
/// refusals as `purge_in` over all the entries together; records that have no entry any more are removed too.
pub fn empty_trash_in(root: &Path, rel_text: &str, confirm: bool) -> Result<Purged, String> {
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    if rel_path(rel_text).map_err(str::to_owned)? != Path::new(TRASH) {
        return Err(format!("only {TRASH} can be emptied"));
    }
    let trash = open_trash(root)?;
    let top_dev = fstat_fd(&trash).map(|st| st.st_dev).ok_or("the trash cannot be read")?;
    let names: Vec<CString> = read_names(&trash, MAX_PURGE + 2)?.into_iter().filter(|n| n.as_bytes() != b".from" && n.as_bytes() != b".gitignore").collect();
    let mut count = Tally::default();
    for name in &names {
        purge_one(&trash, name, top_dev, 0, Walk::Count, &mut count)?;
    }
    if !confirm {
        if names.len() > MAX_PURGE {
            return Err(format!("more than {MAX_PURGE} entries in the trash: delete the biggest in a file manager or a shell"));
        }
        return Ok(Purged::Summary { entries: names.len(), items: count.items, bytes: count.bytes, left: count.left });
    }
    let mut done = Tally::default();
    for name in &names {
        if purge_one(&trash, name, top_dev, 0, Walk::Delete, &mut done)? {
            drop_record(&trash, &String::from_utf8_lossy(name.as_bytes()));
        }
    }
    // A record whose entry is gone (deleted for good or moved out some other way) is no use to anybody.
    let kept: Vec<String> = read_names(&trash, 1_000_000).unwrap_or_default().iter().filter_map(|n| trash_id(&String::from_utf8_lossy(n.as_bytes())).map(str::to_owned)).collect();
    if let Ok(froms) = open_at(&trash, &c_name(std::ffi::OsStr::new(".from"))?, libc::O_RDONLY | libc::O_DIRECTORY, 0).map(OwnedFd::from) {
        for record in read_names(&froms, 1_000_000).unwrap_or_default() {
            if !kept.iter().any(|id| id.as_bytes() == record.as_bytes()) {
                // SAFETY: the folder is open and the name is NUL-terminated.
                unsafe { libc::unlinkat(froms.as_raw_fd(), record.as_ptr(), 0) };
            }
        }
    }
    Ok(Purged::Done { removed: done.removed, left: done.left })
}

#[cfg(unix)]
/// Renames or moves a plain file, a folder or a link to `to_text` (the whole new path; its folder must already exist inside the
/// project, and it may be the same folder). Nothing is overwritten: an existing name is an error, and a move to another mount is
/// refused (nothing is copied). A folder takes what is in it along; the guard looks at the real old and new place of the entry, which
/// is all that changes for what is inside.
pub fn rename_in(root: &Path, rel_text: &str, to_text: &str, confirm: bool) -> Result<Changed, String> {
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    let from_rel = rel_path(rel_text).map_err(str::to_owned)?;
    let to_rel = rel_path(to_text).map_err(str::to_owned)?;
    let to_name = to_rel.file_name().ok_or_else(|| "that is the project folder".to_owned())?.to_owned();
    new_name(&to_name.to_string_lossy()).map_err(str::to_owned)?;
    if from_rel == to_rel {
        return Err("it is already there".to_owned());
    }
    let p = place(root, rel_text)?;
    let st = p.st.ok_or_else(|| "no such file or folder".to_owned())?;
    let q = place(root, to_text).map_err(|e| if e == "no such file or folder" { "the destination folder does not exist".to_owned() } else { e })?;
    move_place(&p, &st, &q, confirm)
}

#[cfg(unix)]
/// Moves the entry `p` (looked at as `st`) to the not yet existing `q`, each through its own open folder. Both real places go through
/// the guard (a name may neither leave nor enter a place the guard refuses); what cannot be done is refused before the person is
/// asked; nothing is overwritten and nothing is copied.
fn move_place(p: &Place, st: &libc::stat, q: &Place, confirm: bool) -> Result<Changed, String> {
    let from_ask = check_change(&p.rel, confirm)?;
    let to_ask = check_change(&q.rel, confirm)?;
    let kind = changeable(st)?;
    if q.st.is_some() {
        return Err("that name already exists".to_owned());
    }
    // A folder cannot go into itself or into what is inside it; both paths come from the open folders, not from the text sent.
    if kind == libc::S_IFDIR && q.rel.parent().is_some_and(|d| d.starts_with(&p.rel)) {
        return Err("a folder cannot be moved into itself or a folder inside it".to_owned());
    }
    // Asked last: a move that cannot be done is refused before the person is asked about it.
    if let Some(why) = from_ask.or(to_ask) {
        return Ok(Changed::NeedsConfirm(why));
    }
    // What was checked is what is moved.
    let again = stat_at(&p.dir, &p.name)?;
    if again.st_ino != st.st_ino || again.st_dev != st.st_dev || again.st_mode & libc::S_IFMT != kind {
        return Err("the entry was replaced meanwhile, so it was not moved".to_owned());
    }
    renameat2_noreplace(&p.dir, &p.name, &q.dir, &q.name, kind)?;
    fsync_dir(&p.dir);
    fsync_dir(&q.dir);
    Ok(Changed::Done)
}

#[cfg(target_os = "linux")]
fn renameat2_raw(from_dir: &OwnedFd, from: &CString, to_dir: &OwnedFd, to: &CString) -> std::io::Result<()> {
    // SAFETY: both folders are open and both names are NUL-terminated. RENAME_NOREPLACE (1) makes an existing target an error
    // instead of replacing it.
    let renamed = unsafe { libc::syscall(libc::SYS_renameat2, from_dir.as_raw_fd(), from.as_ptr(), to_dir.as_raw_fd(), to.as_ptr(), 1 as libc::c_uint) };
    if renamed == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Moving (like the `/proc/self/fd` check that every operation here starts with) is Linux only.
#[cfg(all(unix, not(target_os = "linux")))]
fn renameat2_raw(_from_dir: &OwnedFd, _from: &CString, _to_dir: &OwnedFd, _to: &CString) -> std::io::Result<()> {
    Err(std::io::Error::from_raw_os_error(libc::ENOSYS))
}

#[cfg(unix)]
fn renameat2_noreplace(from_dir: &OwnedFd, from: &CString, to_dir: &OwnedFd, to: &CString, kind: libc::mode_t) -> Result<(), String> {
    let Err(e) = renameat2_raw(from_dir, from, to_dir, to) else { return Ok(()) };
    Err(match e.raw_os_error() {
        Some(libc::EXDEV) => "it is on another disk or mount point, so it cannot be moved there (nothing was copied)".to_owned(),
        Some(libc::EINVAL) if kind == libc::S_IFDIR => "a folder cannot be moved into itself or a folder inside it, or this file system cannot move without the risk of overwriting".to_owned(),
        Some(libc::EINVAL | libc::ENOSYS) => "this file system cannot move without the risk of overwriting".to_owned(),
        _ => io_sentence(&e, "it could not be moved"),
    })
}

#[cfg(not(unix))]
pub fn create_in(_root: &Path, _rel_text: &str, _kind: &str, _confirm: bool) -> Result<Changed, String> {
    Err("the editor is not available on Windows".to_owned())
}

#[cfg(not(unix))]
pub fn delete_in(_root: &Path, _rel_text: &str, _base: Option<&str>, _confirm: bool) -> Result<Changed, String> {
    Err("the editor is not available on Windows".to_owned())
}

#[cfg(not(unix))]
pub fn purge_in(_root: &Path, _rel_text: &str, _confirm: bool) -> Result<Purged, String> {
    Err("the editor is not available on Windows".to_owned())
}

#[cfg(not(unix))]
pub fn empty_trash_in(_root: &Path, _rel_text: &str, _confirm: bool) -> Result<Purged, String> {
    Err("the editor is not available on Windows".to_owned())
}

#[cfg(not(unix))]
pub fn restore_in_to(_root: &Path, _rel_text: &str, _confirm: bool) -> Result<(Changed, Option<String>), String> {
    Err("the editor is not available on Windows".to_owned())
}

#[cfg(not(unix))]
pub fn restore_in(_root: &Path, _rel_text: &str, _confirm: bool) -> Result<Changed, String> {
    Err("the editor is not available on Windows".to_owned())
}

#[cfg(not(unix))]
pub fn rename_in(_root: &Path, _rel_text: &str, _to_text: &str, _confirm: bool) -> Result<Changed, String> {
    Err("the editor is not available on Windows".to_owned())
}

/// Windows: the editor does not open or save files. A link, a junction, a name that differs only in case or a trailing dot, an
/// alternate data stream or a device name cannot be told apart from the project's own files with the checks used on Linux, and a
/// weak copy of them would be worse than none.
#[cfg(not(unix))]
pub fn read_text(_project: &Path, _rel_text: &str) -> Result<TextFile, String> {
    Err("the editor is not available on Windows".to_owned())
}

#[cfg(not(unix))]
pub fn write_text(_project: &Path, _rel_text: &str, _text: &str, _base: &str, _confirm: bool) -> Result<Saved, String> {
    Err("the editor is not available on Windows".to_owned())
}

#[cfg(not(unix))]
pub fn read_text_in(_root: &Path, _rel_text: &str) -> Result<TextFile, String> {
    Err("the editor is not available on Windows".to_owned())
}

#[cfg(not(unix))]
pub fn version_in(_root: &Path, _rel_text: &str) -> Result<String, String> {
    Err("the editor is not available on Windows".to_owned())
}

#[cfg(not(unix))]
pub fn write_text_in(_root: &Path, _rel_text: &str, _text: &str, _base: &str, _confirm: bool) -> Result<Saved, String> {
    Err("the editor is not available on Windows".to_owned())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn project() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("proj");
        std::fs::create_dir_all(&p).unwrap();
        (dir, p)
    }

    fn base_of(p: &Path, rel: &str) -> String {
        read_text(p, rel).unwrap().version
    }

    #[test]
    fn paths_are_relative_and_plain() {
        assert_eq!(rel_path("").unwrap(), PathBuf::new());
        assert_eq!(rel_path("a/b.txt").unwrap(), PathBuf::from("a/b.txt"));
        for bad in ["/etc/passwd", "../x", "a/../b", "./a", "a//b", "a/", "a\0b"] {
            assert!(rel_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_guard_sorts_the_places_a_later_chat_reads() {
        let g = |p: &str| guard(Path::new(p));
        for p in [".git/config", "sub/.GIT/hooks/pre-commit", ".git"] {
            assert!(matches!(g(p), Guard::Block(_)), "{p}");
        }
        for p in [".claude/settings.json", "a/.Claude/commands/x.md", ".mcp.json", "CLAUDE.md", "sub/claude.local.md", ".envrc", ".env", ".env.local", ".vscode/tasks.json", ".husky/pre-commit", ".pre-commit-config.yaml"] {
            assert!(matches!(g(p), Guard::Confirm(_)), "{p}");
        }
        for p in ["src/main.rs", "README.md", "environment.md", "a/.github/x.yml", ".gitignore"] {
            assert_eq!(g(p), Guard::Free, "{p}");
        }
    }

    #[test]
    fn a_listing_is_sorted_marked_and_stays_inside() {
        let (dir, p) = project();
        std::fs::create_dir_all(p.join("sub")).unwrap();
        std::fs::create_dir_all(p.join(".git")).unwrap();
        std::fs::create_dir_all(p.join("node_modules")).unwrap();
        std::fs::write(p.join("b.txt"), "x").unwrap();
        std::fs::write(p.join("A.txt"), "xy").unwrap();
        std::fs::write(p.join(".env"), "K=v").unwrap();
        std::fs::write(p.join("CLAUDE.md"), "x").unwrap();
        let outside = dir.path().join("out");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("s.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside, p.join("out-link")).unwrap();
        std::os::unix::fs::symlink(outside.join("s.txt"), p.join("file-link")).unwrap();
        std::os::unix::fs::symlink(p.join("sub"), p.join("in-link")).unwrap();
        std::os::unix::fs::symlink(p.join("missing"), p.join("broken")).unwrap();
        std::os::unix::fs::symlink(&outside, p.join("sub").join("x")).unwrap();

        let l = list_dir(&p, "").unwrap();
        let names: Vec<&str> = l.entries.iter().map(|e| e.name.as_str()).collect();
        let dirs: Vec<&str> = l.entries.iter().filter(|e| e.kind == "dir").map(|e| e.name.as_str()).collect();
        assert_eq!(dirs, [".git", "in-link", "node_modules", "sub"], "folders first, then by name");
        assert!(names.iter().position(|n| *n == "A.txt") < names.iter().position(|n| *n == "b.txt"), "case ignored when sorting");
        let get = |n: &str| l.entries.iter().find(|e| e.name == n).unwrap();
        assert_eq!(get(".git").flags, ["blocked", "heavy"]);
        assert_eq!(get("node_modules").flags, ["heavy"]);
        assert_eq!(get(".env").flags, ["guarded", "secret"]);
        assert_eq!(get("CLAUDE.md").flags, ["guarded"]);
        assert_eq!(get("A.txt").size, 2);
        assert!(get("in-link").link && get("in-link").kind == "dir", "a link to a folder inside the project is a folder");
        assert!(get("out-link").link && get("out-link").kind == "other", "a link out of the project cannot be opened");
        assert_eq!(get("file-link").kind, "other");
        assert_eq!(get("broken").kind, "other");

        assert!(list_dir(&p, "sub").is_ok());
        assert!(list_dir(&p, "out-link").is_err(), "a folder reached through a link out is not listed");
        assert!(list_dir(&p, "sub/x").is_err());
        assert!(list_dir(&p, "../out").is_err());
        assert!(list_dir(&p, "b.txt").is_err());
        assert!(list_dir(&p, "nope").is_err());
    }

    #[test]
    fn a_listing_is_capped_and_odd_names_are_counted() {
        let (_d, p) = project();
        for i in 0..(MAX_ENTRIES + 5) {
            std::fs::write(p.join(format!("f{i:05}")), "").unwrap();
        }
        let name = std::ffi::OsStr::from_bytes(b"bad\xff");
        std::fs::write(p.join(name), "").unwrap();
        let l = list_dir(&p, "").unwrap();
        assert_eq!(l.entries.len(), MAX_ENTRIES);
        assert!(l.truncated);
        assert_eq!(l.unreadable, 1);
    }

    #[test]
    fn the_disk_version_matches_read_and_save_and_follows_outside_writes() {
        let (_dir, p) = project();
        std::fs::write(p.join("a.txt"), "one\n").unwrap();
        assert_eq!(version_in(&p.canonicalize().unwrap(), "a.txt").unwrap(), read_text(&p, "a.txt").unwrap().version);
        // A CRLF file saved by the editor: the version it returns is the one the check finds.
        std::fs::write(p.join("c.txt"), "x\r\ny\r\n").unwrap();
        let base = read_text(&p, "c.txt").unwrap().version;
        let Saved::Saved { version: saved } = write_text(&p, "c.txt", "a\r\nb\r\n", &base, false).unwrap() else { panic!("not saved") };
        let root = p.canonicalize().unwrap();
        assert_eq!(version_in(&root, "c.txt").unwrap(), saved);
        let before = version_in(&root, "a.txt").unwrap();
        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        assert_ne!(version_in(&root, "a.txt").unwrap(), before);
        // Missing, a folder, a way out of the project, an empty name and a file that is too big are errors.
        std::fs::create_dir_all(p.join("d")).unwrap();
        std::fs::write(p.join("big.txt"), vec![b'a'; MAX_TEXT + 1]).unwrap();
        for bad in ["nope.txt", "d", "../x", "", "big.txt"] {
            assert!(version_in(&root, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn text_is_read_with_its_version_and_line_ends() {
        let (dir, p) = project();
        std::fs::create_dir_all(p.join("sub")).unwrap();
        std::fs::write(p.join("sub/a.txt"), "hi\n한글\n").unwrap();
        let t = read_text(&p, "sub/a.txt").unwrap();
        assert_eq!(t.text, "hi\n한글\n");
        assert_eq!(t.eol, "lf");
        assert_eq!(t.guard, None);
        assert_eq!(t.version, version("hi\n한글\n".as_bytes()));
        std::fs::write(p.join("crlf.txt"), "a\r\nb\r\n").unwrap();
        assert_eq!(read_text(&p, "crlf.txt").unwrap().eol, "crlf");
        std::fs::write(p.join("mix.txt"), "a\r\nb\n").unwrap();
        assert_eq!(read_text(&p, "mix.txt").unwrap().eol, "mixed");
        std::fs::write(p.join("cr.txt"), "a\rb").unwrap();
        assert_eq!(read_text(&p, "cr.txt").unwrap().eol, "mixed");
        std::fs::write(p.join("CLAUDE.md"), "x").unwrap();
        assert_eq!(read_text(&p, "CLAUDE.md").unwrap().guard_kind, "confirm");
        std::fs::create_dir_all(p.join(".git")).unwrap();
        std::fs::write(p.join(".git/config"), "x").unwrap();
        assert_eq!(read_text(&p, ".git/config").unwrap().guard_kind, "block", "a blocked file can still be read");

        std::fs::write(p.join("bin"), b"a\0b").unwrap();
        std::fs::write(p.join("latin"), b"caf\xe9").unwrap();
        std::fs::write(p.join("big"), vec![b'a'; MAX_TEXT + 1]).unwrap();
        std::fs::write(p.join("edge"), vec![b'a'; MAX_TEXT]).unwrap();
        for bad in ["bin", "latin", "big", "sub", "", "missing", "../proj/a", "/etc/passwd"] {
            assert!(read_text(&p, bad).is_err(), "{bad}");
        }
        assert!(read_text(&p, "edge").is_ok());

        let outside = dir.path().join("o.txt");
        std::fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, p.join("l.txt")).unwrap();
        assert!(read_text(&p, "l.txt").is_err(), "a link out of the project is not followed");
        std::os::unix::fs::symlink(p.join("sub/a.txt"), p.join("in.txt")).unwrap();
        assert_eq!(read_text(&p, "in.txt").unwrap().text, "hi\n한글\n", "a link that stays inside is followed");
    }

    #[test]
    fn the_guard_and_the_answer_name_the_real_place_of_a_link() {
        let (_d, p) = project();
        std::fs::create_dir_all(p.join(".claude")).unwrap();
        std::fs::write(p.join(".claude/settings.json"), "{}").unwrap();
        std::fs::create_dir_all(p.join("x")).unwrap();
        std::os::unix::fs::symlink(p.join(".claude"), p.join("x/y")).unwrap();
        let t = read_text(&p, "x/y/settings.json").unwrap();
        assert_eq!(t.real, ".claude/settings.json");
        assert_eq!(t.guard_kind, "confirm", "a folder reached through a link is judged by where it really is");
        assert!(matches!(write_text(&p, "x/y/settings.json", "{}", &t.version, false).unwrap(), Saved::NeedsConfirm(_)));
        std::fs::write(p.join("plain.txt"), "p").unwrap();
        assert_eq!(read_text(&p, "plain.txt").unwrap().real, "plain.txt");
    }

    #[test]
    fn a_save_keeps_the_group_of_the_file_when_it_can() {
        use std::os::unix::fs::MetadataExt;
        let (_d, p) = project();
        std::fs::write(p.join("a.txt"), "a").unwrap();
        let gid = std::fs::metadata(p.join("a.txt")).unwrap().gid();
        write_text(&p, "a.txt", "b", &version(b"a"), false).unwrap();
        assert_eq!(std::fs::metadata(p.join("a.txt")).unwrap().gid(), gid);
    }

    #[test]
    fn a_fifo_is_refused_without_waiting() {
        let (_d, p) = project();
        let path = CString::new(p.join("pipe").as_os_str().as_bytes()).unwrap();
        // SAFETY: a valid path; the result is checked.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(read_text(&p, "pipe").is_err());
    }

    #[test]
    fn a_save_replaces_the_text_keeps_the_mode_and_leaves_nothing_behind() {
        let (_d, p) = project();
        std::fs::write(p.join("a.txt"), "old").unwrap();
        std::fs::set_permissions(p.join("a.txt"), std::fs::Permissions::from_mode(0o640)).unwrap();
        let base = base_of(&p, "a.txt");
        let saved = write_text(&p, "a.txt", "new text\n", &base, false).unwrap();
        assert_eq!(saved, Saved::Saved { version: version(b"new text\n") });
        assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "new text\n");
        assert_eq!(std::fs::metadata(p.join("a.txt")).unwrap().permissions().mode() & 0o7777, 0o640);
        let left: Vec<_> = std::fs::read_dir(&p).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "no temporary file stays: {left:?}");
        // The saved version is the base of the next save.
        let again = write_text(&p, "a.txt", "third", &version(b"new text\n"), false).unwrap();
        assert!(matches!(again, Saved::Saved { .. }));

        std::fs::write(p.join("x.sh"), "x").unwrap();
        std::fs::set_permissions(p.join("x.sh"), std::fs::Permissions::from_mode(0o4755)).unwrap();
        write_text(&p, "x.sh", "y", &base_of(&p, "x.sh"), false).unwrap();
        assert_eq!(std::fs::metadata(p.join("x.sh")).unwrap().permissions().mode() & 0o7777, 0o755, "setuid is not carried over");
    }

    #[test]
    fn a_save_over_a_changed_file_is_a_conflict_and_changes_nothing() {
        let (_d, p) = project();
        std::fs::write(p.join("a.txt"), "one").unwrap();
        let base = base_of(&p, "a.txt");
        std::fs::write(p.join("a.txt"), "two, written by someone else").unwrap();
        let out = write_text(&p, "a.txt", "mine", &base, false).unwrap();
        assert_eq!(out, Saved::Conflict { text: "two, written by someone else".into(), version: version(b"two, written by someone else") });
        assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "two, written by someone else");
        // Taking the new version as the base is the "save anyway".
        let now = version(b"two, written by someone else");
        assert!(matches!(write_text(&p, "a.txt", "mine", &now, false).unwrap(), Saved::Saved { .. }));
    }

    #[test]
    fn a_save_is_refused_or_confirmed_where_a_later_chat_would_read_it() {
        let (_d, p) = project();
        std::fs::create_dir_all(p.join(".git/hooks")).unwrap();
        std::fs::create_dir_all(p.join(".claude")).unwrap();
        std::fs::write(p.join(".git/config"), "c").unwrap();
        std::fs::write(p.join(".git/hooks/pre-commit"), "c").unwrap();
        std::fs::write(p.join(".claude/settings.json"), "{}").unwrap();
        std::fs::write(p.join("CLAUDE.md"), "c").unwrap();
        for blocked in [".git/config", ".git/hooks/pre-commit"] {
            let b = base_of(&p, blocked);
            assert!(write_text(&p, blocked, "x", &b, true).is_err(), "{blocked} is refused even with a confirmation");
            assert_eq!(std::fs::read_to_string(p.join(blocked)).unwrap(), "c");
        }
        for guarded in [".claude/settings.json", "CLAUDE.md"] {
            let b = base_of(&p, guarded);
            assert!(matches!(write_text(&p, guarded, "x", &b, false).unwrap(), Saved::NeedsConfirm(_)), "{guarded}");
            assert_ne!(std::fs::read_to_string(p.join(guarded)).unwrap(), "x");
            assert!(matches!(write_text(&p, guarded, "x", &b, true).unwrap(), Saved::Saved { .. }), "{guarded}");
            assert_eq!(std::fs::read_to_string(p.join(guarded)).unwrap(), "x");
        }
        // The guard looks at the real place: a link with an innocent name to a guarded file is guarded too.
        std::fs::write(p.join(".mcp.json"), "{}").unwrap();
        std::os::unix::fs::symlink(p.join(".mcp.json"), p.join("harmless.txt")).unwrap();
        let b = base_of(&p, "harmless.txt");
        assert!(matches!(write_text(&p, "harmless.txt", "x", &b, false).unwrap(), Saved::NeedsConfirm(_)));
    }

    #[test]
    fn a_save_never_leaves_the_project_or_touches_what_it_should_not() {
        let (dir, p) = project();
        let outside = dir.path().join("out");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("s.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(outside.join("s.txt"), p.join("l.txt")).unwrap();
        std::os::unix::fs::symlink(&outside, p.join("d")).unwrap();
        assert!(write_text(&p, "l.txt", "x", &version(b"outside"), true).is_err());
        assert!(write_text(&p, "d/s.txt", "x", &version(b"outside"), true).is_err());
        assert!(write_text(&p, "../out/s.txt", "x", &version(b"outside"), true).is_err());
        assert_eq!(std::fs::read_to_string(outside.join("s.txt")).unwrap(), "outside");

        // A hard link to a file outside: reading is allowed, a save replaces the project's name only.
        std::fs::hard_link(outside.join("s.txt"), p.join("h.txt")).unwrap();
        write_text(&p, "h.txt", "changed", &version(b"outside"), false).unwrap();
        assert_eq!(std::fs::read_to_string(p.join("h.txt")).unwrap(), "changed");
        assert_eq!(std::fs::read_to_string(outside.join("s.txt")).unwrap(), "outside", "the outer file is not changed");

        // Read-only files, folders, new files and big text.
        std::fs::write(p.join("ro.txt"), "ro").unwrap();
        std::fs::set_permissions(p.join("ro.txt"), std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(write_text(&p, "ro.txt", "x", &version(b"ro"), false).is_err());
        assert_eq!(std::fs::read_to_string(p.join("ro.txt")).unwrap(), "ro");
        std::fs::create_dir_all(p.join("sub")).unwrap();
        assert!(write_text(&p, "sub", "x", "", false).is_err());
        assert!(write_text(&p, "new.txt", "x", "", false).is_err(), "no new files");
        std::fs::write(p.join("a.txt"), "a").unwrap();
        assert!(write_text(&p, "a.txt", &"a".repeat(MAX_TEXT + 1), &version(b"a"), false).is_err());
        assert!(write_text(&p, "a.txt", "a\0b", &version(b"a"), false).is_err());
        assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "a");
    }

    #[test]
    fn a_project_folder_swapped_for_a_link_after_the_check_is_not_taken_for_the_project() {
        let (dir, p) = project();
        std::fs::write(p.join("s.txt"), "mine").unwrap();
        let outside = dir.path().join("out");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("s.txt"), "secret").unwrap();
        // The folder as it was when the project was checked.
        let root = p.canonicalize().unwrap();
        let base = base_of(&p, "s.txt");
        // Then the project's name is made to point somewhere else.
        std::fs::rename(&p, dir.path().join("proj-moved")).unwrap();
        std::os::unix::fs::symlink(&outside, &p).unwrap();
        // Through the checked path nothing outside is listed, read or changed: the name is not resolved again.
        assert!(read_text_in(&root, "s.txt").is_err());
        assert!(write_text_in(&root, "s.txt", "changed", &base, true).is_err());
        assert!(list_dir_in(&root, "").is_err() || list_dir_in(&root, "").unwrap().entries.iter().all(|e| e.name != "s.txt"));
        assert_eq!(std::fs::read_to_string(outside.join("s.txt")).unwrap(), "secret");
    }
    // ---- create, delete, rename ----

    fn root_of(p: &Path) -> PathBuf {
        p.canonicalize().unwrap()
    }

    #[test]
    fn create_makes_a_file_or_a_folder_that_did_not_exist() {
        let (_d, p) = project();
        let root = root_of(&p);
        std::fs::create_dir_all(p.join("sub")).unwrap();
        assert_eq!(create_in(&root, "sub/new.txt", "file", false), Ok(Changed::Done));
        assert_eq!(std::fs::read(p.join("sub/new.txt")).unwrap(), b"");
        assert_eq!(create_in(&root, "sub/dir", "dir", false), Ok(Changed::Done));
        assert!(p.join("sub/dir").is_dir());
        std::fs::write(p.join("old.txt"), "keep").unwrap();
        assert!(create_in(&root, "old.txt", "file", false).is_err());
        assert!(create_in(&root, "sub", "dir", false).is_err());
        assert_eq!(std::fs::read_to_string(p.join("old.txt")).unwrap(), "keep");
        // No folder in between is made, and a kind that does not exist is refused.
        assert!(create_in(&root, "nope/x.txt", "file", false).is_err());
        assert!(!p.join("nope").exists());
        assert!(create_in(&root, "x", "socket", false).is_err());
        assert!(create_in(&root, "", "file", false).is_err());
    }

    #[test]
    fn create_never_goes_through_a_link() {
        let (d, p) = project();
        let root = root_of(&p);
        let outside = d.path().join("out");
        std::fs::create_dir_all(&outside).unwrap();
        // A dangling link in the place of the new name: nothing is made behind it.
        std::os::unix::fs::symlink(outside.join("made.txt"), p.join("dangling")).unwrap();
        assert!(create_in(&root, "dangling", "file", false).is_err());
        assert!(!outside.join("made.txt").exists());
        // A folder link that leaves the project is not a way to make something outside.
        std::os::unix::fs::symlink(&outside, p.join("out-link")).unwrap();
        assert!(create_in(&root, "out-link/x.txt", "file", false).is_err());
        assert!(!outside.join("x.txt").exists());
        // A link to a folder inside the project is followed in the folder part only, and the guard looks at the real place.
        std::fs::create_dir_all(p.join(".git")).unwrap();
        std::os::unix::fs::symlink(p.join(".git"), p.join("alias")).unwrap();
        assert!(create_in(&root, "alias/hook", "file", false).is_err());
        assert!(!p.join(".git/hook").exists());
    }

    #[test]
    fn create_delete_and_rename_follow_the_guard() {
        let (_d, p) = project();
        let root = root_of(&p);
        std::fs::create_dir_all(p.join(".git")).unwrap();
        std::fs::create_dir_all(p.join(".claude")).unwrap();
        std::fs::write(p.join(".git/config"), "[core]").unwrap();
        std::fs::write(p.join(".claude/settings.json"), "{}").unwrap();
        for bad in [".GIT/x", ".git/x"] {
            assert!(create_in(&root, bad, "file", true).is_err(), "{bad}");
        }
        assert!(delete_in(&root, ".git/config", None, true).is_err());
        assert!(delete_in(&root, ".git", None, true).is_err());
        assert!(p.join(".git/config").exists());
        // A place a later chat reads asks first; the confirmation does it.
        assert!(matches!(create_in(&root, ".claude/new.json", "file", false), Ok(Changed::NeedsConfirm(_))));
        assert!(!p.join(".claude/new.json").exists());
        assert_eq!(create_in(&root, ".claude/new.json", "file", true), Ok(Changed::Done));
        assert!(matches!(delete_in(&root, ".claude/settings.json", None, false), Ok(Changed::NeedsConfirm(_))));
        assert!(p.join(".claude/settings.json").exists());
        assert!(matches!(delete_in(&root, ".claude/settings.json", None, true), Ok(Changed::Moved(_))));
        // A name may neither enter nor leave a refused place.
        std::fs::write(p.join("hooks"), "x").unwrap();
        assert!(rename_in(&root, "hooks", ".git", true).is_err());
        assert!(rename_in(&root, ".git/config", ".git/other", true).is_err());
        assert!(matches!(rename_in(&root, "hooks", ".claude", false), Err(_) | Ok(Changed::NeedsConfirm(_))));
        std::fs::write(p.join("plain"), "x").unwrap();
        assert!(matches!(rename_in(&root, "plain", "CLAUDE.md", false), Ok(Changed::NeedsConfirm(_))));
        assert!(p.join("plain").exists());
    }

    #[test]
    fn names_that_are_not_names_are_refused() {
        let (_d, p) = project();
        let root = root_of(&p);
        let long = "a".repeat(256);
        for bad in ["", ".", "..", "a/../b", "x\n", "x\u{202e}", " x", "x ", ".tenrec-1.tmp", long.as_str()] {
            assert!(create_in(&root, bad, "file", false).is_err(), "{bad:?}");
        }
        std::fs::write(p.join("a"), "1").unwrap();
        for bad in ["b\n", "b ", ".tenrec-2.tmp", "sub/b"] {
            assert!(rename_in(&root, "a", bad, false).is_err(), "{bad:?}");
        }
        assert!(p.join("a").exists());
    }

    fn trashed(r: Result<Changed, String>) -> String {
        match r {
            Ok(Changed::Moved(to)) => to,
            other => panic!("not trashed: {other:?}"),
        }
    }

    #[test]
    fn delete_moves_a_file_a_folder_or_a_link_to_the_trash_and_nothing_else() {
        use std::os::unix::fs::MetadataExt;
        let (d, p) = project();
        let root = root_of(&p);
        let outside = d.path().join("out");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), "keep").unwrap();
        std::fs::write(p.join("a.txt"), "one").unwrap();
        // A file that changed since it was opened is not deleted.
        assert!(delete_in(&root, "a.txt", Some(&version(b"old")), false).is_err());
        assert!(p.join("a.txt").exists());
        let ino = std::fs::metadata(p.join("a.txt")).unwrap().ino();
        let to = trashed(delete_in(&root, "a.txt", Some(&version(b"one")), false));
        assert!(!p.join("a.txt").exists());
        assert!(to.starts_with(".tenrec-trash/") && to.ends_with("-a.txt"), "{to}");
        assert_eq!(std::fs::metadata(p.join(&to)).unwrap().ino(), ino, "the same file, moved");
        assert!(delete_in(&root, "a.txt", None, false).is_err());
        // The trash: 0700, ignored by git as a whole, and the place the entry came from is recorded.
        assert_eq!(std::fs::metadata(p.join(".tenrec-trash")).unwrap().mode() & 0o777, 0o700);
        assert_eq!(std::fs::read_to_string(p.join(".tenrec-trash/.gitignore")).unwrap(), "*\n");
        let id = trash_id(to.rsplit('/').next().unwrap()).unwrap().to_owned();
        assert_eq!(std::fs::read_to_string(p.join(".tenrec-trash/.from").join(&id)).unwrap(), "a.txt");
        // A folder with what it holds goes whole.
        std::fs::create_dir_all(p.join("full/sub")).unwrap();
        std::fs::write(p.join("full/sub/x"), "x").unwrap();
        let full = trashed(delete_in(&root, "full", None, false));
        assert!(!p.join("full").exists() && p.join(&full).join("sub/x").exists());
        // A link is moved by itself, what it points to stays.
        std::os::unix::fs::symlink(outside.join("keep.txt"), p.join("file-link")).unwrap();
        std::os::unix::fs::symlink(&outside, p.join("dir-link")).unwrap();
        trashed(delete_in(&root, "file-link", None, false));
        trashed(delete_in(&root, "dir-link", None, false));
        assert!(std::fs::symlink_metadata(p.join("file-link")).is_err() && std::fs::symlink_metadata(p.join("dir-link")).is_err());
        assert_eq!(std::fs::read_to_string(outside.join("keep.txt")).unwrap(), "keep");
        // A read-only file can go to the trash (a move needs the folder, not the file); a path out of the project and the project
        // folder itself are refused.
        std::fs::write(p.join("ro.txt"), "r").unwrap();
        std::fs::set_permissions(p.join("ro.txt"), std::fs::Permissions::from_mode(0o444)).unwrap();
        trashed(delete_in(&root, "ro.txt", None, false));
        assert!(delete_in(&root, "../out/keep.txt", None, false).is_err());
        assert!(delete_in(&root, "", None, false).is_err());
        assert!(outside.join("keep.txt").exists());
    }

    #[test]
    fn a_delete_inside_the_trash_is_for_good_and_only_for_what_is_empty() {
        let (_d, p) = project();
        let root = root_of(&p);
        std::fs::create_dir_all(p.join("full")).unwrap();
        std::fs::write(p.join("full/x"), "x").unwrap();
        std::fs::write(p.join("f.txt"), "f").unwrap();
        std::fs::create_dir_all(p.join("empty")).unwrap();
        let full = trashed(delete_in(&root, "full", None, false));
        let file = trashed(delete_in(&root, "f.txt", None, false));
        let empty = trashed(delete_in(&root, "empty", None, false));
        assert!(delete_in(&root, &full, None, false).is_err(), "a folder with something in it is not deleted for good yet");
        assert!(p.join(&full).join("x").exists());
        assert_eq!(delete_in(&root, &file, None, false), Ok(Changed::Done));
        assert_eq!(delete_in(&root, &empty, None, false), Ok(Changed::Done));
        assert!(!p.join(&file).exists() && !p.join(&empty).exists());
    }

    #[test]
    fn restore_brings_an_entry_back_to_where_it_came_from_or_says_why_not() {
        use std::os::unix::fs::MetadataExt;
        let (_d, p) = project();
        let root = root_of(&p);
        std::fs::create_dir_all(p.join("sub")).unwrap();
        std::fs::write(p.join("sub/a.txt"), "a").unwrap();
        let ino = std::fs::metadata(p.join("sub/a.txt")).unwrap().ino();
        let to = trashed(delete_in(&root, "sub/a.txt", None, false));
        assert_eq!(restore_in(&root, &to, false), Ok(Changed::Moved("sub/a.txt".to_owned())));
        assert_eq!(std::fs::metadata(p.join("sub/a.txt")).unwrap().ino(), ino);
        assert!(!p.join(&to).exists());
        // The record is gone with it, so the same entry cannot be restored twice.
        assert!(restore_in(&root, &to, false).is_err());
        // The name is taken again, or the folder is gone: nothing moves.
        let again = trashed(delete_in(&root, "sub/a.txt", None, false));
        std::fs::write(p.join("sub/a.txt"), "new").unwrap();
        assert!(restore_in(&root, &again, false).is_err());
        assert!(p.join(&again).exists() && std::fs::read_to_string(p.join("sub/a.txt")).unwrap() == "new");
        std::fs::remove_file(p.join("sub/a.txt")).unwrap();
        std::fs::remove_dir(p.join("sub")).unwrap();
        assert!(restore_in(&root, &again, false).is_err());
        assert!(p.join(&again).exists());
        // Only an entry directly in the trash, with a record, can be restored.
        std::fs::write(p.join("plain.txt"), "p").unwrap();
        assert!(restore_in(&root, "plain.txt", false).is_err());
        assert!(restore_in(&root, ".tenrec-trash/.gitignore", false).is_err());
        assert!(restore_in(&root, "", false).is_err());
    }

    #[test]
    fn the_trash_follows_the_guard_both_ways() {
        let (_d, p) = project();
        let root = root_of(&p);
        std::fs::create_dir_all(p.join(".claude")).unwrap();
        std::fs::create_dir_all(p.join(".git")).unwrap();
        std::fs::write(p.join(".claude/s.json"), "{}").unwrap();
        std::fs::write(p.join(".git/config"), "c").unwrap();
        // What a later chat reads asks before it goes; .git never does.
        assert!(matches!(delete_in(&root, ".claude/s.json", None, false), Ok(Changed::NeedsConfirm(_))));
        assert!(p.join(".claude/s.json").exists());
        assert!(delete_in(&root, ".git/config", None, true).is_err());
        let to = trashed(delete_in(&root, ".claude/s.json", None, true));
        // Back into it: asks again.
        assert!(matches!(restore_in(&root, &to, false), Ok(Changed::NeedsConfirm(_))));
        assert!(p.join(&to).exists());
        assert_eq!(restore_in(&root, &to, true), Ok(Changed::Moved(".claude/s.json".to_owned())));
        // A trashed name that would mean something at the top (.env, .claude, CLAUDE.md) is free in the trash.
        for name in [".env", ".claude", "CLAUDE.md"] {
            assert_eq!(guard(Path::new(&format!(".tenrec-trash/1-abcdef01-{name}"))), Guard::Free, "{name}");
        }
        std::fs::write(p.join(".env"), "SECRET=1").unwrap();
        assert!(matches!(delete_in(&root, ".env", None, false), Ok(Changed::NeedsConfirm(_))));
        trashed(delete_in(&root, ".env", None, true));
    }

    #[test]
    fn a_trash_that_is_not_a_folder_of_ours_stops_the_delete_before_anything_moves() {
        let (d, p) = project();
        let root = root_of(&p);
        let outside = d.path().join("out");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(p.join("a.txt"), "a").unwrap();
        // A file in the place of the trash, and a link that leaves the project.
        std::fs::write(p.join(".tenrec-trash"), "not a folder").unwrap();
        assert!(delete_in(&root, "a.txt", None, false).is_err());
        std::fs::remove_file(p.join(".tenrec-trash")).unwrap();
        std::os::unix::fs::symlink(&outside, p.join(".tenrec-trash")).unwrap();
        assert!(delete_in(&root, "a.txt", None, false).is_err());
        assert!(p.join("a.txt").exists());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "nothing was made outside the project");
    }

    #[test]
    fn git_does_not_see_the_trash_and_the_listing_names_where_an_entry_came_from() {
        let (_d, p) = project();
        let root = root_of(&p);
        let git = |args: &[&str]| std::process::Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@example.com"]).args(args).current_dir(&p).output();
        if !git(&["init", "-q"]).is_ok_and(|o| o.status.success()) {
            return; // no git here: the rest of this test is about git
        }
        std::fs::write(p.join("a.txt"), "a").unwrap();
        let to = trashed(delete_in(&root, "a.txt", None, false));
        let status = git(&["status", "--porcelain", "--untracked-files=all"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&status.stdout).trim(), "", "git status is clean");
        let top = list_dir_in(&root, "").unwrap();
        assert!(top.entries.iter().any(|e| e.name == ".tenrec-trash" && e.flags.contains(&"trash")));
        let inside = list_dir_in(&root, ".tenrec-trash").unwrap();
        let entry = inside.entries.iter().find(|e| to.ends_with(&e.name)).unwrap();
        assert_eq!(entry.from.as_deref(), Some("a.txt"));
        assert!(inside.entries.iter().filter(|e| e.name != ".from" && e.name != ".gitignore").all(|e| e.from.is_some()));
    }

    #[test]
    fn delete_does_not_stop_on_a_fifo_and_leaves_it() {
        let (_d, p) = project();
        let root = root_of(&p);
        let fifo = CString::new(p.join("pipe").as_os_str().as_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(delete_in(&root, "pipe", None, false).is_err());
        assert!(p.join("pipe").exists());
    }

    #[test]
    fn rename_stays_in_its_folder_and_never_overwrites() {
        let (d, p) = project();
        let root = root_of(&p);
        std::fs::create_dir_all(p.join("sub")).unwrap();
        std::fs::write(p.join("sub/a.txt"), "a").unwrap();
        std::fs::write(p.join("sub/b.txt"), "b").unwrap();
        assert_eq!(rename_in(&root, "sub/a.txt", "sub/c.txt", false), Ok(Changed::Done));
        assert!(!p.join("sub/a.txt").exists());
        assert_eq!(std::fs::read_to_string(p.join("sub/c.txt")).unwrap(), "a");
        // An existing name, file or folder, is an error and both stay.
        assert!(rename_in(&root, "sub/c.txt", "sub/b.txt", false).is_err());
        std::fs::create_dir_all(p.join("sub/d")).unwrap();
        assert!(rename_in(&root, "sub/c.txt", "sub/d", false).is_err());
        assert_eq!(std::fs::read_to_string(p.join("sub/b.txt")).unwrap(), "b");
        assert_eq!(std::fs::read_to_string(p.join("sub/c.txt")).unwrap(), "a");
        // A path that does not exist and the project folder are errors.
        assert!(rename_in(&root, "sub/none", "sub/x", false).is_err());
        assert!(rename_in(&root, "", "x", false).is_err());
        // A folder can be renamed; a link is renamed by itself; only the case of a name can change.
        assert_eq!(rename_in(&root, "sub/d", "sub/e", false), Ok(Changed::Done));
        std::os::unix::fs::symlink(d.path(), p.join("l")).unwrap();
        assert_eq!(rename_in(&root, "l", "m", false), Ok(Changed::Done));
        assert!(std::fs::symlink_metadata(p.join("m")).unwrap().file_type().is_symlink() && d.path().exists());
        assert_eq!(rename_in(&root, "sub/b.txt", "sub/B.txt", false), Ok(Changed::Done));
        assert!(p.join("sub/B.txt").exists());
    }

    #[test]
    fn a_save_cannot_bring_back_a_name_that_was_renamed() {
        let (_d, p) = project();
        let root = root_of(&p);
        std::fs::write(p.join("a.txt"), "one").unwrap();
        let base = base_of(&p, "a.txt");
        rename_in(&root, "a.txt", "b.txt", false).unwrap();
        assert!(write_text_in(&root, "a.txt", "two", &base, false).is_err());
        assert!(!p.join("a.txt").exists());
    }
    #[test]
    fn an_entry_can_be_moved_to_another_folder_of_the_project() {
        let (_d, p) = project();
        let root = root_of(&p);
        std::fs::create_dir_all(p.join("sub/deep")).unwrap();
        std::fs::write(p.join("sub/a.txt"), "a").unwrap();
        // Up, down and down with a new name.
        assert_eq!(rename_in(&root, "sub/a.txt", "a.txt", false), Ok(Changed::Done));
        assert_eq!(rename_in(&root, "a.txt", "sub/deep/a.txt", false), Ok(Changed::Done));
        assert_eq!(rename_in(&root, "sub/deep/a.txt", "sub/b.txt", false), Ok(Changed::Done));
        assert_eq!(std::fs::read_to_string(p.join("sub/b.txt")).unwrap(), "a");
        assert_eq!(read_text(&p, "sub/b.txt").unwrap().text, "a");
        // A folder takes what is in it; the open file's new path reads.
        std::fs::create_dir_all(p.join("d")).unwrap();
        std::fs::write(p.join("d/f.txt"), "f").unwrap();
        assert_eq!(rename_in(&root, "d", "sub/d", false), Ok(Changed::Done));
        assert!(!p.join("d").exists());
        assert_eq!(read_text(&p, "sub/d/f.txt").unwrap().text, "f");
        // Onto an existing name, from and to the same place, into a folder that is not there, and the project folder.
        std::fs::write(p.join("x.txt"), "x").unwrap();
        std::fs::write(p.join("sub/x.txt"), "other").unwrap();
        assert!(rename_in(&root, "x.txt", "sub/x.txt", false).is_err());
        assert_eq!(std::fs::read_to_string(p.join("sub/x.txt")).unwrap(), "other");
        assert!(p.join("x.txt").exists());
        assert!(rename_in(&root, "x.txt", "x.txt", false).is_err());
        assert!(rename_in(&root, "x.txt", "nowhere/x.txt", false).is_err());
        assert!(rename_in(&root, "x.txt", "x.txt/y", false).is_err(), "a file is not a folder to move into");
        assert!(rename_in(&root, "", "y", false).is_err());
    }

    #[test]
    fn a_folder_is_not_moved_into_itself_or_what_is_inside_it() {
        let (_d, p) = project();
        let root = root_of(&p);
        std::fs::create_dir_all(p.join("d/sub")).unwrap();
        std::fs::write(p.join("d/sub/f"), "f").unwrap();
        for to in ["d/x", "d/sub/d", "d/sub/x"] {
            assert!(rename_in(&root, "d", to, false).is_err(), "{to}");
        }
        // Through a link that points inside the folder.
        std::os::unix::fs::symlink(p.join("d/sub"), p.join("l")).unwrap();
        assert!(rename_in(&root, "d", "l/d", false).is_err());
        assert_eq!(std::fs::read_to_string(p.join("d/sub/f")).unwrap(), "f");
        assert!(!p.join("d/sub/d").exists());
    }

    #[test]
    fn a_move_never_leaves_the_project_or_enters_a_refused_place() {
        let (d, p) = project();
        let root = root_of(&p);
        let outside = d.path().join("out");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(p.join("x"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, p.join("out-link")).unwrap();
        assert!(rename_in(&root, "x", "out-link/x", false).is_err());
        assert!(!outside.join("x").exists() && p.join("x").exists());
        // Into .git, also through a link that points there, and out of it.
        std::fs::create_dir_all(p.join(".git")).unwrap();
        std::fs::write(p.join(".git/config"), "c").unwrap();
        std::os::unix::fs::symlink(p.join(".git"), p.join("alias")).unwrap();
        for (from, to) in [("x", ".git/x"), ("x", "alias/x"), (".git/config", "config")] {
            assert!(rename_in(&root, from, to, true).is_err(), "{from} -> {to}");
        }
        assert!(p.join("x").exists() && p.join(".git/config").exists());
        // A place a later chat reads asks first, in either direction; the confirmation does it.
        std::fs::create_dir_all(p.join(".claude")).unwrap();
        assert!(matches!(rename_in(&root, "x", ".claude/x", false), Ok(Changed::NeedsConfirm(_))));
        assert!(p.join("x").exists());
        assert_eq!(rename_in(&root, "x", ".claude/x", true), Ok(Changed::Done));
        assert!(matches!(rename_in(&root, ".claude/x", "x", false), Ok(Changed::NeedsConfirm(_))));
        std::fs::create_dir_all(p.join("sub")).unwrap();
        std::fs::write(p.join("plain"), "p").unwrap();
        assert!(matches!(rename_in(&root, "plain", "sub/CLAUDE.md", false), Ok(Changed::NeedsConfirm(_))));
        // A folder that holds guarded entries moves without a question: what is inside it keeps its own name and folder.
        std::fs::create_dir_all(p.join("w/.claude")).unwrap();
        assert_eq!(rename_in(&root, "w", "sub/w", false), Ok(Changed::Done));
        assert!(p.join("sub/w/.claude").is_dir());
        // A link moves as a link.
        std::os::unix::fs::symlink(&outside, p.join("l")).unwrap();
        assert_eq!(rename_in(&root, "l", "sub/l", false), Ok(Changed::Done));
        assert!(std::fs::symlink_metadata(p.join("sub/l")).unwrap().file_type().is_symlink() && outside.exists());
    }
    #[test]
    fn a_move_sees_the_real_place_of_a_link_in_the_destination_and_a_broken_link_as_a_taken_name() {
        let (_d, p) = project();
        let root = root_of(&p);
        std::fs::create_dir_all(p.join(".claude")).unwrap();
        std::fs::write(p.join("x"), "x").unwrap();
        // A folder link that points into a place a later chat reads: the question is asked about the real place.
        std::os::unix::fs::symlink(p.join(".claude"), p.join("lnk")).unwrap();
        assert!(matches!(rename_in(&root, "x", "lnk/x", false), Ok(Changed::NeedsConfirm(_))));
        assert!(p.join("x").exists() && !p.join(".claude/x").exists());
        // A broken link in the place of the new name is a name that exists.
        std::os::unix::fs::symlink(p.join("missing"), p.join("broken")).unwrap();
        assert!(rename_in(&root, "x", "broken", false).is_err());
        assert!(p.join("x").exists());
        // A refusal comes before a question: a folder into itself is refused even where a confirmation would be asked.
        std::fs::create_dir_all(p.join("d/.claude")).unwrap();
        assert!(rename_in(&root, "d", "d/.claude/d", false).is_err());
    }
    #[test]
    fn a_delete_for_good_needs_the_trash_in_the_path_and_takes_its_record_along() {
        let (_d, p) = project();
        let root = root_of(&p);
        std::fs::write(p.join("a.txt"), "a").unwrap();
        let to = trashed(delete_in(&root, "a.txt", None, false));
        let name = to.rsplit('/').next().unwrap().to_owned();
        let id = trash_id(&name).unwrap().to_owned();
        // Through a link that points at the trash the words of the path say "not the trash": refused, nothing deleted.
        std::os::unix::fs::symlink(p.join(".tenrec-trash"), p.join("alias")).unwrap();
        assert!(delete_in(&root, &format!("alias/{name}"), None, false).is_err());
        assert!(p.join(&to).exists());
        // The trash itself is not deleted here.
        assert!(delete_in(&root, ".tenrec-trash", None, false).is_err());
        assert!(p.join(".tenrec-trash").is_dir());
        // For good from the trash itself: the record goes too.
        assert!(p.join(".tenrec-trash/.from").join(&id).exists());
        assert_eq!(delete_in(&root, &to, None, false), Ok(Changed::Done));
        assert!(!p.join(&to).exists() && !p.join(".tenrec-trash/.from").join(&id).exists());
    }
    #[test]
    fn a_purge_asks_first_then_removes_a_whole_tree_without_following_a_link() {
        let (d, p) = project();
        let root = root_of(&p);
        let outside = d.path().join("out");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), "keep").unwrap();
        std::fs::create_dir_all(p.join("tree/sub/deeper")).unwrap();
        std::fs::write(p.join("tree/a.txt"), "12345").unwrap();
        std::fs::write(p.join("tree/sub/ro.txt"), "r").unwrap();
        std::fs::set_permissions(p.join("tree/sub/ro.txt"), std::fs::Permissions::from_mode(0o444)).unwrap();
        std::os::unix::fs::symlink(&outside, p.join("tree/to-out")).unwrap();
        std::os::unix::fs::symlink(p.join("missing"), p.join("tree/broken")).unwrap();
        let to = trashed(delete_in(&root, "tree", None, false));
        let name = to.rsplit('/').next().unwrap().to_owned();
        let id = trash_id(&name).unwrap().to_owned();
        // Without the confirmation: what it would reach, and nothing is deleted.
        let Ok(Purged::Summary { entries, items, bytes, .. }) = purge_in(&root, &to, false) else { panic!("no summary") };
        assert_eq!((entries, items, bytes), (1, 7, 5 + 1));
        assert!(p.join(&to).join("sub/ro.txt").exists());
        // With it: all of it goes, a read-only file too, a link only as a link, and the record goes with the entry.
        assert_eq!(purge_in(&root, &to, true), Ok(Purged::Done { removed: 7, left: 0 }));
        assert!(!p.join(&to).exists() && !p.join(".tenrec-trash/.from").join(&id).exists());
        assert_eq!(std::fs::read_to_string(outside.join("keep.txt")).unwrap(), "keep");
    }

    #[test]
    fn a_purge_leaves_what_it_cannot_remove_and_says_so() {
        let (_d, p) = project();
        let root = root_of(&p);
        std::fs::create_dir_all(p.join("tree/locked")).unwrap();
        std::fs::write(p.join("tree/locked/x"), "x").unwrap();
        std::fs::write(p.join("tree/y"), "y").unwrap();
        std::fs::set_permissions(p.join("tree/locked"), std::fs::Permissions::from_mode(0o500)).unwrap();
        let to = trashed(delete_in(&root, "tree", None, false));
        let name = to.rsplit('/').next().unwrap().to_owned();
        let id = trash_id(&name).unwrap().to_owned();
        let done = purge_in(&root, &to, true);
        // What cannot be entered stays with what is in it (never changed with chmod), and so does the entry and its record.
        assert_eq!(done, Ok(Purged::Done { removed: 1, left: 2 }));
        assert!(p.join(&to).join("locked/x").exists() && !p.join(&to).join("y").exists());
        assert!(p.join(".tenrec-trash/.from").join(&id).exists());
        assert_eq!(std::fs::metadata(p.join(&to).join("locked")).unwrap().permissions().mode() & 0o777, 0o500);
        std::fs::set_permissions(p.join(&to).join("locked"), std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn a_purge_that_is_too_deep_or_too_big_deletes_nothing() {
        let (_d, p) = project();
        let root = root_of(&p);
        let deep = (0..66).map(|_| "d").collect::<Vec<_>>().join("/");
        std::fs::create_dir_all(p.join("deep").join(&deep)).unwrap();
        let to = trashed(delete_in(&root, "deep", None, false));
        assert!(purge_in(&root, &to, false).is_err());
        assert!(purge_in(&root, &to, true).is_err());
        assert!(p.join(&to).join("d/d").exists());
        std::fs::create_dir_all(p.join("many")).unwrap();
        for i in 0..=MAX_PURGE {
            std::fs::write(p.join("many").join(format!("f{i}")), "").unwrap();
        }
        let many = trashed(delete_in(&root, "many", None, false));
        assert!(purge_in(&root, &many, true).is_err());
        assert!(p.join(&many).join("f0").exists() && p.join(&many).join(format!("f{MAX_PURGE}")).exists());
    }

    #[test]
    fn only_an_entry_directly_in_the_trash_can_be_purged() {
        let (_d, p) = project();
        let root = root_of(&p);
        std::fs::write(p.join("a.txt"), "a").unwrap();
        std::fs::create_dir_all(p.join("d/inner")).unwrap();
        let to = trashed(delete_in(&root, "a.txt", None, false));
        let dir = trashed(delete_in(&root, "d", None, false));
        std::os::unix::fs::symlink(p.join(".tenrec-trash"), p.join("alias")).unwrap();
        let name = to.rsplit('/').next().unwrap().to_owned();
        for bad in [".tenrec-trash", ".tenrec-trash/.from", ".tenrec-trash/.gitignore", "a.txt", &format!("{dir}/inner"), &format!("alias/{name}"), ""] {
            assert!(purge_in(&root, bad, true).is_err(), "{bad:?}");
        }
        assert!(p.join(&to).exists() && p.join(&dir).join("inner").exists());
        // A file is purged too (counted as one).
        assert_eq!(purge_in(&root, &to, true), Ok(Purged::Done { removed: 1, left: 0 }));
    }

    #[test]
    fn emptying_the_trash_removes_the_entries_and_the_records_that_have_none() {
        let (_d, p) = project();
        let root = root_of(&p);
        assert!(empty_trash_in(&root, ".tenrec-trash", false).is_err(), "no trash yet");
        std::fs::create_dir_all(p.join("d/e")).unwrap();
        std::fs::write(p.join("d/e/f"), "ff").unwrap();
        std::fs::write(p.join("a.txt"), "a").unwrap();
        trashed(delete_in(&root, "d", None, false));
        let kept = trashed(delete_in(&root, "a.txt", None, false));
        // A record without an entry (the entry was moved out by hand).
        std::fs::write(p.join(".tenrec-trash/.from/1-abcdef01"), "x").unwrap();
        for bad in ["", "d", ".tenrec-trash/x"] {
            assert!(empty_trash_in(&root, bad, true).is_err(), "{bad:?}");
        }
        assert_eq!(empty_trash_in(&root, ".tenrec-trash", false), Ok(Purged::Summary { entries: 2, items: 4, bytes: 3, left: 0 }));
        assert!(p.join(&kept).exists());
        assert_eq!(empty_trash_in(&root, ".tenrec-trash", true), Ok(Purged::Done { removed: 4, left: 0 }));
        let rest: Vec<String> = std::fs::read_dir(p.join(".tenrec-trash")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(rest.iter().all(|n| n == ".from" || n == ".gitignore"), "{rest:?}");
        assert_eq!(std::fs::read_dir(p.join(".tenrec-trash/.from")).unwrap().count(), 0);
    }

    #[test]
    fn a_restore_that_asks_tells_where_the_entry_would_go() {
        let (_d, p) = project();
        let root = root_of(&p);
        std::fs::create_dir_all(p.join(".claude")).unwrap();
        std::fs::write(p.join(".claude/s.json"), "{}").unwrap();
        let to = trashed(delete_in(&root, ".claude/s.json", None, true));
        assert_eq!(restore_in_to(&root, &to, false), Ok((Changed::NeedsConfirm(match guard(Path::new(".claude/s.json")) { Guard::Confirm(w) => w, other => panic!("{other:?}") }), Some(".claude/s.json".to_owned()))));
        assert_eq!(restore_in_to(&root, &to, true), Ok((Changed::Moved(".claude/s.json".to_owned()), None)));
    }
    #[test]
    fn a_trash_that_is_a_link_is_neither_purged_nor_emptied() {
        let (d, p) = project();
        let root = root_of(&p);
        let outside = d.path().join("out");
        std::fs::create_dir_all(outside.join("sub")).unwrap();
        std::fs::write(outside.join("sub/keep.txt"), "keep").unwrap();
        std::os::unix::fs::symlink(&outside, p.join(".tenrec-trash")).unwrap();
        assert!(empty_trash_in(&root, ".tenrec-trash", true).is_err());
        assert!(purge_in(&root, ".tenrec-trash/sub", true).is_err());
        assert_eq!(std::fs::read_to_string(outside.join("sub/keep.txt")).unwrap(), "keep");
    }
}
