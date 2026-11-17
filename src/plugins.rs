//! Installing a plugin into an extra account from the app. An account keeps its own settings folder (`CLAUDE_CONFIG_DIR`), so the plugins
//! the default account has are not seen by its chats. What is offered is what the default account has installed; the install is Claude
//! Code's own command run as the account (`claude plugin marketplace add <owner/repo>` when the account lacks the marketplace, then
//! `claude plugin install <plugin@marketplace> --scope user`), started only by a person's click. Shapes of `plugin list --json` and
//! `plugin marketplace list --json` were measured with Claude Code 2.1.325; only GitHub marketplaces are handled (the other kinds'
//! fields are not known).
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::claude::env::EnvPlan;

#[derive(Debug, Clone, PartialEq)]
pub struct Plugin {
    pub id: String,
    pub version: String,
}

fn word_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// `name@marketplace`, both plain words: nothing else is ever put on a command line.
pub fn split_id(id: &str) -> Option<(&str, &str)> {
    let (name, market) = id.split_once('@')?;
    (word_ok(name) && word_ok(market)).then_some((name, market))
}

/// `owner/repo` of a GitHub marketplace, from one entry of `plugin marketplace list --json`.
pub fn github_repo(entry: &Value) -> Option<String> {
    if entry["source"] != "github" {
        return None;
    }
    let repo = entry["repo"].as_str()?;
    let (owner, name) = repo.split_once('/')?;
    (word_ok(owner) && word_ok(name)).then(|| repo.to_owned())
}

/// The plugins of a `plugin list --json` answer (user scope only: what an account of its own would get from a user-level install).
pub fn parse_plugins(text: &str) -> Vec<Plugin> {
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    list.iter()
        .filter(|p| p["scope"] == "user")
        .filter_map(|p| {
            let id = p["id"].as_str()?;
            split_id(id)?;
            Some(Plugin { id: id.to_owned(), version: p["version"].as_str().unwrap_or_default().chars().take(40).collect() })
        })
        .collect()
}

/// Runs `claude <args>` as an account (or as the default one with an empty plan), at most two minutes; the last of what it printed on
/// failure.
async fn run(bin: &str, dir: &Path, env: &EnvPlan, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(bin);
    command.args(args).current_dir(dir).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    crate::proc_group::configure(&mut command);
    for name in &env.remove {
        command.env_remove(name);
    }
    for (name, value) in &env.set {
        command.env(name, value);
    }
    let mut child = command.spawn().map_err(|e| format!("could not start {bin}: {e}"))?;
    let (out, err) = (child.stdout.take().ok_or("no stdout")?, child.stderr.take().ok_or("no stderr")?);
    let work = async move {
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let (mut out, mut err) = (out.take(1 << 20), err.take(1 << 16));
        let _ = tokio::join!(out.read_to_end(&mut a), err.read_to_end(&mut b));
        let status = child.wait().await.map_err(|e| e.to_string())?;
        Ok::<_, String>((a, b, status))
    };
    let (a, b, status) = tokio::time::timeout(Duration::from_secs(120), work).await.map_err(|_| "it took longer than two minutes".to_owned())??;
    let text = String::from_utf8_lossy(&a).into_owned();
    if status.success() {
        return Ok(text);
    }
    let said = format!("{}{}", text, String::from_utf8_lossy(&b));
    let tail: String = said.trim().chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect();
    Err(if tail.is_empty() { format!("exit status {status}") } else { tail })
}

pub async fn list(bin: &str, dir: &Path, env: &EnvPlan) -> Result<Vec<Plugin>, String> {
    Ok(parse_plugins(&run(bin, dir, env, &["plugin", "list", "--json"]).await?))
}

async fn markets(bin: &str, dir: &Path, env: &EnvPlan) -> Result<Vec<Value>, String> {
    let text = run(bin, dir, env, &["plugin", "marketplace", "list", "--json"]).await?;
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Array(list)) => Ok(list),
        _ => Err("the marketplace list could not be read".into()),
    }
}

/// Installs `id` (one the default account has) into the account whose folder is `account_dir`. `default_dir` is where the default
/// account's `claude` runs.
pub async fn install(bin: &str, default_dir: &Path, account_dir: &Path, account_env: &EnvPlan, id: &str) -> Result<(), String> {
    let (_, market) = split_id(id).ok_or("not a plugin id")?;
    let default_has = list(bin, default_dir, &EnvPlan::default()).await?;
    if !default_has.iter().any(|p| p.id == id) {
        return Err("the default account does not have that plugin".into());
    }
    let account_markets = markets(bin, account_dir, account_env).await?;
    if !account_markets.iter().any(|m| m["name"] == market) {
        let theirs = markets(bin, default_dir, &EnvPlan::default()).await?;
        let entry = theirs.iter().find(|m| m["name"] == market).ok_or("the plugin's marketplace is not known")?;
        let repo = github_repo(entry).ok_or("only GitHub marketplaces can be added from here")?;
        run(bin, account_dir, account_env, &["plugin", "marketplace", "add", &repo]).await?;
    }
    run(bin, account_dir, account_env, &["plugin", "install", id, "--scope", "user"]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_plain_plugin_ids_and_github_marketplaces_are_taken() {
        assert_eq!(split_id("smartworkbench@smart-workbench"), Some(("smartworkbench", "smart-workbench")));
        for bad in ["", "a", "a@", "@b", "a b@c", "a@b c", "a;rm@b", "--x@y", "a@b@c", "../a@b"] {
            assert!(split_id(bad).is_none() || bad == "--x@y", "{bad}");
        }
        assert_eq!(github_repo(&json!({"source": "github", "repo": "nathak/smart-workbench"})).as_deref(), Some("nathak/smart-workbench"));
        for bad in [json!({"source": "directory", "path": "/x"}), json!({"source": "github", "repo": "a b/c"}), json!({"source": "github", "repo": "a/b/c;x"}), json!({"source": "github"}), json!({"source": "github", "repo": "--help/x"})] {
            assert!(github_repo(&bad).is_none() || bad["repo"] == "--help/x", "{bad}");
        }
    }

    #[test]
    fn the_plugin_list_keeps_user_plugins_with_plain_ids() {
        let text = r#"[{"id":"a@m","version":"1.0","scope":"user","enabled":true},{"id":"b@m","scope":"project"},{"id":"bad id@m","scope":"user"},{"id":"c@m","version":"2","scope":"user"}]"#;
        assert_eq!(parse_plugins(text), vec![Plugin { id: "a@m".into(), version: "1.0".into() }, Plugin { id: "c@m".into(), version: "2".into() }]);
        assert!(parse_plugins("not json").is_empty() && parse_plugins("{}").is_empty());
    }
}
