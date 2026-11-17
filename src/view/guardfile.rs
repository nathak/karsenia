//! The project's SmartWorkbench guard file, `<project>/.claude/smartworkbench.json`: the rules (`tool` and `command` are patterns, `path`
//! a glob, `policy` allow, ask or block, first match wins), shown and edited from the Team page. Only the `rules` array is touched:
//! every other key (profile, templates, audit, ...) is kept as it is. The plug-in documents the format; it reads the file again at
//! its next prompt. A file that is not valid JSON is never rewritten, a rule is only appended at the end (so it never shadows an
//! earlier one), and a change is refused when the file is not what the person was shown.
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

pub const REL: &str = ".claude/smartworkbench.json";
const MAX_BYTES: u64 = 256 * 1024;
pub const MAX_RULES: usize = 200;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Rule {
    /// `allow`, `ask` or `block`; `?` when the entry is not a rule the plug-in understands.
    pub policy: String,
    pub tool: String,
    pub command: String,
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GuardFile {
    pub exists: bool,
    pub profile: Option<String>,
    /// Every entry of `rules`, in order (the index is the position in the file).
    pub rules: Vec<Rule>,
    /// The file's text as read (None: there is no file): a change quotes it.
    pub text: Option<String>,
}

pub fn path_of(root: &Path) -> PathBuf {
    root.join(".claude").join("smartworkbench.json")
}

fn cut(v: &Value, cap: usize) -> String {
    v.as_str().unwrap_or_default().chars().filter(|c| !c.is_control()).take(cap).collect()
}

pub fn read_text(file: &Path) -> Result<Option<String>, &'static str> {
    match std::fs::symlink_metadata(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("gf_err_read"),
        Ok(m) if !m.is_file() => Err("gf_err_file"),
        Ok(m) if m.len() > MAX_BYTES => Err("gf_err_big"),
        Ok(_) => std::fs::read_to_string(file).map(Some).map_err(|_| "gf_err_read"),
    }
}

/// An Err is an i18n key.
pub fn read(root: &Path) -> Result<GuardFile, &'static str> {
    let Some(text) = read_text(&path_of(root))? else { return Ok(GuardFile { exists: false, profile: None, rules: Vec::new(), text: None }) };
    let v: Value = serde_json::from_str(&text).map_err(|_| "gf_err_json")?;
    let o = v.as_object().ok_or("gf_err_json")?;
    let rules = match o.get("rules") {
        None => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .take(MAX_RULES)
            .map(|r| match r.as_object() {
                Some(r) => Rule {
                    policy: r.get("policy").and_then(Value::as_str).filter(|p| ["allow", "ask", "block"].contains(p)).unwrap_or("?").to_owned(),
                    tool: cut(r.get("tool").unwrap_or(&Value::Null), 200),
                    command: cut(r.get("command").unwrap_or(&Value::Null), 400),
                    path: cut(r.get("path").unwrap_or(&Value::Null), 300),
                    reason: cut(r.get("reason").unwrap_or(&Value::Null), 200),
                },
                None => Rule { policy: "?".into(), ..Rule::default() },
            })
            .collect(),
        Some(_) => return Err("gf_err_rules"),
    };
    Ok(GuardFile { exists: true, profile: o.get("profile").and_then(Value::as_str).map(|p| cut(&Value::String(p.to_owned()), 20)), rules, text: Some(text) })
}

/// Checks a rule the person wrote. An Err is an i18n key.
pub fn check(rule: &Rule) -> Result<(), &'static str> {
    if !["allow", "ask", "block"].contains(&rule.policy.as_str()) {
        return Err("gf_err_policy");
    }
    if rule.tool.trim().is_empty() {
        return Err("gf_err_tool");
    }
    for (text, cap) in [(&rule.tool, 200), (&rule.command, 400), (&rule.path, 300), (&rule.reason, 200)] {
        if text.chars().count() > cap || text.chars().any(char::is_control) {
            return Err("gf_err_text");
        }
    }
    for pattern in [&rule.tool, &rule.command] {
        if !pattern.trim().is_empty() && regex::Regex::new(pattern).is_err() {
            return Err("gf_err_pattern");
        }
    }
    // An allow rule that names neither a command nor a path would let every call of the tool through.
    if rule.policy == "allow" && rule.command.trim().is_empty() && rule.path.trim().is_empty() {
        return Err("gf_err_broad");
    }
    Ok(())
}

