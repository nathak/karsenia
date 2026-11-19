//! `tenrec guard --data <folder> --project <folder> --sid <chat>`: the PreToolUse hook the app gives each chat's `claude`. It reads the call
//! from stdin, classifies it (see `risk`), applies the chat's guard profile and pause switch and the project's rule file, and answers with
//! nothing (let Claude Code decide as it would), `ask` (Claude Code's own permission prompt, which this app shows) or `deny`.
//! It never answers `allow`: that would skip the person's own permission rules. Claude Code runs a call whose hook fails, so a failure
//! inside the guard is itself a `deny`, a hook that takes too long answers `deny` before Claude Code's own limit, and the hook's command
//! (see `hook_command`) turns any other way of failing into exit code 2, which blocks.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use super::risk::{classify, decide, effective_profile, Policy};
use super::rules::{self, apply_rule};
use super::store;

/// What the hook may take from stdin, and how long it may think (Claude Code's own limit for the hook is 10 s).
const MAX_INPUT: u64 = 1024 * 1024;
const THINK: Duration = Duration::from_secs(7);

/// Set by whoever prints the answer first: the watchdog and the decision must not both write one.
static ANSWERED: AtomicBool = AtomicBool::new(false);

/// What the hook tells Claude Code.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    /// No opinion: Claude Code goes on as it would.
    Silent,
    Ask(String),
    Deny(String),
}

impl Answer {
    /// The hook's stdout (empty for no opinion).
    pub fn to_json(&self) -> Option<String> {
        let (decision, reason) = match self {
            Answer::Silent => return None,
            Answer::Ask(reason) => ("ask", reason),
            Answer::Deny(reason) => ("deny", reason),
        };
        Some(json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": decision, "permissionDecisionReason": reason}}).to_string())
    }
}

/// The call as Claude Code gives it to a hook.
struct Call {
    tool: String,
    input: Value,
    cwd: PathBuf,
    /// The permission mode of the session (`bypassPermissions`, `dontAsk`, ...), when Claude Code says it.
    mode: String,
}

fn parse_call(text: &str) -> Result<Call, &'static str> {
    let v: Value = serde_json::from_str(text).map_err(|_| "the hook input is not JSON")?;
    let tool = v["tool_name"].as_str().filter(|t| !t.is_empty()).ok_or("the hook input names no tool")?.to_owned();
    Ok(Call { tool, input: v["tool_input"].clone(), cwd: PathBuf::from(v["cwd"].as_str().unwrap_or(".")), mode: v["permission_mode"].as_str().unwrap_or_default().to_owned() })
}

fn reason_text(verdict: &super::risk::Verdict) -> String {
    let why = if verdict.reason.is_empty() { "a call the guard looks at" } else { verdict.reason.as_str() };
    format!("SmartWorkbench guard ({}): {why}", verdict.category.name())
}

fn closed(why: &str) -> Answer {
    Answer::Deny(format!("SmartWorkbench guard failed closed: {why}"))
}

/// The same path with forward slashes and no Windows `\\?\` prefix, so a mention of it can be found whichever way it is written.
fn plain(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    text.strip_prefix("//?/").unwrap_or(&text).trim_end_matches('/').to_owned()
}

/// Whether the call names the app's data folder (where this chat's own workbench file lives): whatever the folder is called, a call that
/// mentions it is not let through, or the profile could be switched off by editing the file.
fn names_data_folder(input: &Value, data: &Path) -> bool {
    // The call as one text with forward slashes, in the case the file system compares (not on Linux, where Tenrec and tenrec differ).
    let fold = |text: String| if cfg!(any(windows, target_os = "macos")) { text.to_lowercase() } else { text };
    let text = fold(input.to_string().replace("\\\\", "/").replace('\\', "/"));
    // The folder as given and as the system resolves it (a link to it, a path with `..` in it, is the same folder), and the place of a chat's
    // workbench file inside any data folder, however the way to it is written (`~`, `$HOME`, a glob around it).
    let mut names = vec![plain(data)];
    if let Ok(real) = data.canonicalize() {
        names.push(plain(&real));
    }
    names.iter().map(|n| fold(n.clone())).any(|n| !n.is_empty() && n != "/" && text.contains(&n)) || text.contains("workbench/chat/")
}

/// The answer for one call of one chat. `data` is the app's data folder, `project` the chat's project folder (the rule file is looked for
/// there, not in the folder a shell command happens to have moved into).
pub fn answer(data: &Path, project: &Path, sid: &str, input_text: &str) -> Answer {
    if !data.is_dir() {
        return closed("the app's data folder is not there");
    }
    let call = match parse_call(input_text) {
        Ok(call) => call,
        Err(why) => return closed(why),
    };
    let wb = match store::load(data, sid) {
        Ok(wb) => wb,
        Err(_) => return closed("this chat's workbench file cannot be read"),
    };
    if names_data_folder(&call.input, data) {
        return Answer::Deny("SmartWorkbench guard (tenrec-data): a call may not name the app's data folder".into());
    }
    let ev = match evaluate(&wb, project, &call.cwd, &call.tool, &call.input) {
        Ok(ev) => ev,
        Err(why) => return closed(why),
    };
    let verdict = ev.verdict;
    match ev.policy {
        Policy::Allow => Answer::Silent,
        // In a mode where Claude Code does not put a question to the person, an ask would not be asked: it is a block.
        Policy::Ask if matches!(call.mode.as_str(), "bypassPermissions" | "dontAsk") => Answer::Deny(format!("{} (a question cannot be asked in {} mode)", reason_text(&verdict), call.mode)),
        Policy::Ask => Answer::Ask(reason_text(&verdict)),
        Policy::Block => Answer::Deny(reason_text(&verdict)),
    }
}

