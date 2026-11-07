//! One `claude` child process: its stdin as a queue of lines, its stdout parsed into events, its stderr kept short.
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use super::protocol::{parse_line, Event};

/// The longest stdout line read; a longer one ends the process (a stream that wrote it cannot be trusted to go on).
pub const MAX_LINE: u64 = 64 * 1024 * 1024;
const STDERR_KEEP: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct SpawnOpts {
    /// The `claude` program; the SWB_CLAUDE variable overrides it (tests point it at the fake).
    pub bin: String,
    pub cwd: PathBuf,
    pub model: Option<String>,
    /// The CLI's own session id, to continue an earlier conversation.
    pub resume: Option<String>,
    /// A file naming the MCP servers this chat gets (it holds a secret, so it is a file and not part of argv).
    pub mcp_config: Option<PathBuf>,
    /// A file of notes appended to the system prompt (a file, so the notes are not in argv where any user can read them).
    pub system_file: Option<PathBuf>,
}

#[derive(Debug)]
pub enum ProcEvent {
    Line(Event),
    /// The tail of stderr, sent once when the process ended.
    Stderr(String),
    Exited(Option<i32>),
}

pub struct ClaudeProcess {
    stdin: mpsc::UnboundedSender<String>,
    pid: Option<u32>,
    /// False once stdout has ended: a line queued after that would never be read.
    alive: Arc<AtomicBool>,
}

pub fn claude_bin() -> String {
    std::env::var("SWB_CLAUDE").unwrap_or_else(|_| "claude".to_owned())
}

pub fn args(opts: &SpawnOpts) -> Vec<String> {
    let mut args: Vec<String> = ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--permission-prompt-tool", "stdio"]
        .iter()
        .map(|a| (*a).to_owned())
        .collect();
    if let Some(model) = &opts.model {
        args.extend(["--model".to_owned(), model.clone()]);
    }
    if let Some(resume) = &opts.resume {
        args.extend(["--resume".to_owned(), resume.clone()]);
    }
    if let Some(config) = &opts.mcp_config {
        args.extend(["--mcp-config".to_owned(), config.display().to_string()]);
    }
    if let Some(file) = &opts.system_file {
        args.extend(["--append-system-prompt-file".to_owned(), file.display().to_string()]);
    }
    args
}

impl ClaudeProcess {
    pub fn spawn(opts: &SpawnOpts) -> Result<(Self, mpsc::UnboundedReceiver<ProcEvent>)> {
        let mut command = Command::new(&opts.bin);
        command
            .args(args(opts))
            .current_dir(&opts.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command.spawn().with_context(|| format!("could not start {}", opts.bin))?;
        let pid = child.id();
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;

        let alive = Arc::new(AtomicBool::new(true));
        let reader_alive = alive.clone();
        let (line_tx, mut line_rx) = mpsc::unbounded_channel::<String>();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<ProcEvent>();

        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(line) = line_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.write_all(b"\n").await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        let tail = tokio::spawn(async move {
            let mut kept: Vec<u8> = Vec::new();
            let mut stderr = stderr;
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                kept.extend_from_slice(&chunk[..n]);
                if kept.len() > STDERR_KEEP {
                    kept.drain(..kept.len() - STDERR_KEEP);
                }
            }
            String::from_utf8_lossy(&kept).into_owned()
        });

        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut buf: Vec<u8> = Vec::new();
            let mut too_long = false;
            loop {
                buf.clear();
                match (&mut reader).take(MAX_LINE).read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if buf.last() != Some(&b'\n') && buf.len() as u64 >= MAX_LINE {
                            too_long = true;
                            break;
                        }
                        let line = String::from_utf8_lossy(&buf);
                        if !line.trim().is_empty() && event_tx.send(ProcEvent::Line(parse_line(&line))).is_err() {
                            break;
                        }
                    }
                }
            }
            reader_alive.store(false, Ordering::SeqCst);
            if too_long {
                kill_group(pid);
            }
            let code = wait_for(&mut child).await;
            let tail_text = tail.await.unwrap_or_default();
            let mut tail_text = tail_text;
            if too_long {
                tail_text.push_str("\n[host] a line longer than the limit ended the process\n");
            }
            let _ = event_tx.send(ProcEvent::Stderr(tail_text));
            let _ = event_tx.send(ProcEvent::Exited(code));
        });

        Ok((Self { stdin: line_tx, pid, alive }, event_rx))
    }

    /// Queues one line for stdin; false when the process is gone.
    pub fn send_line(&self, line: String) -> bool {
        self.alive.load(Ordering::SeqCst) && self.stdin.send(line).is_ok()
    }

    /// SIGKILL to the whole group now, for a host that is exiting and cannot wait three seconds.
    pub fn kill_now(&self) {
        if let Some(pid) = self.pid {
            // SAFETY: killpg with a process group id this host created (process_group(0) at spawn).
            unsafe {
                libc::killpg(pid as i32, libc::SIGKILL);
            }
        }
    }

    /// Ends the process and everything it started: SIGTERM to its group, SIGKILL after three seconds.
    pub fn stop(&self) {
        kill_group(self.pid);
    }
}