fn write_atomic(root: &Path, object: &Map<String, Value>) -> Result<(), &'static str> {
    use std::io::Write;
    let file = path_of(root);
    let folder = file.parent().ok_or("gf_err_write")?;
    // The project is not trusted: a link in place of `.claude` would send the write somewhere else.
    match std::fs::symlink_metadata(folder) {
        Ok(m) if !m.is_dir() => return Err("gf_err_file"),
        Ok(_) => {}
        Err(_) => std::fs::create_dir_all(folder).map_err(|_| "gf_err_write")?,
    }
    if !std::fs::symlink_metadata(folder).map(|m| m.is_dir()).unwrap_or(false) {
        return Err("gf_err_file");
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(object.clone())).map_err(|_| "gf_err_write")?;
    text.push('\n');
    // A name nobody can guess, made only if it does not exist (a planted link or file of the same name is never written through).
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = folder.join(format!(".smartworkbench.{:x}-{:x}.tmp", std::process::id(), nonce));
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|_| "gf_err_write")?;
    if f.write_all(text.as_bytes()).and_then(|()| f.flush()).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return Err("gf_err_write");
    }
    drop(f);
    std::fs::rename(&tmp, &file).map_err(|_| {
        let _ = std::fs::remove_file(&tmp);
        "gf_err_write"
    })
}

/// The file as an object to change, after checking it is still the text the person saw (`seen`: None when there was no file).
fn load(root: &Path, seen: Option<&str>) -> Result<Map<String, Value>, &'static str> {
    let now = read_text(&path_of(root))?;
    if now.as_deref() != seen {
        return Err("gf_err_changed");
    }
    match now {
        None => Ok(Map::new()),
        Some(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(o)) => Ok(o),
            _ => Err("gf_err_json"),
        },
    }
}

/// Appends `rule` at the end of the rules.
pub fn add(root: &Path, seen: Option<&str>, rule: &Rule) -> Result<(), &'static str> {
    check(rule)?;
    let mut o = load(root, seen)?;
    let rules = o.entry("rules").or_insert_with(|| json!([]));
    let rules = rules.as_array_mut().ok_or("gf_err_rules")?;
    if rules.len() >= MAX_RULES {
        return Err("gf_err_many");
    }
    let mut entry = Map::new();
    entry.insert("tool".into(), json!(rule.tool.trim()));
    if !rule.command.trim().is_empty() {
        entry.insert("command".into(), json!(rule.command.trim()));
    }
    if !rule.path.trim().is_empty() {
        entry.insert("path".into(), json!(rule.path.trim()));
    }
    entry.insert("policy".into(), json!(rule.policy));
    if !rule.reason.trim().is_empty() {
        entry.insert("reason".into(), json!(rule.reason.trim()));
    }
    rules.push(Value::Object(entry));
    write_atomic(root, &o)
}

