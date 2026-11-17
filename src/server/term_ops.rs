//! The terminal tab: a shell on a pseudo-terminal in the open chat's project folder. It is OFF unless the app was started with
//! `--terminal` (or TENREC_TERMINAL=1): whoever holds the page's token can type into it, so it must not be a setting a page can flip.
//!
//! Wire format. Controls are JSON text: `term_open {sid, req, cols, rows}` -> `term_opened {req, tid}`, `term_input {tid, data}`,
//! `term_resize {tid, cols, rows}`, `term_close {tid}`; a refusal is `term_error {req?, tid?, message}`. Output and the end of a shell
//! go out as binary frames in one ordered channel: `[1][tid u32 BE][bytes <= 16 KiB]` and `[2][tid u32 BE][exit code i32 BE]`.
//!
//! What the shell prints is never logged, never put in the event log or the index, and goes only to the socket that opened it.
//! This is not a sandbox: the shell runs as the person's own account and can read what that account can read.
use std::collections::{HashMap, VecDeque};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::unix::AsyncFd;
use tokio::io::Interest;
use tokio::sync::mpsc;

use super::{plain, AppState};

pub const FRAME_OUTPUT: u8 = 1;
pub const FRAME_EXIT: u8 = 2;
const MAX_CHUNK: usize = 16 * 1024;
const MAX_INPUT: usize = 4 * 1024;
const PER_SOCKET: usize = 2;
pub const IN_TOTAL: usize = 6;
const IDLE: Duration = Duration::from_secs(30 * 60);

/// The names a shell is given from the app's own environment. Everything else (provider keys, tokens, agent sockets, the runtime
/// folder) is left out: a list of what to keep, not of what to drop, so a name nobody thought of stays out.
fn keeps(name: &str) -> bool {
    matches!(name, "PATH" | "HOME" | "USER" | "LOGNAME" | "LANG" | "TZ") || name.starts_with("LC_")
}

/// `config_dir` is the settings folder of the chat's account when it is an extra one: a `claude` typed in this shell (or a
/// `claude auth login`) then belongs to the chat's account and not to the default one.
pub fn term_env(inherited: &[(String, String)], shell: &str, config_dir: Option<&Path>) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = inherited.iter().filter(|(k, _)| keeps(k)).cloned().collect();
    if let Some(dir) = config_dir {
        env.push(("CLAUDE_CONFIG_DIR".to_owned(), dir.display().to_string()));
    }
    for (k, v) in [("TERM", "dumb"), ("PAGER", "cat"), ("GIT_PAGER", "cat"), ("HISTFILE", ""), ("PS1", "\\w $ "), ("SHELL", shell)] {
        env.retain(|(name, _)| name != k);
        env.push((k.to_owned(), v.to_owned()));
    }
    env
}

pub fn clamp_size(cols: Option<u64>, rows: Option<u64>) -> (u16, u16) {
    (cols.unwrap_or(80).clamp(20, 500) as u16, rows.unwrap_or(24).clamp(5, 200) as u16)
}

enum Cmd {
    Input(Vec<u8>),
    Resize(u16, u16),
}

pub struct Term {
    cmd: mpsc::Sender<Cmd>,
    /// Dropped with the terminal: the task sees it even while input is backed up (the command channel is not read then).
    _stop: tokio::sync::oneshot::Sender<()>,
}

/// The terminals one socket opened. Dropping it ends them all: each task sees its command channel close and stops the shell.
#[derive(Default)]
pub struct Terms {
    map: HashMap<u32, Term>,
    next: u32,
}

/// One of the app's terminal places, given back when the terminal ends.
pub struct Slot(Arc<AtomicUsize>);