impl Drop for ClaudeProcess {
    fn drop(&mut self) {
        kill_group(self.pid);
    }
}

fn kill_group(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    let group = pid as i32;
    // SAFETY: killpg with a process group id this host created (process_group(0) at spawn).
    unsafe {
        libc::killpg(group, libc::SIGTERM);
    }
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        // SAFETY: as above; a group that already ended makes this a no-op.
        unsafe {
            libc::killpg(group, libc::SIGKILL);
        }
    });
}

async fn wait_for(child: &mut Child) -> Option<i32> {
    match child.wait().await {
        Ok(status) => status.code(),
        Err(_) => None,
    }
}

/// True when the path exists and is a directory the host may run a session in: not `/`, not the home folder itself.
pub fn is_safe_cwd(path: &Path) -> bool {
    let Ok(real) = path.canonicalize() else { return false };
    if !real.is_dir() || real == Path::new("/") {
        return false;
    }
    match std::env::var_os("HOME").map(PathBuf::from).and_then(|home| home.canonicalize().ok()) {
        Some(home) => real != home,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arguments_include_the_permission_prompt_tool_and_resume() {
        let opts = SpawnOpts { bin: "claude".into(), cwd: ".".into(), model: Some("haiku".into()), resume: Some("abc".into()), mcp_config: Some("/d/run/x.mcp.json".into()), system_file: Some("/d/run/x.mem.txt".into()) };
        let a = args(&opts);
        assert!(a.windows(2).any(|w| w == ["--permission-prompt-tool", "stdio"]));
        assert!(a.windows(2).any(|w| w == ["--model", "haiku"]));
        assert!(a.windows(2).any(|w| w == ["--resume", "abc"]));
        assert!(a.windows(2).any(|w| w == ["--mcp-config", "/d/run/x.mcp.json"]));
        assert!(a.windows(2).any(|w| w == ["--append-system-prompt-file", "/d/run/x.mem.txt"]));
        let plain = args(&SpawnOpts { resume: None, model: None, mcp_config: None, system_file: None, ..opts });
        assert!(!plain.iter().any(|x| x == "--resume" || x == "--model" || x == "--mcp-config" || x == "--append-system-prompt-file"));
    }

    #[test]
    fn root_and_home_are_not_a_session_folder() {
        assert!(!is_safe_cwd(Path::new("/")));
        assert!(!is_safe_cwd(Path::new("/definitely/not/here")));
        if let Some(home) = std::env::var_os("HOME") {
            assert!(!is_safe_cwd(Path::new(&home)));
        }
        assert!(is_safe_cwd(Path::new("/tmp")));
    }
}
