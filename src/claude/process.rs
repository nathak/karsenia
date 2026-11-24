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
use super::env::EnvPlan;
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
    /// The permission mode chosen when the chat was made; the default adds no argument.
    pub permission_mode: crate::claude::perm::PermMode,
    /// The name a person gave the chat: passed as `--name`, so it is the CLI's own display name of the session too (its `/resume` picker
    /// and terminal title; measured: it is written to the session file as a `custom-title`). None for a chat nobody named.
    pub name: Option<String>,
    /// Run without the SmartWorkbench plug-in (this app's own workbench does its work): `--settings` switches it off for this process only.
    pub plugins_off: bool,
    /// The command of the PreToolUse hook that runs this app's guard (`<exe> guard --data <folder> --sid <chat>`), when it is on.
    pub guard_hook: Option<String>,
    /// The Stop hook of the same workbench (it reads the chat's file when it runs, so turning the gate on needs no restart).
    pub stop_hook: Option<String>,
    /// Where the CLI writes its own debug log (`--debug-file`, which also turns its debug mode on); None for an ordinary chat.
    pub debug_file: Option<PathBuf>,
    /// The app's own team mode: the helper subagents as a `--agents` JSON value (see `workbench::team`); None when team mode is off.
    pub agents: Option<String>,
    /// The lead's effort (`--effort`); None leaves the CLI's own.
    pub effort: Option<String>,
    /// Only the person's own (user) settings are read, not the project's (a decision the person made about this project's configuration,
    /// see `project_check`): `--setting-sources user`, given once even with a provider's pinned address.
    pub user_settings_only: bool,
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
    group: Option<Arc<crate::proc_group::Group>>,
    /// False once stdout has ended: a line queued after that would never be read.
    alive: Arc<AtomicBool>,
    /// Set when this host ended the process (`stop`): its exit is then reported with no code, whatever code the system gave (on
    /// Windows a process that is ended gets the code 1, which is also what a failure looks like).
    stopped: Arc<AtomicBool>,
    /// The runtime and the container's name: killing the client of `docker run` does not stop the container, `rm -f` does.
    container: Option<(PathBuf, String)>,
}

/// The SmartWorkbench plug-in as Claude Code names it (`<plugin>@<marketplace>`).
pub const SWB_PLUGIN: &str = "smartworkbench@smart-workbench";

pub fn claude_bin() -> String {
    std::env::var("TENREC_CLAUDE").unwrap_or_else(|_| "claude".to_owned())
}

pub fn args(opts: &SpawnOpts) -> Vec<String> {
    let mut args: Vec<String> = ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--permission-prompt-tool", "stdio"]
        .iter()
        .map(|a| (*a).to_owned())
        .collect();
    if let Some(name) = opts.name.as_ref().filter(|n| !n.is_empty()) {
        args.extend(["--name".to_owned(), name.clone()]);
    }
    if let Some(mode) = opts.permission_mode.cli_arg() {
        args.extend(["--permission-mode".to_owned(), mode.to_owned()]);
    }
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
    if let Some(json) = &opts.agents {
        args.extend(["--agents".to_owned(), json.clone()]);
    }
    if let Some(level) = &opts.effort {
        args.extend(["--effort".to_owned(), level.clone()]);
    }
    if let Some(file) = &opts.debug_file {
        args.extend(["--debug-file".to_owned(), file.display().to_string()]);
    }
    // One `--settings` value carries everything this app sets by value: the provider's address and the plug-ins it switches off.
    let mut settings = serde_json::Map::new();
    if opts.pinned_url.is_some() || opts.user_settings_only {
        // The project's own settings are not read: its `env` could point the address (and with it the token) somewhere else, and its
        // hooks and key helpers would run beside a third party's key. The address is also given by value, which wins over the rest.
        args.extend(["--setting-sources".to_owned(), "user".to_owned()]);
    }
    if let Some(url) = &opts.pinned_url {
        settings.insert("env".into(), serde_json::json!({"ANTHROPIC_BASE_URL": url}));
    }
    if opts.plugins_off {
        settings.insert("enabledPlugins".into(), serde_json::json!({ SWB_PLUGIN: false }));
    }
    if let Some(command) = &opts.guard_hook {
        // A project's own `disableAllHooks` must not switch the guard off.
        settings.insert("disableAllHooks".into(), serde_json::Value::Bool(false));
        let mut hooks = serde_json::json!({"PreToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": command, "timeout": 10}]}]});
        if let Some(stop) = &opts.stop_hook {
            hooks["Stop"] = serde_json::json!([{"hooks": [{"type": "command", "command": stop, "timeout": 10}]}]);
        }
        settings.insert("hooks".into(), hooks);
    }
    if !settings.is_empty() {
        args.extend(["--settings".to_owned(), serde_json::Value::Object(settings).to_string()]);
    }
    args
}

