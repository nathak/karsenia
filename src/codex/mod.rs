//! The Codex (OpenAI) side of an account of kind `Codex` (see `accounts::AccountKind`): its login lives in a folder of the account's own
//! (the program's `CODEX_HOME`), which this app only looks at through the program (`codex login status`, `codex logout`). The login itself
//! is made by the person in a terminal, as for a Claude account. No token is read, kept, logged or sent anywhere: of the program's own
//! words only a kind of answer is kept (`CodexStatus`), because the lines of an API-key login may carry a piece of the key.
//!
//! Chats do not run on Codex in this version; where `codex exec` will be started is here too.
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::accounts::{run_capped, Streams};
use crate::claude::env::{is_credential_var, EnvPlan};

/// How the person logged in, as far as the program says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexMethod {
    ChatGpt,
    ApiKey,
    Other,
}

impl CodexMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::ChatGpt => "chatgpt",
            Self::ApiKey => "api_key",
            Self::Other => "other",
        }
    }
}

/// What `codex login status` said, as a kind of answer and nothing of its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexStatus {
    LoggedIn(CodexMethod),
    LoggedOut,
    /// The program said something this app does not know how to read (another version, another language).
    Unknown,
}

/// Reads what the program printed (both of its outputs; the status line comes on stderr, measured with codex-cli 0.156.1). The first line
/// that says "Not logged in" or "Logged in ..." decides; a warning before it is skipped.
pub fn parse_status(bytes: &[u8]) -> CodexStatus {
    for line in String::from_utf8_lossy(bytes).lines() {
        let line = plain_line(line);
        if line.starts_with("not logged in") {
            return CodexStatus::LoggedOut;
        }
        if line.starts_with("logged in") {
            let method = if line.contains("chatgpt") {
                CodexMethod::ChatGpt
            } else if line.contains("api key") {
                CodexMethod::ApiKey
            } else {
                CodexMethod::Other
            };
            return CodexStatus::LoggedIn(method);
        }
    }
    CodexStatus::Unknown
}

/// A line without colour codes (`ESC [ ... letter`) and other control characters, trimmed and in lower case.
fn plain_line(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            // The sequence ends at its first letter or symbol of the final range (`@` to `~`).
            for end in chars.by_ref() {
                if ('@'..='~').contains(&end) {
                    break;
                }
            }
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out.trim().to_lowercase()
}

/// What the app tells a page about an account's login.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CodexAuth {
    pub logged_in: bool,
    pub method: Option<&'static str>,
    /// False when the program's answer could not be read: the person is sent to a terminal to look.
    pub known: bool,
    /// Whether the login file is in the account's folder (a plain file, not a link). A login that is not there lives somewhere else (the
    /// system's key store, a variable of the app's environment): it is not this account's own.
    pub file: bool,
}

impl CodexAuth {
    fn from(status: CodexStatus, file: bool) -> Self {
        match status {
            CodexStatus::LoggedIn(method) => Self { logged_in: true, method: Some(method.as_str()), known: true, file },
            CodexStatus::LoggedOut => Self { logged_in: false, method: None, known: true, file },
            CodexStatus::Unknown => Self { logged_in: false, method: None, known: false, file },
        }
    }
}

/// Whether a candidate for the `codex` program is one this build will run. On Windows an `.exe`, or the `.cmd` shim an npm install makes
/// (which goes through the command interpreter): the arguments given to this program are fixed words (`login status`, `logout`), and
/// the account's folder goes in as a variable, so no quoting of text from outside is involved. A `.ps1` shim is never run.
fn runnable(path: &Path) -> bool {
    if cfg!(windows) {
        path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("cmd"))
    } else {
        true
    }
}

/// The `codex` program: `TENREC_CODEX` when it is set, else the first one on `PATH` (looked for at each call, so installing it needs no
/// restart; on Windows `codex.exe` first, then the npm `codex.cmd`).
pub fn bin(path_var: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(chosen) = std::env::var_os("TENREC_CODEX").filter(|v| !v.is_empty()) {
        let chosen = PathBuf::from(chosen);
        return runnable(&chosen).then_some(chosen);
    }
    let path_var = path_var?;
    crate::platform::find_program(path_var, "codex").or_else(|| {
        if !cfg!(windows) {
            return None;
        }
        std::env::split_paths(path_var).map(|dir| dir.join("codex.cmd")).find(|p| p.is_file())
    })
}

/// Why `bin` found nothing, in words for the person.
pub fn not_found() -> String {
    if std::env::var_os("TENREC_CODEX").is_some_and(|v| !v.is_empty()) {
        return "TENREC_CODEX must name the codex program (on Windows an .exe or a .cmd file)".to_owned();
    }
    "the codex program was not found (it must be on PATH, or named in TENREC_CODEX)".to_owned()
}