/// What the guard makes of one call: the verdict (with the project's rule applied), the profile in force and the policy it comes to.
pub struct Evaluation {
    pub verdict: super::risk::Verdict,
    pub profile: super::Profile,
    pub policy: Policy,
}

/// The decision part of `answer`, for whoever wants to know what the guard would say about a call (the live view shows it): the chat's
/// profile and pause switch, the project's rule file (looked for in `project`, read the way the editor reads it: a plain file, not too big;
/// a link, a pipe or a huge file put in its place is a sign of tampering, and a hook that waits on it would be let through when it times out)
/// and the call itself. The rule file's profile may only raise the chat's own. An Err is why the guard cannot decide (the hook blocks then).
pub fn evaluate(wb: &super::Workbench, project: &Path, cwd: &Path, tool: &str, input: &Value) -> Result<Evaluation, &'static str> {
    let (rule_list, file_profile) = match crate::view::guardfile::read_text(&crate::view::guardfile::path_of(project)) {
        Ok(Some(text)) => match rules::parse_guard_file(&text) {
            Ok(file) => (file.rules, file.profile),
            // A rule file that is not valid JSON would leave the project's block rules silently out: it is a block, until it is fixed.
            Err(_) => return Err("the project's rule file is not valid JSON; fix it (the Team page shows it)"),
        },
        Ok(None) => (Vec::new(), None),
        Err(_) => return Err("the project's rule file is not a plain file of a usual size"),
    };
    let profile = effective_profile(wb.profile, file_profile);
    let rule = rules::match_rule_in(&rule_list, tool, input, project.to_str().or(cwd.to_str()));
    let verdict = apply_rule(classify(tool, input), rule);
    let policy = decide(&verdict, profile, wb.paused);
    Ok(Evaluation { verdict, profile, policy })
}

/// Prints an answer and ends the process with 0.
fn finish(answer: &Answer) -> ! {
    if !ANSWERED.swap(true, Ordering::SeqCst) {
        if let Some(json) = answer.to_json() {
            println!("{json}");
        }
        std::process::exit(0);
    }
    // The watchdog has answered: nothing more is written.
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// `tenrec guard ...`: prints the answer and returns the exit code (0; 2 when it cannot decide, which denies the call).
pub fn run(args: &[String]) -> i32 {
    let value_of = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let (Some(data), Some(sid)) = (value_of("--data"), value_of("--sid")) else {
        eprintln!("SmartWorkbench guard failed closed: --data and --sid are required");
        return 2;
    };
    let project = value_of("--project");
    // A hook that has not answered in time answers `deny` itself: Claude Code would run the call when its own limit passes.
    std::thread::spawn(|| {
        std::thread::sleep(THINK);
        if !ANSWERED.swap(true, Ordering::SeqCst) {
            println!("{}", closed("it took too long to decide").to_json().unwrap_or_default());
            std::process::exit(0);
        }
    });
    let mut input = String::new();
    let mut stdin = std::io::stdin();
    let read = (&mut stdin).take(MAX_INPUT + 1).read_to_string(&mut input);
    if read.is_err() || input.len() as u64 > MAX_INPUT {
        // What is left is read and dropped (a bounded amount), so the writer is not cut off in the middle of its write.
        let _ = std::io::copy(&mut stdin.take(64 * 1024 * 1024), &mut std::io::sink());
        eprintln!("SmartWorkbench guard failed closed: the call could not be read, or is too big");
        return 2;
    }
    let project = project.map(PathBuf::from).unwrap_or_else(|| PathBuf::from(serde_json::from_str::<Value>(&input).ok().and_then(|v| v["cwd"].as_str().map(str::to_owned)).unwrap_or_else(|| ".".into())));
    // A panic inside the guard must not let the call through.
    let decided = std::panic::catch_unwind(|| answer(Path::new(&data), &project, &sid, &input));
    match decided {
        Ok(answer) => finish(&answer),
        Err(_) => {
            eprintln!("SmartWorkbench guard failed closed: internal error");
            2
        }
    }
}

/// The Stop hook of a chat's claude: `tenrec stop-gate --data <folder> --sid <chat>`. It holds the stop once when the person asked for
/// that and a done condition is unmet (see `stop_decision`). It can only ever let the chat go on: whatever fails (no data folder, an
/// unreadable file, bad input) it prints nothing and exits 0, because a Stop hook that errors must not keep a chat from ending its turn.
pub fn run_stop(args: &[String]) -> i32 {
    let value_of = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let (Some(data), Some(sid)) = (value_of("--data"), value_of("--sid")) else { return 0 };
    let mut input = String::new();
    if std::io::stdin().take(MAX_INPUT + 1).read_to_string(&mut input).is_err() || input.len() as u64 > MAX_INPUT {
        return 0;
    }
    let Ok(value) = serde_json::from_str::<Value>(&input) else { return 0 };
    let Ok(wb) = store::load(Path::new(&data), &sid) else { return 0 };
    if let Some(reason) = super::stop_decision(&value, &wb) {
        println!("{}", serde_json::json!({"decision": "block", "reason": reason}));
    }
    0
}