/// The ids a container runs as. Containers are not available on Windows.
#[cfg(unix)]
fn own_ids() -> Result<(u32, u32)> {
    // SAFETY: getuid and getgid only read the process's own ids.
    Ok(unsafe { (libc::getuid(), libc::getgid()) })
}

#[cfg(not(unix))]
fn own_ids() -> Result<(u32, u32)> {
    anyhow::bail!("a container is not available on Windows")
}

impl ClaudeProcess {
    pub fn spawn(opts: &SpawnOpts) -> Result<(Self, mpsc::UnboundedReceiver<ProcEvent>)> {
        let (program, argv) = match &opts.container {
            Some(spec) => {
                let names: Vec<String> = opts.env.set.iter().map(|(name, _)| name.clone()).filter(|name| name != "CLAUDE_CONFIG_DIR").collect();
                let (uid, gid) = own_ids()?;
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
            .kill_on_drop(true);
        crate::proc_group::configure(&mut command);
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
        let group = match child.id() {
            Some(pid) => match crate::proc_group::adopt(pid) {
                Some(group) => Some(Arc::new(group)),
                None => {
                    let _ = child.start_kill();
                    anyhow::bail!("could not put the program in a group it can be ended with");
                }
            },
            None => None,
        };
        let reader_group = group.clone();
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;

        let alive = Arc::new(AtomicBool::new(true));
        let reader_alive = alive.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let reader_stopped = stopped.clone();
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
                kill_group(reader_group.as_ref());
            }
            let code = wait_for(&mut child).await;
            let tail_text = tail.await.unwrap_or_default();
            let mut tail_text = tail_text;
            if too_long {
                tail_text.push_str("\n[host] a line longer than the limit ended the process\n");
            }
            let _ = event_tx.send(ProcEvent::Stderr(tail_text));
            // Ended by this host (`stop`): no code, whatever the system said (see `ClaudeProcess::stopped`).
            let _ = event_tx.send(ProcEvent::Exited(if reader_stopped.load(Ordering::SeqCst) { None } else { code }));
        });

        let container = opts.container.as_ref().map(|spec| (spec.runtime.program.clone(), spec.name.clone()));
        Ok((Self { stdin: line_tx, group, alive, stopped, container }, event_rx))
    }

    /// Queues one line for stdin; false when the process is gone.
    pub fn send_line(&self, line: String) -> bool {
        self.alive.load(Ordering::SeqCst) && self.stdin.send(line).is_ok()
    }

    /// SIGKILL to the whole group now, for a host that is exiting and cannot wait three seconds.
    pub fn kill_now(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(group) = &self.group {
            group.kill();
        }
        if let Some((program, name)) = &self.container {
            container::remove(program, name, Duration::ZERO);
        }
    }

    /// Ends the process and everything it started: SIGTERM to its group, SIGKILL after three seconds.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        kill_group(self.group.as_ref());
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
        kill_group(self.group.as_ref());
        if let Some((program, name)) = self.container.take() {
            // Started here and not waited for: the host may be ending, and the command goes on without it.
            container::remove(&program, &name, Duration::ZERO);
        }
    }
}