/// Removes the rule at `index`.
pub fn remove(root: &Path, seen: Option<&str>, index: usize) -> Result<(), &'static str> {
    let mut o = load(root, seen)?;
    let rules = o.get_mut("rules").and_then(Value::as_array_mut).ok_or("gf_err_rules")?;
    if index >= rules.len() {
        return Err("gf_err_changed");
    }
    rules.remove(index);
    write_atomic(root, &o)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(policy: &str, tool: &str, command: &str) -> Rule {
        Rule { policy: policy.into(), tool: tool.into(), command: command.into(), path: String::new(), reason: "because".into() }
    }

    #[test]
    fn a_rule_is_added_at_the_end_and_everything_else_in_the_file_stays() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(path_of(root), r#"{"profile":"strict","rules":[{"tool":"Bash","command":"x","policy":"block"}],"audit":true,"templates":{"a":{}}}"#).unwrap();
        let before = read(root).unwrap();
        assert_eq!((before.profile.as_deref(), before.rules.len()), (Some("strict"), 1));
        add(root, before.text.as_deref(), &rule("allow", "Bash", r"^git status$")).unwrap();
        let after = read(root).unwrap();
        assert_eq!(after.rules.len(), 2);
        assert_eq!(after.rules[1], Rule { policy: "allow".into(), tool: "Bash".into(), command: "^git status$".into(), path: String::new(), reason: "because".into() });
        let raw: Value = serde_json::from_str(after.text.as_deref().unwrap()).unwrap();
        assert_eq!(raw["audit"], true);
        assert_eq!(raw["profile"], "strict");
        assert!(raw["templates"]["a"].is_object());
        remove(root, after.text.as_deref(), 1).unwrap();
        assert_eq!(read(root).unwrap().rules.len(), 1);
    }

    #[test]
    fn there_may_be_no_file_and_one_is_made_with_the_first_rule() {
        let dir = tempfile::tempdir().unwrap();
        let none = read(dir.path()).unwrap();
        assert!(!none.exists && none.rules.is_empty());
        add(dir.path(), None, &rule("ask", "Edit|Write", "")).unwrap();
        assert_eq!(read(dir.path()).unwrap().rules.len(), 1);
    }

    #[test]
    fn a_change_is_refused_when_the_file_is_not_what_was_shown_or_is_not_json() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(path_of(root), r#"{"rules":[]}"#).unwrap();
        let seen = read(root).unwrap().text;
        std::fs::write(path_of(root), r#"{"rules":[{"tool":"Bash","policy":"block"}]}"#).unwrap();
        assert_eq!(add(root, seen.as_deref(), &rule("ask", "Bash", "")), Err("gf_err_changed"));
        std::fs::write(path_of(root), "{ not json").unwrap();
        assert_eq!(read(root), Err("gf_err_json"));
        let text = std::fs::read_to_string(path_of(root)).unwrap();
        assert_eq!(add(root, Some(&text), &rule("ask", "Bash", "")), Err("gf_err_json"));
        assert_eq!(std::fs::read_to_string(path_of(root)).unwrap(), "{ not json", "a broken file is never rewritten");
    }

    #[test]
    fn rules_are_checked() {
        assert_eq!(check(&rule("allow", "Bash", "")), Err("gf_err_broad"), "an allow rule must narrow the calls");
        assert_eq!(check(&rule("maybe", "Bash", "x")), Err("gf_err_policy"));
        assert_eq!(check(&rule("ask", " ", "x")), Err("gf_err_tool"));
        assert_eq!(check(&rule("ask", "Bash", "(")), Err("gf_err_pattern"));
        assert!(check(&rule("block", "Bash", "")).is_ok(), "a block rule may be broad");
        assert!(check(&Rule { policy: "allow".into(), tool: "Edit".into(), path: "docs/**".into(), ..Rule::default() }).is_ok());
    }

    #[test]
    fn a_link_in_place_of_the_folder_or_the_file_is_never_written_through() {
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            let victim = dir.path().join("victim");
            std::fs::create_dir_all(&victim).unwrap();
            let project = dir.path().join("p");
            std::fs::create_dir_all(&project).unwrap();
            std::os::unix::fs::symlink(&victim, project.join(".claude")).unwrap();
            assert!(add(&project, None, &rule("ask", "Bash", "")).is_err());
            assert!(std::fs::read_dir(&victim).unwrap().next().is_none(), "nothing was written through the link");
        }
    }

    #[test]
    fn a_link_in_place_of_the_file_is_not_followed() {
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
            std::fs::write(dir.path().join("real.json"), "{}").unwrap();
            std::os::unix::fs::symlink(dir.path().join("real.json"), path_of(dir.path())).unwrap();
            assert_eq!(read(dir.path()), Err("gf_err_file"));
        }
    }
}