impl Slot {
    pub fn take(counter: &Arc<AtomicUsize>, most: usize) -> Option<Slot> {
        let mut now = counter.load(Ordering::SeqCst);
        loop {
            if now >= most {
                return None;
            }
            match counter.compare_exchange(now, now + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Some(Slot(counter.clone())),
                Err(seen) => now = seen,
            }
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn shell_program() -> (&'static str, &'static [&'static str]) {
    if Path::new("/bin/bash").exists() {
        ("/bin/bash", &["--noprofile", "--norc", "-i"])
    } else {
        ("/bin/sh", &["-i"])
    }
}

fn set_size(fd: i32, cols: u16, rows: u16) {
    let size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCSWINSZ on the master with a valid winsize that lives through the call.
    unsafe {
        libc::ioctl(fd, libc::TIOCSWINSZ, &size);
    }
}

/// The name of the slave side of a pseudo-terminal, into `name` (128 bytes, NUL-ended). False when it does not fit or is unknown.
#[cfg(not(target_vendor = "apple"))]
unsafe fn slave_name(master: libc::c_int, name: &mut [libc::c_char; 128]) -> bool {
    libc::ptsname_r(master, name.as_mut_ptr(), name.len()) == 0
}

/// Apple's libc has no `ptsname_r`: `ptsname` returns a static buffer, so one call at a time.
#[cfg(target_vendor = "apple")]
unsafe fn slave_name(master: libc::c_int, name: &mut [libc::c_char; 128]) -> bool {
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let p = libc::ptsname(master);
    if p.is_null() {
        return false;
    }
    let bytes = std::ffi::CStr::from_ptr(p).to_bytes_with_nul();
    if bytes.len() > name.len() {
        return false;
    }
    for (to, from) in name.iter_mut().zip(bytes) {
        *to = *from as libc::c_char;
    }
    true
}

/// Opens a pseudo-terminal and starts the shell on its slave side, as the leader of its own session and process group.
fn start_shell(cwd: &Path, inherited: Vec<(String, String)>, cols: u16, rows: u16, config_dir: Option<&Path>) -> Result<(OwnedFd, tokio::process::Child), String> {
    // The master and the slave are opened close-on-exec from the start: a program started by another thread of the app
    // between the two calls (claude, a container) must not inherit them.
    // SAFETY: posix_openpt returns a descriptor we own; grantpt/unlockpt/ptsname_r work on it; ptsname_r writes into our buffer.
    let (master, slave) = unsafe {
        let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if m < 0 {
            return Err("no pseudo-terminal could be opened".to_owned());
        }
        let master = OwnedFd::from_raw_fd(m);
        let mut name = [0 as libc::c_char; 128];
        if libc::grantpt(m) != 0 || libc::unlockpt(m) != 0 || !slave_name(m, &mut name) {
            return Err("no pseudo-terminal could be opened".to_owned());
        }
        let s = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if s < 0 {
            return Err("no pseudo-terminal could be opened".to_owned());
        }
        let flags = libc::fcntl(m, libc::F_GETFL);
        libc::fcntl(m, libc::F_SETFL, flags | libc::O_NONBLOCK);
        (master, OwnedFd::from_raw_fd(s))
    };
    set_size(master.as_raw_fd(), cols, rows);
    let (program, args) = shell_program();
    let clone = |slave: &OwnedFd| slave.try_clone().map_err(|_| "no pseudo-terminal could be opened".to_owned());
    let mut command = tokio::process::Command::new(program);
    command.args(args).current_dir(cwd).env_clear().envs(term_env(&inherited, program, config_dir)).stdin(Stdio::from(clone(&slave)?)).stdout(Stdio::from(clone(&slave)?)).stderr(Stdio::from(slave)).kill_on_drop(true);
    // SAFETY: only setsid and ioctl run between fork and exec; both are async-signal-safe and allocate nothing.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The slave is already standard input here: it becomes the controlling terminal of the new session.
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().map_err(|error| format!("the shell could not be started: {error}"))?;
    Ok((master, child))
}

/// Sends `signal` to every process of the shell's session (the shell, and the jobs it started, each of which has a group of its
/// own under job control, so a signal to the shell's group alone would miss them). The shell must not have been waited for yet:
/// its pid is then still the session id and cannot belong to anything else.
fn kill_session(pid: i32, signal: i32) {
    if pid <= 1 {
        return;
    }
    let Ok(entries) = std::fs::read_dir("/proc") else { return };
    for entry in entries.flatten() {
        let Some(other) = entry.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
        // "pid (name) state ppid pgrp session ...": the name may hold spaces and brackets, so count from its last ")".
        let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else { continue };
        if rest.split_whitespace().nth(3).and_then(|v| v.parse::<i32>().ok()) == Some(pid) {
            // SAFETY: a process of the session this module started.
            unsafe {
                libc::kill(other, signal);
            }
        }
    }
}

/// True once the process has ended but has not been waited for (so its pid, and with it the session id, cannot be reused yet).
fn ended(pid: i32) -> bool {
    // SAFETY: waitid with WNOHANG|WNOWAIT only looks; the zeroed siginfo is a valid out-parameter.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        libc::waitid(libc::P_PID, pid as u32, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) == 0 && info.si_pid() != 0
    }
}

/// Hangs up the shell and everything it started, then kills what is left, and returns the shell's exit code.
async fn terminate(child: &mut tokio::process::Child) -> i32 {
    let pid = child.id().map(|p| p as i32).unwrap_or(0);
    kill_session(pid, libc::SIGHUP);
    for _ in 0..40 {
        if ended(pid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Before the shell is waited for, so the session id still belongs to this session; a job that ignores SIGHUP ends here.
    kill_session(pid, libc::SIGKILL);
    match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(status)) => status.code().unwrap_or(-1),
        _ => -1,
    }
}

/// The shells that are running (their pids), so the app can end them when it ends.
static SHELLS: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());

/// Kills every shell and what it started; for the app's own shutdown (nothing waits for an answer).
pub fn kill_all() {
    let pids: Vec<i32> = SHELLS.lock().map(|l| l.clone()).unwrap_or_default();
    for pid in pids {
        kill_session(pid, libc::SIGKILL);
    }
}

fn read_fd(fd: i32, buf: &mut [u8]) -> std::io::Result<usize> {
    // SAFETY: reads at most buf.len() bytes into a buffer we own.
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    if n < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

fn write_fd(fd: i32, buf: &[u8]) -> std::io::Result<usize> {
    // SAFETY: writes at most buf.len() bytes from a buffer we own.
    let n = unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
    if n < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

fn frame(kind: u8, tid: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + body.len());
    out.push(kind);
    out.extend_from_slice(&tid.to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// Owns one shell: carries its output to the socket (reading only as fast as the page takes it, so a slow page stops the shell
/// instead of filling memory), its input to the shell, and ends it when the page lets go, the shell ends, or nothing happens for `idle`.
async fn run(master: OwnedFd, mut child: tokio::process::Child, ctl: (mpsc::Receiver<Cmd>, tokio::sync::oneshot::Receiver<()>), tid: u32, bin: mpsc::Sender<Vec<u8>>, idle: Duration, _slot: Slot) {
    // The commands, and the signal that the terminal was closed.
    let (mut rx, mut stop) = ctl;
    let fd = master.as_raw_fd();
    let pid = child.id().map(|p| p as i32).unwrap_or(0);
    if let Ok(mut list) = SHELLS.lock() {
        list.push(pid);
    }
    // SAFETY: the descriptor is non-blocking and owned by the AsyncFd from here until it is dropped; nothing else closes it.
    let registered = unsafe { AsyncFd::register_with_interest(master, Interest::READABLE | Interest::WRITABLE) };
    let Ok(master) = registered else {
        let code = terminate(&mut child).await;
        if let Ok(mut list) = SHELLS.lock() {
            list.retain(|p| *p != pid);
        }
        let _ = bin.send(frame(FRAME_EXIT, tid, &code.to_be_bytes())).await;
        return;
    };
    let mut buf = vec![0u8; MAX_CHUNK];
    let mut waiting: Option<Vec<u8>> = None;
    let mut typed: VecDeque<u8> = VecDeque::new();
    let quiet = tokio::time::sleep(idle);
    tokio::pin!(quiet);
    // A job that keeps the terminal open keeps the master from ever reporting the end, so the shell itself is looked at too.
    let mut look = tokio::time::interval(Duration::from_millis(500));
    // The shell itself ended (not a close, not the idle limit): what it printed last is still sent before the end is told.
    let mut exited = false;
    loop {
        tokio::select! {
            // The terminal was closed (or its socket ended): always watched, also while input is not taken.
            _ = &mut stop => break,
            // Input is taken only while little is waiting: a shell that reads nothing makes the page's sends fail ("busy").
            cmd = rx.recv(), if typed.len() < 64 * 1024 => match cmd {
                None => break,
                Some(Cmd::Input(bytes)) => {
                    typed.extend(bytes);
                    quiet.as_mut().reset(tokio::time::Instant::now() + idle);
                }
                Some(Cmd::Resize(cols, rows)) => set_size(fd, cols, rows),
            },
            ready = master.readable(), if waiting.is_none() => {
                let Ok(mut guard) = ready else { break };
                match guard.try_io(|inner| read_fd(inner.as_raw_fd(), &mut buf)) {
                    Ok(Ok(0)) | Ok(Err(_)) => {
                        exited = true;
                        break;
                    }
                    Ok(Ok(n)) => {
                        waiting = Some(frame(FRAME_OUTPUT, tid, &buf[..n]));
                        quiet.as_mut().reset(tokio::time::Instant::now() + idle);
                    }
                    Err(_would_block) => {}
                }
            }
            permit = bin.reserve(), if waiting.is_some() => match permit {
                Ok(permit) => {
                    if let Some(out) = waiting.take() {
                        permit.send(out);
                    }
                }
                Err(_) => break,
            },
            ready = master.writable(), if !typed.is_empty() => {
                let Ok(mut guard) = ready else { break };
                let result = guard.try_io(|inner| {
                    let (first, _) = typed.as_slices();
                    write_fd(inner.as_raw_fd(), first)
                });
                match result {
                    Ok(Ok(n)) => {
                        typed.drain(..n);
                    }
                    Ok(Err(_)) => break,
                    Err(_would_block) => {}
                }
            }
            _ = look.tick() => {
                if ended(pid) {
                    exited = true;
                    break;
                }
            }
            _ = &mut quiet => break,
        }
    }
    if exited {
        // Held back for a full channel, then whatever is still in the terminal: all of it within two seconds (a slow page or a job
        // that keeps the terminal open cannot hold the end back), and in order (a frame that could not be sent ends the drain).
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            if let Some(out) = waiting.take() {
                if bin.send(out).await.is_err() {
                    return;
                }
            }
            let mut sent = 0;
            while sent < 256 * 1024 {
                match read_fd(master.as_raw_fd(), &mut buf) {
                    Ok(n) if n > 0 => {
                        sent += n;
                        if bin.send(frame(FRAME_OUTPUT, tid, &buf[..n])).await.is_err() {
                            return;
                        }
                    }
                    _ => return,
                }
            }
        })
        .await;
    }
    drop(master);
    let code = terminate(&mut child).await;
    if let Ok(mut list) = SHELLS.lock() {
        list.retain(|p| *p != pid);
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), bin.send(frame(FRAME_EXIT, tid, &code.to_be_bytes()))).await;
}

/// Starts a shell and the task that serves it; returns its input side.
fn spawn_term(cwd: &Path, size: (u16, u16), tid: u32, bin: mpsc::Sender<Vec<u8>>, slot: Slot, idle: Duration, config_dir: Option<PathBuf>) -> Result<Term, String> {
    let (cols, rows) = size;
    let inherited: Vec<(String, String)> = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect();
    let (master, child) = start_shell(cwd, inherited, cols, rows, config_dir.as_deref())?;
    let (cmd, rx) = mpsc::channel(64);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(run(master, child, (rx, stop_rx), tid, bin, idle, slot));
    Ok(Term { cmd, _stop: stop_tx })
}

/// Whether a terminal may be opened in this project folder: the same refusals as the editor's, a folder the app may run in, and
/// not a chat that runs in a container (this shell runs on the host, so it would not be the chat's place).
fn check(project: &Path, container: bool, data_dir: &Path, home: Option<&Path>) -> Result<PathBuf, String> {
    if container {
        return Err("a terminal is not available for a container chat".to_owned());
    }
    // The shell starts in the path that was checked, not in the project's name resolved a second time.
    let root = super::fs_ops::check_project(project, data_dir, home, true)?;
    if !crate::claude::process::is_safe_cwd(project) {
        return Err("a terminal is not available for this folder".to_owned());
    }
    Ok(root)
}

pub async fn handle(state: &AppState, name: &str, op: &Value, out: &mpsc::UnboundedSender<String>, bin: &mpsc::Sender<Vec<u8>>, terms: &mut Terms) {
    let req = op["req"].clone();
    let tid = op["tid"].as_u64().and_then(|t| u32::try_from(t).ok());
    let refuse = |message: &str| {
        let _ = out.send(plain("term_error", None, json!({"req": req, "tid": tid, "message": message})));
    };
    if !state.terminal {
        return refuse("the terminal is off: start the app with --terminal");
    }
    match name {
        "term_open" => {
            terms.map.retain(|_, t| !t.cmd.is_closed());
            if terms.map.len() >= PER_SOCKET {
                return refuse("too many terminals are open on this page");
            }
            let sid = op["sid"].as_str().unwrap_or_default();
            let Some(handle) = state.registry.get(sid) else { return refuse("no such session") };
            let Some(info) = handle.info().await else { return refuse("no such session") };
            let home = std::env::var_os("HOME").map(PathBuf::from);
            let cwd = match check(&info.meta.cwd, info.meta.container, state.registry.dir(), home.as_deref()) {
                Ok(cwd) => cwd,
                Err(message) => return refuse(&message),
            };
            // A chat of an extra account gets a shell of that account: its settings folder is looked at again here.
            let config_dir = match &info.meta.account {
                Some(account) => match state.registry.account_login(&account.id) {
                    Ok((dir, _)) => Some(dir),
                    Err(message) => return refuse(&message),
                },
                None => None,
            };
            let Some(slot) = Slot::take(&state.term_slots, IN_TOTAL) else { return refuse("too many terminals are open") };
            let (cols, rows) = clamp_size(op["cols"].as_u64(), op["rows"].as_u64());
            terms.next += 1;
            let id = terms.next;
            match spawn_term(&cwd, (cols, rows), id, bin.clone(), slot, IDLE, config_dir) {
                Ok(term) => {
                    terms.map.insert(id, term);
                    let _ = out.send(plain("term_opened", Some(sid), json!({"req": req, "tid": id})));
                }
                Err(message) => refuse(&message),
            }
        }
        "term_input" | "term_resize" | "term_close" => {
            let Some(term) = tid.and_then(|t| terms.map.get(&t)) else { return refuse("no such terminal") };
            match name {
                "term_input" => {
                    let data = op["data"].as_str().unwrap_or_default();
                    if data.len() > MAX_INPUT {
                        return refuse("too much input at once");
                    }
                    if term.cmd.try_send(Cmd::Input(data.as_bytes().to_vec())).is_err() {
                        refuse("the terminal is busy");
                    }
                }
                "term_resize" => {
                    let (cols, rows) = clamp_size(op["cols"].as_u64(), op["rows"].as_u64());
                    let _ = term.cmd.try_send(Cmd::Resize(cols, rows));
                }
                _ => {
                    if let Some(t) = tid {
                        terms.map.remove(&t);
                    }
                }
            }
        }
        _ => refuse("unknown op"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn a_shell_of_an_accounts_chat_gets_that_accounts_settings_folder_and_a_default_chat_gets_none() {
        let inherited = pairs(&[("PATH", "/bin"), ("CLAUDE_CONFIG_DIR", "/home/me/.claude"), ("ANTHROPIC_API_KEY", "k")]);
        let plain = term_env(&inherited, "/bin/bash", None);
        assert!(!plain.iter().any(|(k, _)| k == "CLAUDE_CONFIG_DIR" || k == "ANTHROPIC_API_KEY"), "the default account's folder and key are not handed to a shell");
        let account = term_env(&inherited, "/bin/bash", Some(Path::new("/data/accounts/ab")));
        let values: Vec<&str> = account.iter().filter(|(k, _)| k == "CLAUDE_CONFIG_DIR").map(|(_, v)| v.as_str()).collect();
        assert_eq!(values, ["/data/accounts/ab"], "exactly the account's folder, once");
        assert!(!account.iter().any(|(k, _)| k == "ANTHROPIC_API_KEY"));
    }

    #[test]
    fn a_shell_gets_only_the_names_on_the_list() {
        let env = term_env(
            &pairs(&[("PATH", "/bin"), ("HOME", "/h"), ("LC_ALL", "C"), ("OPENROUTER_API_KEY", "k"), ("ANTHROPIC_API_KEY", "k"), ("TENREC_MCP_SOCKET", "s"), ("SSH_AUTH_SOCK", "x"), ("XDG_RUNTIME_DIR", "r"), ("GITHUB_TOKEN", "t"), ("PS1", "mine")]),
            "/bin/bash",
            None,
        );
        let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        for kept in ["PATH", "HOME", "LC_ALL", "TERM", "PAGER", "HISTFILE", "PS1", "SHELL"] {
            assert!(names.contains(&kept), "{kept} is given");
        }
        for left in ["OPENROUTER_API_KEY", "ANTHROPIC_API_KEY", "TENREC_MCP_SOCKET", "SSH_AUTH_SOCK", "XDG_RUNTIME_DIR", "GITHUB_TOKEN"] {
            assert!(!names.contains(&left), "{left} is not given");
        }
        assert_eq!(env.iter().filter(|(k, _)| k == "PS1").count(), 1);
        assert_eq!(env.iter().find(|(k, _)| k == "PS1").unwrap().1, "\\w $ ");
        assert_eq!(env.iter().find(|(k, _)| k == "TERM").unwrap().1, "dumb");
    }

    #[test]
    fn the_size_is_kept_in_range() {
        assert_eq!(clamp_size(None, None), (80, 24));
        assert_eq!(clamp_size(Some(0), Some(0)), (20, 5));
        assert_eq!(clamp_size(Some(100_000), Some(100_000)), (500, 200));
        assert_eq!(clamp_size(Some(132), Some(40)), (132, 40));
    }

    #[test]
    fn places_are_counted_and_given_back() {
        let counter = Arc::new(AtomicUsize::new(0));
        let a = Slot::take(&counter, 2).unwrap();
        let _b = Slot::take(&counter, 2).unwrap();
        assert!(Slot::take(&counter, 2).is_none());
        drop(a);
        assert!(Slot::take(&counter, 2).is_some());
    }

    #[test]
    fn a_container_chat_and_the_home_folder_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("project");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        assert!(check(&project, true, &data, Some(dir.path())).is_err());
        assert!(check(&data, false, &data, Some(dir.path())).is_err());
        assert!(check(&project, false, &data, None).is_err(), "no home folder known");
        assert!(check(&project, false, &data, Some(dir.path())).is_ok());
    }

    /// A shell on a real pseudo-terminal: what is typed comes back, the size can be changed, and ending it ends its group too.
    #[tokio::test]
    async fn a_shell_runs_in_the_folder_and_ends_with_its_group() {
        let dir = tempfile::tempdir().unwrap();
        let (bin_tx, mut bin_rx) = mpsc::channel::<Vec<u8>>(32);
        let counter = Arc::new(AtomicUsize::new(0));
        let slot = Slot::take(&counter, 1).unwrap();
        let term = spawn_term(dir.path(), (100, 30), 7, bin_tx, slot, Duration::from_secs(60), None).unwrap();
        term.cmd.send(Cmd::Input(b"pwd; stty size; sleep 1000 & echo pid=$!; \n".to_vec())).await.unwrap();
        let mut seen = String::new();
        let pid = loop {
            let got = tokio::time::timeout(Duration::from_secs(8), bin_rx.recv()).await.expect("timed out").expect("closed");
            assert_eq!(got[0], FRAME_OUTPUT);
            assert_eq!(&got[1..5], &7u32.to_be_bytes());
            seen.push_str(&String::from_utf8_lossy(&got[5..]));
            // The typed line is echoed too ("pid=$!"), so the number is read from a line that holds digits.
            let found = seen.lines().find_map(|line| line.trim().strip_prefix("pid=").and_then(|r| r.parse::<i32>().ok()));
            if let Some(pid) = found {
                break pid;
            }
        };
        assert!(seen.contains(&dir.path().canonicalize().unwrap().display().to_string()), "{seen}");
        assert!(seen.contains("30 100"), "{seen}");
        drop(term);
        // The shell is told to end; then its exit comes as the last frame, and the background job is gone.
        let exit = loop {
            let got = tokio::time::timeout(Duration::from_secs(8), bin_rx.recv()).await.expect("timed out").expect("closed");
            if got[0] == FRAME_EXIT {
                break got;
            }
        };
        assert_eq!(exit.len(), 9);
        let gone = tokio::time::timeout(Duration::from_secs(5), async {
            // SAFETY: signal 0 only asks whether the process exists.
            while unsafe { libc::kill(pid, 0) } == 0 {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        assert!(gone.is_ok(), "the background job outlived the terminal");
    }

    #[tokio::test]
    async fn a_quiet_shell_is_ended() {
        let dir = tempfile::tempdir().unwrap();
        let (bin_tx, mut bin_rx) = mpsc::channel::<Vec<u8>>(32);
        let counter = Arc::new(AtomicUsize::new(0));
        let slot = Slot::take(&counter, 1).unwrap();
        let _term = spawn_term(dir.path(), (80, 24), 1, bin_tx, slot, Duration::from_millis(600), None).unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match bin_rx.recv().await {
                    Some(got) if got[0] == FRAME_EXIT => return true,
                    Some(_) => {}
                    None => return false,
                }
            }
        })
        .await;
        assert_eq!(ended, Ok(true));
        // The place is given back once the task is over.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    /// Reads output until a line `PID<number>` has come.
    async fn pid_from(bin_rx: &mut mpsc::Receiver<Vec<u8>>) -> i32 {
        let mut seen = String::new();
        loop {
            let got = tokio::time::timeout(Duration::from_secs(8), bin_rx.recv()).await.expect("timed out").expect("closed");
            if got[0] == FRAME_OUTPUT {
                seen.push_str(&String::from_utf8_lossy(&got[5..]));
                if let Some(pid) = seen.lines().find_map(|l| l.trim().strip_prefix("PID").and_then(|r| r.parse::<i32>().ok())) {
                    return pid;
                }
            }
        }
    }

    async fn gone(pid: i32) -> bool {
        tokio::time::timeout(Duration::from_secs(8), async {
            // SAFETY: signal 0 only asks whether the process exists.
            while unsafe { libc::kill(pid, 0) } == 0 {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .is_ok()
    }

    /// A job that ignores SIGHUP has a process group of its own under job control: only a kill of the whole session reaches it.
    #[tokio::test]
    async fn a_job_that_ignores_hangup_is_killed_with_the_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let (bin_tx, mut bin_rx) = mpsc::channel::<Vec<u8>>(32);
        let counter = Arc::new(AtomicUsize::new(0));
        let term = spawn_term(dir.path(), (80, 24), 1, bin_tx, Slot::take(&counter, 1).unwrap(), Duration::from_secs(60), None).unwrap();
        term.cmd.send(Cmd::Input(b"trap '' HUP; sleep 1000 & echo PID$!\n".to_vec())).await.unwrap();
        let pid = pid_from(&mut bin_rx).await;
        drop(term);
        assert!(gone(pid).await, "a job that ignores SIGHUP outlived the terminal");
    }

    /// A shell that exits while a job still holds the terminal open is noticed, and its job ends.
    #[tokio::test]
    async fn a_shell_that_exits_with_a_job_running_is_ended() {
        let dir = tempfile::tempdir().unwrap();
        let (bin_tx, mut bin_rx) = mpsc::channel::<Vec<u8>>(32);
        let counter = Arc::new(AtomicUsize::new(0));
        let _term = spawn_term(dir.path(), (80, 24), 1, bin_tx, Slot::take(&counter, 1).unwrap(), Duration::from_secs(60), None).unwrap();
        _term.cmd.send(Cmd::Input(b"sleep 1000 & echo PID$!; exit\n".to_vec())).await.unwrap();
        let pid = pid_from(&mut bin_rx).await;
        let exit = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                match bin_rx.recv().await {
                    Some(got) if got[0] == FRAME_EXIT => return true,
                    Some(_) => {}
                    None => return false,
                }
            }
        })
        .await;
        assert_eq!(exit, Ok(true), "the end of the shell was not noticed");
        assert!(gone(pid).await, "the job outlived the shell");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 0, "the place is not given back");
    }
}