fn kill_group(group: Option<&Arc<crate::proc_group::Group>>) {
    let Some(group) = group else { return };
    group.term();
    let group = group.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        group.kill();
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
    if !real.is_dir() || real.parent().is_none() {
        return false;
    }
    match crate::platform::home_dir().and_then(|home| home.canonicalize().ok()) {
        Some(home) => real != home,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::perm::PermMode;

    #[test]
    fn one_settings_value_carries_the_provider_address_and_the_plugins_that_are_off() {
        let base = SpawnOpts { bin: "claude".into(), cwd: ".".into(), model: None, resume: None, mcp_config: None, system_file: None, plugin_dir: None, env: EnvPlan::default(), pinned_url: None, container: None, permission_mode: Default::default(), name: None, plugins_off: false, guard_hook: None, stop_hook: None, debug_file: None, agents: None, effort: None, user_settings_only: false };
        assert!(args(&base).iter().all(|a| a != "--settings"));
        let off = args(&SpawnOpts { plugins_off: true, ..base.clone() });
        let value: serde_json::Value = serde_json::from_str(&off[off.iter().position(|a| a == "--settings").unwrap() + 1]).unwrap();
        assert_eq!(value["enabledPlugins"][SWB_PLUGIN], false);
        assert!(off.iter().all(|a| a != "--setting-sources"), "the project's settings stay when no provider is pinned");
        // A decision to leave the project's configuration out gives the same argument, once, with or without a provider's address.
        let only = args(&SpawnOpts { user_settings_only: true, ..base.clone() });
        assert_eq!(only.iter().filter(|a| *a == "--setting-sources").count(), 1);
        assert!(only.windows(2).any(|w| w == ["--setting-sources", "user"]));
        let both_on = args(&SpawnOpts { user_settings_only: true, pinned_url: Some("https://gw.example.com/x".into()), ..base.clone() });
        assert_eq!(both_on.iter().filter(|a| *a == "--setting-sources").count(), 1);
        let both = args(&SpawnOpts { plugins_off: true, pinned_url: Some("https://gw.example.com/x".into()), ..base });
        assert_eq!(both.iter().filter(|a| *a == "--settings").count(), 1, "one --settings only");
        let value: serde_json::Value = serde_json::from_str(&both[both.iter().position(|a| a == "--settings").unwrap() + 1]).unwrap();
        assert_eq!((value["env"]["ANTHROPIC_BASE_URL"].as_str(), value["enabledPlugins"][SWB_PLUGIN].as_bool()), (Some("https://gw.example.com/x"), Some(false)));
        assert!(both.windows(2).any(|w| w == ["--setting-sources", "user"]));
    }

    #[test]
    fn the_arguments_include_the_permission_prompt_tool_and_resume() {
        let opts = SpawnOpts { bin: "claude".into(), cwd: ".".into(), model: Some("haiku".into()), resume: Some("abc".into()), mcp_config: Some("/d/run/x.mcp.json".into()), system_file: Some("/d/run/x.mem.txt".into()), plugin_dir: Some("/d/run/x.skills".into()), env: EnvPlan::default(), pinned_url: None, container: None, permission_mode: Default::default(), name: None, plugins_off: false, guard_hook: None, stop_hook: None, debug_file: None, agents: None, effort: None, user_settings_only: false };
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
        let opts = SpawnOpts { bin: "claude".into(), cwd: ".".into(), model: None, resume: None, mcp_config: None, system_file: None, plugin_dir: None, env, pinned_url: Some("https://gw.example.com/x".into()), container: None, permission_mode: Default::default(), name: None, plugins_off: false, guard_hook: None, stop_hook: None, debug_file: None, agents: None, effort: None, user_settings_only: false };
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

    #[test]
    fn a_name_a_person_gave_is_passed_as_the_clis_own_session_name() {
        let base = SpawnOpts { bin: "claude".into(), cwd: ".".into(), model: None, resume: None, mcp_config: None, system_file: None, plugin_dir: None, env: EnvPlan::default(), pinned_url: None, container: None, permission_mode: PermMode::Default, name: None, plugins_off: false, guard_hook: None, stop_hook: None, debug_file: None, agents: None, effort: None, user_settings_only: false };
        assert!(!args(&base).iter().any(|a| a == "--name"));
        assert!(args(&SpawnOpts { name: Some(String::new()), ..base.clone() }).iter().all(|a| a != "--name"), "a blank name is none");
        assert!(args(&SpawnOpts { name: Some("내 작업".into()), ..base }).windows(2).any(|w| w == ["--name", "내 작업"]));
    }

    #[test]
    fn a_chosen_permission_mode_is_passed_and_the_default_adds_nothing() {
        let base = SpawnOpts { bin: "claude".into(), cwd: ".".into(), model: None, resume: None, mcp_config: None, system_file: None, plugin_dir: None, env: EnvPlan::default(), pinned_url: None, container: None, permission_mode: PermMode::Default, name: None, plugins_off: false, guard_hook: None, stop_hook: None, debug_file: None, agents: None, effort: None, user_settings_only: false };
        assert!(!args(&base).iter().any(|x| x == "--permission-mode" || x == "--dangerously-skip-permissions"), "a chat that never chose has the arguments it always had");
        let accept = args(&SpawnOpts { permission_mode: PermMode::AcceptEdits, ..base.clone() });
        assert!(accept.windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
        let bypass = args(&SpawnOpts { permission_mode: PermMode::Bypass, ..base });
        assert!(bypass.windows(2).any(|w| w == ["--permission-mode", "bypassPermissions"]));
        assert!(!bypass.iter().any(|x| x == "--dangerously-skip-permissions" || x == "--allow-dangerously-skip-permissions"), "one way of saying it, and no mid-session switch is enabled");
    }

    #[test]
    fn the_stop_hook_is_set_beside_the_guard_hook_only_when_there_is_one() {
        let mut opts = SpawnOpts { bin: "claude".into(), cwd: ".".into(), model: None, resume: None, mcp_config: None, system_file: None, plugin_dir: None, env: EnvPlan::default(), pinned_url: None, container: None, permission_mode: Default::default(), name: None, plugins_off: false, guard_hook: Some("guard-cmd".into()), stop_hook: Some("stop-cmd || exit 0".into()), debug_file: None, agents: None, effort: None, user_settings_only: false };
        let settings = |opts: &SpawnOpts| -> serde_json::Value {
            let args = args(opts);
            let at = args.iter().position(|a| a == "--settings").expect("settings given");
            serde_json::from_str(&args[at + 1]).unwrap()
        };
        let both = settings(&opts);
        assert_eq!(both["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "guard-cmd");
        assert_eq!(both["hooks"]["Stop"][0]["hooks"][0]["command"], "stop-cmd || exit 0");
        assert_eq!(both["disableAllHooks"], false);
        opts.stop_hook = None;
        assert!(settings(&opts)["hooks"].get("Stop").is_none());
        opts.guard_hook = None;
        opts.stop_hook = Some("stop-cmd".into());
        assert!(!args(&opts).iter().any(|a| a == "--settings"), "a Stop hook alone is not set");
    }

    #[test]
    fn the_team_arguments_are_there_only_when_set() {
        let base = SpawnOpts { bin: "claude".into(), cwd: ".".into(), model: None, resume: None, mcp_config: None, system_file: None, plugin_dir: None, env: EnvPlan::default(), pinned_url: None, container: None, permission_mode: Default::default(), name: None, plugins_off: false, guard_hook: None, stop_hook: None, debug_file: None, agents: None, effort: None, user_settings_only: false };
        let plain = args(&base);
        assert!(!plain.iter().any(|a| a == "--agents" || a == "--effort"), "{plain:?}");
        let team = args(&SpawnOpts { agents: Some("{\"explorer\":{}}".into()), effort: Some("high".into()), ..base });
        let at = team.iter().position(|a| a == "--agents").expect("--agents");
        assert_eq!(team[at + 1], "{\"explorer\":{}}");
        let at = team.iter().position(|a| a == "--effort").expect("--effort");
        assert_eq!(team[at + 1], "high");
    }

    #[test]
    fn the_cli_debug_log_is_asked_for_only_when_a_file_is_given() {
        let mut opts = SpawnOpts { bin: "claude".into(), cwd: ".".into(), model: None, resume: None, mcp_config: None, system_file: None, plugin_dir: None, env: EnvPlan::default(), pinned_url: None, container: None, permission_mode: Default::default(), name: None, plugins_off: false, guard_hook: None, stop_hook: None, debug_file: None, agents: None, effort: None, user_settings_only: false };
        assert!(!args(&opts).iter().any(|a| a.contains("debug")), "an ordinary chat has no debug flag");
        opts.debug_file = Some(PathBuf::from("/d/run/x.debug.log"));
        let list = args(&opts);
        let at = list.iter().position(|a| a == "--debug-file").expect("the flag");
        assert_eq!(list[at + 1], "/d/run/x.debug.log");
    }
}
