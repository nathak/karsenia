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

use super::container::{self, Spec};
use super::env::{pin_settings, EnvPlan};
use super::protocol::{parse_line, Event};

/// The longest stdout line read; a longer one ends the process (a stream that wrote it cannot be trusted to go on).
pub const MAX_LINE: u64 = 64 * 1024 * 1024;
const STDERR_KEEP: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct SpawnOpts {
    /// The `claude` program; the TENREC_CLAUDE variable overrides it (tests point it at the fake).
    pub bin: String,
    pub cwd: PathBuf,
    pub model: Option<String>,
    /// The CLI's own session id, to continue an earlier conversation.
    pub resume: Option<String>,
    /// A file naming the MCP servers this chat gets (it holds a secret, so it is a file and not part of argv).
    pub mcp_config: Option<PathBuf>,
    /// A file of notes appended to the system prompt (a file, so the notes are not in argv where any user can read them).
    pub system_file: Option<PathBuf>,
    /// A one-off plugin folder holding this chat's approved skills.
    pub plugin_dir: Option<PathBuf>,
    /// A provider chat's environment (empty for a normal chat). Its values hold the token, so it is never put into argv.
    pub env: EnvPlan,
    /// A provider chat's address: pinned on the command line, and project settings are not read (see `env`).
    pub pinned_url: Option<String>,
    /// Run inside a container (a provider chat only; EXPERIMENTAL, see `container`).
    pub container: Option<Spec>,
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
    /// The runtime and the container's name: killing the client of `docker run` does not stop the container, `rm -f` does.
    container: Option<(PathBuf, String)>,
}

pub fn claude_bin() -> String {
    std::env::var("TENREC_CLAUDE").unwrap_or_else(|_| "claude".to_owned())
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
    if let Some(dir) = &opts.plugin_dir {
        args.extend(["--plugin-dir".to_owned(), dir.display().to_string()]);
    }
    if let Some(url) = &opts.pinned_url {
        // The project's own settings are not read: its `env` could point the address (and with it the token) somewhere else, and its
        // hooks and key helpers would run beside a third party's key. The address is also given by value, which wins over the rest.
        args.extend(["--setting-sources".to_owned(), "user".to_owned(), "--settings".to_owned(), pin_settings(url)]);
    }
    args
}

impl ClaudeProcess {
    pub fn spawn(opts: &SpawnOpts) -> Result<(Self, mpsc::UnboundedReceiver<ProcEvent>)> {
        let (program, argv) = match &opts.container {
            Some(spec) => {
                let names: Vec<String> = opts.env.set.iter().map(|(name, _)| name.clone()).filter(|name| name != "CLAUDE_CONFIG_DIR").collect();
                // SAFETY: getuid and getgid only read the process's own ids.
                let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
                (spec.runtime.program.as_os_str().to_owned(), container::run_args(spec, &opts.cwd, &names, &args(opts), uid, gid).map_err(|e| anyhow::anyhow!(e))?)
            }
            None => (std::ffi::OsString::from(&opts.bin), args(opts)),
        };
        let mut command = Command::new(&program);
        command
            .args(argv)
            .current_dir(&opts.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        for name in &opts.env.remove {
            command.env_remove(name);
        }
        for (name, value) in &opts.env.set {
            // In a container the config folder is a path inside it, given by the runtime's own `-e`; the host path stays out.
            if opts.container.is_some() && name == "CLAUDE_CONFIG_DIR" {
                command.env_remove(name);
                continue;
            }
            command.env(name, value);
        }
        let mut child = command.spawn().with_context(|| format!("could not start {}", program.to_string_lossy()))?;
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

        let container = opts.container.as_ref().map(|spec| (spec.runtime.program.clone(), spec.name.clone()));
        Ok((Self { stdin: line_tx, pid, alive, container }, event_rx))
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
        if let Some((program, name)) = &self.container {
            container::remove(program, name, Duration::ZERO);
        }
    }

    /// Ends the process and everything it started: SIGTERM to its group, SIGKILL after three seconds.
    pub fn stop(&self) {
        kill_group(self.pid);
        if let Some((program, name)) = self.container.clone() {
            // The client passes SIGTERM on; whatever is left after three seconds is removed with the container.
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let _ = tokio::task::spawn_blocking(move || container::remove(&program, &name, Duration::from_secs(5))).await;
            });
        }
    }
}

impl Drop for ClaudeProcess {
    fn drop(&mut self) {
        kill_group(self.pid);
        if let Some((program, name)) = self.container.take() {
            // Started here and not waited for: the host may be ending, and the command goes on without it.
            container::remove(&program, &name, Duration::ZERO);
        }
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
        let opts = SpawnOpts { bin: "claude".into(), cwd: ".".into(), model: Some("haiku".into()), resume: Some("abc".into()), mcp_config: Some("/d/run/x.mcp.json".into()), system_file: Some("/d/run/x.mem.txt".into()), plugin_dir: Some("/d/run/x.skills".into()), env: EnvPlan::default(), pinned_url: None, container: None };
        let a = args(&opts);
        assert!(a.windows(2).any(|w| w == ["--permission-prompt-tool", "stdio"]));
        assert!(a.windows(2).any(|w| w == ["--model", "haiku"]));
        assert!(a.windows(2).any(|w| w == ["--resume", "abc"]));
        assert!(a.windows(2).any(|w| w == ["--mcp-config", "/d/run/x.mcp.json"]));
        assert!(a.windows(2).any(|w| w == ["--append-system-prompt-file", "/d/run/x.mem.txt"]));
        assert!(a.windows(2).any(|w| w == ["--plugin-dir", "/d/run/x.skills"]));
        let plain = args(&SpawnOpts { resume: None, model: None, mcp_config: None, system_file: None, plugin_dir: None, ..opts });
        assert!(!plain.iter().any(|x| x == "--resume" || x == "--model" || x == "--mcp-config" || x == "--append-system-prompt-file" || x == "--plugin-dir" || x == "--settings" || x == "--setting-sources"), "a normal chat's arguments are as they were");
    }

    #[test]
    fn a_provider_chat_pins_its_address_and_skips_project_settings_and_no_secret_is_in_the_arguments() {
        let env = EnvPlan { set: vec![("ANTHROPIC_AUTH_TOKEN".into(), "tok-secret-xyz".into())], remove: vec![] };
        let opts = SpawnOpts { bin: "claude".into(), cwd: ".".into(), model: None, resume: None, mcp_config: None, system_file: None, plugin_dir: None, env, pinned_url: Some("https://gw.example.com/x".into()), container: None };
        let a = args(&opts);
        assert!(a.windows(2).any(|w| w == ["--setting-sources", "user"]));
        let settings = a.windows(2).find(|w| w[0] == "--settings").map(|w| w[1].clone()).unwrap();
        assert_eq!(settings, r#"{"env":{"ANTHROPIC_BASE_URL":"https://gw.example.com/x"}}"#);
        assert!(!a.join(" ").contains("tok-secret-xyz"));
        assert!(!format!("{opts:?}").contains("tok-secret-xyz"), "Debug of the options hides the values");
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