/// Variables of this process that carry a credential or point `codex` somewhere else; a Codex account's run has none of them.
fn is_codex_credential_var(name: &str) -> bool {
    ["OPENAI_", "CODEX_", "AZURE_OPENAI_"].iter().any(|prefix| name.starts_with(prefix)) || is_credential_var(name)
}

/// The environment of a `codex` run for one account: its own folder as `CODEX_HOME`, none of the credential variables the app inherited
/// (so the person's other login or key cannot be used in its place), and everything else as it was (proxy, certificates, `PATH`).
pub fn plan(inherited: &[String], home: &Path) -> EnvPlan {
    let mut remove: Vec<String> = inherited.iter().filter(|name| is_codex_credential_var(name) && name.as_str() != "CODEX_HOME").cloned().collect();
    remove.sort();
    remove.dedup();
    EnvPlan { set: vec![("CODEX_HOME".into(), home.display().to_string())], remove }
}

/// Asks the program about the login in `home`. Its exit code is not looked at (a logged-out login ends with 1 and says so in words).
pub async fn status(bin: &str, home: &Path, env: &EnvPlan) -> Result<CodexAuth, String> {
    let (bytes, _) = run_capped(bin, home, env, &["login", "status"], Streams::Both).await?;
    let file = std::fs::symlink_metadata(home.join("auth.json")).map(|m| m.is_file()).unwrap_or(false);
    Ok(CodexAuth::from(parse_status(&bytes), file))
}

/// Best effort: asks the program to drop the login kept for this folder before the folder is removed.
pub async fn logout(bin: &str, home: &Path, env: &EnvPlan) -> Result<(), String> {
    let (_, status) = run_capped(bin, home, env, &["logout"], Streams::Both).await?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the program ended with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_program_prints_is_read_as_a_kind_of_answer_and_nothing_else() {
        assert_eq!(parse_status(b"Logged in using ChatGPT\n"), CodexStatus::LoggedIn(CodexMethod::ChatGpt));
        assert_eq!(parse_status(b"WARNING: proceeding, even though we could not create PATH aliases: x\nNot logged in\n"), CodexStatus::LoggedOut);
        assert_eq!(parse_status(b"WARNING: something\nLogged in using ChatGPT\n"), CodexStatus::LoggedIn(CodexMethod::ChatGpt));
        assert_eq!(parse_status(b"Logged in using an API key - sk-proj-***abcd\n"), CodexStatus::LoggedIn(CodexMethod::ApiKey));
        assert_eq!(parse_status(b"Logged in via something new\n"), CodexStatus::LoggedIn(CodexMethod::Other));
        assert_eq!(parse_status(b"  \x1b[0mlogged IN using chatgpt\x07 \n"), CodexStatus::LoggedIn(CodexMethod::ChatGpt));
        for odd in [&b""[..], b"\n\n", b"\xff\xfe", b"Something else entirely", b"WARNING only"] {
            assert_eq!(parse_status(odd), CodexStatus::Unknown, "{odd:?}");
        }
        // What is sent on keeps no word of the program's own, a key piece included.
        let auth = CodexAuth::from(parse_status(b"Logged in using an API key - sk-proj-***abcd"), true);
        let json = serde_json::to_string(&auth).unwrap();
        assert!(!json.contains("sk-") && !json.contains("abcd"), "{json}");
        assert_eq!(json, r#"{"logged_in":true,"method":"api_key","known":true,"file":true}"#);
        assert_eq!(serde_json::to_string(&CodexAuth::from(CodexStatus::Unknown, false)).unwrap(), r#"{"logged_in":false,"method":null,"known":false,"file":false}"#);
    }

    #[test]
    fn a_run_has_its_own_folder_and_loses_every_credential_but_nothing_else() {
        let names: Vec<String> = ["OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_API_KEY", "CODEX_HOME", "AZURE_OPENAI_ENDPOINT", "ANTHROPIC_API_KEY", "PATH", "HOME", "HTTPS_PROXY", "SSL_CERT_FILE", "LANG"].iter().map(|s| (*s).to_owned()).collect();
        let plan = plan(&names, Path::new("/data/accounts/ab"));
        assert_eq!(plan.set, vec![("CODEX_HOME".to_owned(), "/data/accounts/ab".to_owned())]);
        let mut gone = plan.remove.clone();
        gone.sort();
        assert_eq!(gone, vec!["ANTHROPIC_API_KEY", "AZURE_OPENAI_ENDPOINT", "CODEX_API_KEY", "OPENAI_API_KEY", "OPENAI_BASE_URL"]);
        assert!(!format!("{plan:?}").contains("/data/accounts"), "the debug text names variables, not values");
    }
}
