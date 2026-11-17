//! The project guard file `.claude/smartworkbench.json`: a list of rules a project checks in, which can tighten the built-in guard
//! (see `risk`) but never loosen what it blocks or what it always asks. A port of the SmartWorkbench plug-in's `hooks/policy.ts`.
//!
//! The file is checked in and read by anyone: it is parsed into typed values here, a broken rule is reported and skipped, never guessed at.
//! A rule is `{ "tool": regex, "command": regex, "path": glob, "policy": "allow"|"ask"|"block", "reason": text }`, at least one of the
//! first three; the rules are checked top to bottom and the first match applies. `tool` is matched against the whole tool name, `command`
//! anywhere in a Bash command, and `path` (`**` crosses folders, `*` and `?` stay inside one) against the path of the call.
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use fancy_regex::{Regex, RegexBuilder};
use serde_json::Value;

use super::risk::{normalise_path, Category, Policy, Risk, Verdict};
use super::Profile;

pub const POLICY_PATH: &str = ".claude/smartworkbench.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// A regular expression, matched against the whole tool name.
    pub tool: Option<String>,
    /// A regular expression, matched anywhere in the command of a Bash call.
    pub command: Option<String>,
    /// A glob, matched against the path of the call (`file_path`, `notebook_path` or `path`).
    pub path: Option<String>,
    pub policy: Policy,
    pub reason: Option<String>,
}

/// What a guard file holds: its own profile, the rules that could be read, and what was wrong with the rest.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GuardFile {
    /// The profile the file asks for (`permissive`, `balanced` or `strict`); it can only tighten the person's own (see `risk::effective_profile`).
    pub profile: Option<Profile>,
    pub rules: Vec<Rule>,
    /// The place of each rule of `rules` in the file's own list (a broken rule before it leaves a gap).
    pub rule_at: Vec<usize>,
    pub warnings: Vec<String>,
}

/// Regular expressions are run with a bounded backtrack budget; a rule that cannot be run to the end counts as a match unless it allows.
const BACKTRACK_LIMIT: usize = 400_000;
/// Compiled patterns are kept by their text; past this many the cache starts over.
const CACHE_LIMIT: usize = 512;

static CACHE: LazyLock<Mutex<HashMap<String, Option<Arc<Regex>>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn compile(source: &str) -> Option<Regex> {
    RegexBuilder::new(source).backtrack_limit(BACKTRACK_LIMIT).build().ok()
}

fn cached(source: &str) -> Option<Arc<Regex>> {
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(found) = cache.get(source) {
        return found.clone();
    }
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    let built = compile(source).map(Arc::new);
    cache.insert(source.to_string(), built.clone());
    built
}

fn tool_source(tool: &str) -> String {
    format!("^(?:{tool})$")
}

/// "src/**/*.ts", "migrations/**", "*.lock": ** crosses folders, * and ? stay inside one. As the source of a regular expression.
fn glob_source(glob: &str) -> String {
    let chars: Vec<char> = glob.chars().collect();
    let mut out = String::from("^");
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let next = chars.get(i + 1).copied();
        match c {
            '*' if next == Some('*') => {
                if chars.get(i + 2).copied() == Some('/') {
                    out.push_str("(?:(?s:.*)/)?");
                    i += 3;
                } else {
                    out.push_str("(?s:.*)");
                    i += 2;
                }
                continue;
            }
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            '.' | '+' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            other => out.push(other),
        }
        i += 1;
    }
    out.push('$');
    out
}

/// Whether `glob` matches `path` as a whole.
pub fn glob_matches(glob: &str, path: &str) -> bool {
    cached(&glob_source(glob)).is_some_and(|re| re.is_match(path).unwrap_or(false))
}

fn policy_rank(policy: Policy) -> u8 {
    match policy {
        Policy::Allow => 0,
        Policy::Ask => 1,
        Policy::Block => 2,
    }
}

fn optional_text(map: &serde_json::Map<String, Value>, key: &str) -> Result<Option<String>, ()> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(()),
    }
}

/// One rule of the file, or what is wrong with it.
fn parse_rule(item: &Value, number: usize) -> Result<Rule, String> {
    let shape = || format!("rule {number}: needs \"policy\" (allow|ask|block) and string fields");
    let map = item.as_object().ok_or_else(shape)?;
    let policy = map.get("policy").and_then(Value::as_str).and_then(Policy::parse).ok_or_else(shape)?;
    let tool = optional_text(map, "tool").map_err(|()| shape())?;
    let command = optional_text(map, "command").map_err(|()| shape())?;
    let path = optional_text(map, "path").map_err(|()| shape())?;
    let reason = optional_text(map, "reason").map_err(|()| shape())?;
    if tool.is_none() && command.is_none() && path.is_none() {
        return Err(format!("rule {number}: needs at least one of \"tool\", \"command\", \"path\""));
    }
    if tool.as_deref().is_some_and(|source| compile(&tool_source(source)).is_none()) {
        return Err(format!("rule {number}: \"tool\" is not a valid regular expression"));
    }
    if command.as_deref().is_some_and(|source| compile(source).is_none()) {
        return Err(format!("rule {number}: \"command\" is not a valid regular expression"));
    }
    Ok(Rule { tool, command, path, policy, reason })
}

/// Reads the checked-in file. Text that is not JSON, or not an object, is an error; a broken profile or rule is a warning and is left out.
pub fn parse_guard_file(text: &str) -> Result<GuardFile, String> {
    let raw: Value = serde_json::from_str(text).map_err(|error| format!("not valid JSON: {error}"))?;
    let Some(map) = raw.as_object() else { return Err("expected an object with \"profile\" and \"rules\"".to_string()) };
    let mut file = GuardFile::default();

    match map.get("profile") {
        None => {}
        Some(Value::String(word)) if matches!(word.as_str(), "permissive" | "balanced" | "strict") => file.profile = Profile::parse(word),
        Some(_) => file.warnings.push("profile must be one of permissive, balanced, strict".to_string()),
    }

    match map.get("rules") {
        None => {}
        Some(Value::Array(items)) => {
            for (at, item) in items.iter().enumerate() {
                match parse_rule(item, at + 1) {
                    Ok(rule) => {
                        file.rules.push(rule);
                        file.rule_at.push(at);
                    }
                    Err(warning) => file.warnings.push(warning),
                }
            }
        }
        Some(_) => file.warnings.push("rules must be a list".to_string()),
    }
    Ok(file)
}

/// The rules of the file and what was wrong with the rest (see `parse_guard_file`, which also gives the file's profile).
pub fn parse_rules(file_text: &str) -> Result<(Vec<Rule>, Vec<String>), String> {
    let file = parse_guard_file(file_text)?;
    Ok((file.rules, file.warnings))
}

/// The path inside the repository once `.` and `..` are folded, or `None` for a path outside it.
pub fn repo_path(path: &str, cwd: &str) -> Option<String> {
    if is_drive_path(path) {
        return None;
    }
    let full = normalise_path(&if path.starts_with('/') { path.to_string() } else { format!("{cwd}/{path}") });
    let root = normalise_path(cwd);
    let prefix = if root == "/" { root } else { format!("{root}/") };
    full.strip_prefix(prefix.as_str()).map(str::to_string)
}

fn is_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// A path outside the repository still has its dots folded, so a rule on /etc/** is not dodged by /repo/../etc/hosts.
fn relative(path: &str, cwd: Option<&str>) -> String {
    if is_drive_path(path) {
        return normalise_path(path);
    }
    let Some(cwd) = cwd else { return normalise_path(path) };
    repo_path(path, cwd).unwrap_or_else(|| normalise_path(&if path.starts_with('/') { path.to_string() } else { format!("{cwd}/{path}") }))
}

/// Whether `source` is found in `text`; a pattern that cannot be compiled or run to the end counts as found unless the rule allows.
fn found(rule: &Rule, source: &str, text: &str) -> bool {
    let unsure = rule.policy != Policy::Allow;
    match cached(source) {
        Some(re) => re.is_match(text).unwrap_or(unsure),
        None => unsure,
    }
}

fn rule_matches(rule: &Rule, tool: &str, command: Option<&str>, path: Option<&str>) -> bool {
    if let Some(source) = &rule.tool {
        if !found(rule, &tool_source(source), tool) {
            return false;
        }
    }
    if let Some(source) = &rule.command {
        if !command.is_some_and(|command| found(rule, source, command)) {
            return false;
        }
    }
    if let Some(glob) = &rule.path {
        if !path.is_some_and(|path| found(rule, &glob_source(glob), path)) {
            return false;
        }
    }
    true
}

/// The first rule that matches the call; `tool` and `command` are regular expressions, `path` is a glob. A path is read as it is
/// written (dots folded); use `match_rule_in` to read a relative path from the project folder.
pub fn match_rule<'a>(rules: &'a [Rule], tool: &str, input: &Value) -> Option<&'a Rule> {
    match_rule_in(rules, tool, input, None)
}

/// `match_rule` for a project at `cwd`: a path inside it is read relative to it (so `migrations/**` matches `<cwd>/migrations/001.sql`).
pub fn match_rule_in<'a>(rules: &'a [Rule], tool: &str, input: &Value, cwd: Option<&str>) -> Option<&'a Rule> {
    let command = input.get("command").and_then(Value::as_str);
    let raw_path = ["file_path", "notebook_path", "path"].iter().filter_map(|key| input.get(key)).find(|value| !value.is_null());
    let path = raw_path.and_then(Value::as_str).map(|raw| relative(raw, cwd));
    rules.iter().find(|rule| rule_matches(rule, tool, command, path.as_deref()))
}

/// A project rule may tighten anything, but cannot loosen a built-in block or a protected check.
pub fn apply_rule(verdict: Verdict, rule: Option<&Rule>) -> Verdict {
    let Some(rule) = rule else { return verdict };
    let loosens = policy_rank(rule.policy) < policy_rank(verdict.policy);
    if loosens && (verdict.policy == Policy::Block || verdict.category.always_ask()) {
        return Verdict { rule: Some(rule.policy), ..verdict };
    }
    let risk = match (rule.policy, verdict.risk) {
        (Policy::Block, _) => Risk::High,
        (Policy::Ask, Risk::Low) => Risk::Medium,
        (_, risk) => risk,
    };
    // A rule that asks about what is already asked, as protected checks are, keeps the built-in category, so the permissive
    // profile and "Allow for session" still treat it as the check it is.
    if rule.policy == verdict.policy && verdict.category.always_ask() {
        let reason = rule.reason.clone().unwrap_or(verdict.reason.clone());
        return Verdict { reason, rule: Some(rule.policy), ..verdict };
    }
    let reason = match (&rule.reason, rule.policy) {
        (Some(reason), _) => reason.clone(),
        (None, Policy::Allow) => "allowed by project rule".to_string(),
        (None, _) => format!("project rule ({POLICY_PATH})"),
    };
    Verdict { category: Category::ProjectRule, risk, policy: rule.policy, reason, rule: Some(rule.policy) }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::risk::{classify, decide};
    use super::*;

    fn rule(tool: Option<&str>, command: Option<&str>, path: Option<&str>, policy: Policy) -> Rule {
        Rule { tool: tool.map(str::to_string), command: command.map(str::to_string), path: path.map(str::to_string), policy, reason: None }
    }

    const EXAMPLE: &str = r#"{
      "profile": "strict",
      "rules": [
        { "tool": "Bash", "command": "\\bterraform\\s+apply\\b", "policy": "block", "reason": "infra changes go through CI" },
        { "tool": "Edit|Write", "path": "migrations/**", "policy": "ask", "reason": "schema migrations need review" }
      ],
      "defaultTemplate": "change",
      "audit": true
    }"#;

    #[test]
    fn the_example_of_the_readme_is_read() {
        let file = parse_guard_file(EXAMPLE).expect("a file");
        assert_eq!(file.profile, Some(Profile::Strict));
        assert_eq!(file.warnings, Vec::<String>::new());
        assert_eq!(file.rule_at, vec![0, 1]);
        assert_eq!(file.rules[0], Rule { tool: Some("Bash".into()), command: Some("\\bterraform\\s+apply\\b".into()), path: None, policy: Policy::Block, reason: Some("infra changes go through CI".into()) });
        let (rules, warnings) = parse_rules(EXAMPLE).expect("rules");
        assert_eq!((rules.len(), warnings.len()), (2, 0));
        let hit = |tool: &str, input: Value| match_rule(&rules, tool, &input).map(|one| one.policy);
        assert_eq!(hit("Bash", json!({ "command": "terraform apply -auto-approve" })), Some(Policy::Block));
        assert_eq!(hit("Bash", json!({ "command": "terraform plan" })), None);
        assert_eq!(hit("Edit", json!({ "file_path": "migrations/2024/001.sql" })), Some(Policy::Ask));
        assert_eq!(hit("Write", json!({ "file_path": "migrations/001.sql" })), Some(Policy::Ask));
        assert_eq!(hit("Read", json!({ "file_path": "migrations/001.sql" })), None);
        // The tool is matched as a whole: "Edit|Write" is not found inside "MultiEdit".
        assert_eq!(hit("MultiEdit", json!({ "file_path": "migrations/001.sql" })), None);
        assert_eq!(hit("Edit", json!({ "file_path": "src/a.sql" })), None);
    }

    #[test]
    fn broken_rules_are_reported_and_skipped_never_guessed_at() {
        let text = json!({
            "profile": "loud",
            "rules": [
                { "policy": "ask" },
                { "tool": "Bash", "command": "a", "policy": "block" },
                { "tool": "Bash", "command": "(", "policy": "block" },
                { "tool": "Bash(", "policy": "ask" },
                { "tool": "Bash", "policy": "maybe" },
                { "tool": 3, "policy": "ask" },
                { "tool": "Bash", "reason": null, "policy": "ask" },
                "x",
                null,
                { "tool": "Bash", "command": "b", "policy": "allow" }
            ]
        })
        .to_string();
        let file = parse_guard_file(&text).expect("a file");
        assert_eq!(file.profile, None);
        // The first rule is broken (nothing to match), so the parsed list starts at the second.
        assert_eq!(file.rule_at, vec![1, 9]);
        assert_eq!(file.rules.iter().map(|one| one.command.clone()).collect::<Vec<_>>(), vec![Some("a".to_string()), Some("b".to_string())]);
        assert_eq!(file.warnings.len(), 9, "{:?}", file.warnings);
        assert!(file.warnings.iter().any(|one| one.starts_with("profile must be")));
        assert!(file.warnings.contains(&"rule 1: needs at least one of \"tool\", \"command\", \"path\"".to_string()));
        assert!(file.warnings.contains(&"rule 3: \"command\" is not a valid regular expression".to_string()));
        assert!(file.warnings.contains(&"rule 4: \"tool\" is not a valid regular expression".to_string()));
        assert!(file.warnings.contains(&"rule 5: needs \"policy\" (allow|ask|block) and string fields".to_string()));
    }

    #[test]
    fn a_file_that_is_not_an_object_is_an_error_and_a_rules_value_that_is_not_a_list_a_warning() {
        for text in ["", "{ not json", "[]", "\"x\"", "3", "null"] {
            assert!(parse_guard_file(text).is_err(), "{text:?}");
            assert!(parse_rules(text).is_err(), "{text:?}");
        }
        let file = parse_guard_file("{\"rules\": 3}").expect("a file");
        assert!(file.rules.is_empty());
        assert_eq!(file.warnings, vec!["rules must be a list".to_string()]);
        let none = parse_guard_file("{}").expect("a file");
        assert_eq!(none, GuardFile::default());
        assert_eq!(parse_guard_file("{\"profile\":\"off\"}").expect("a file").profile, None);
        assert_eq!(parse_guard_file("{\"profile\":\"balanced\"}").expect("a file").profile, Some(Profile::Balanced));
    }

    #[test]
    fn the_first_matching_rule_wins() {
        let rules = vec![rule(Some("Bash"), Some("git push"), None, Policy::Block), rule(Some("Bash"), Some("git"), None, Policy::Allow), rule(None, None, Some("**"), Policy::Ask)];
        let policy_of = |tool: &str, input: Value| match_rule(&rules, tool, &input).map(|one| one.policy);
        assert_eq!(policy_of("Bash", json!({ "command": "git push" })), Some(Policy::Block));
        assert_eq!(policy_of("Bash", json!({ "command": "git status" })), Some(Policy::Allow));
        assert_eq!(policy_of("Bash", json!({ "command": "ls" })), None);
        // A rule with a command does not match a call that has none; a rule with a path does not match a call without one.
        assert_eq!(policy_of("Bash", json!({})), None);
        assert_eq!(policy_of("Read", json!({ "file_path": "a/b.rs" })), Some(Policy::Ask));
        assert_eq!(policy_of("Read", json!({ "file_path": 3 })), None);
        assert_eq!(policy_of("Grep", json!({ "path": "src" })), Some(Policy::Ask));
        assert_eq!(policy_of("NotebookEdit", json!({ "notebook_path": "n.ipynb" })), Some(Policy::Ask));
        assert_eq!(match_rule(&[], "Bash", &json!({ "command": "ls" })), None);
    }

    #[test]
    fn globs_cross_folders_with_two_stars_and_stay_in_one_with_one() {
        assert!(glob_matches("src/**/*.ts", "src/a.ts"));
        assert!(glob_matches("src/**/*.ts", "src/x/y/a.ts"));
        assert!(!glob_matches("src/**/*.ts", "src/x/y/a.rs"));
        assert!(glob_matches("migrations/**", "migrations/a/b.sql"));
        assert!(!glob_matches("migrations/**", "other/migrations/a.sql"));
        assert!(glob_matches("**/migrations/**", "other/migrations/a.sql"));
        assert!(glob_matches("*.lock", "Cargo.lock"));
        assert!(!glob_matches("*.lock", "a/Cargo.lock"));
        assert!(glob_matches("a?c", "abc"));
        assert!(!glob_matches("a?c", "a/c"));
        assert!(glob_matches("a.b", "a.b"));
        assert!(!glob_matches("a.b", "axb"));
        assert!(glob_matches("a+(b)[c]{d}|e^$", "a+(b)[c]{d}|e^$"));
        assert!(glob_matches("日本/*.md", "日本/a.md"));
        assert!(glob_matches("", ""));
        assert!(!glob_matches("", "a"));
        assert!(glob_matches("**", "a/b/c"));
        assert!(glob_matches("a/**", "a/"));
    }

    #[test]
    fn dots_in_a_path_cannot_dodge_a_path_rule_and_a_project_folder_makes_relative_paths() {
        let rules = vec![rule(None, None, Some("migrations/**"), Policy::Ask), rule(None, None, Some("/etc/**"), Policy::Block)];
        let hit = |input: Value, cwd: Option<&str>| match_rule_in(&rules, "Edit", &input, cwd).map(|one| one.policy);
        assert_eq!(hit(json!({ "file_path": "src/../migrations/1.sql" }), None), Some(Policy::Ask));
        assert_eq!(hit(json!({ "file_path": "/repo/migrations/1.sql" }), Some("/repo")), Some(Policy::Ask));
        assert_eq!(hit(json!({ "file_path": "/repo/src/../migrations//1.sql" }), Some("/repo")), Some(Policy::Ask));
        assert_eq!(hit(json!({ "file_path": "migrations/1.sql" }), Some("/repo")), Some(Policy::Ask));
        assert_eq!(hit(json!({ "file_path": "/repo/../etc/hosts" }), Some("/repo")), Some(Policy::Block));
        assert_eq!(hit(json!({ "file_path": "/repo/migrations/1.sql" }), None), None);
        assert_eq!(hit(json!({ "file_path": "/repo/src/a.rs" }), Some("/repo")), None);
        assert_eq!(repo_path("/repo/a/../b.rs", "/repo"), Some("b.rs".to_string()));
        assert_eq!(repo_path("/other/b.rs", "/repo"), None);
        assert_eq!(repo_path("C:\\r\\b.rs", "/repo"), None);
        assert_eq!(repo_path("/b.rs", "/"), Some("b.rs".to_string()));
    }

    #[test]
    fn a_rule_that_cannot_be_compiled_or_run_counts_as_a_match_unless_it_allows() {
        // Made by hand, so the pattern was never checked by the parser.
        let broken = |policy| vec![rule(None, Some("("), None, policy)];
        let input = json!({ "command": "ls" });
        assert!(match_rule(&broken(Policy::Block), "Bash", &input).is_some());
        assert!(match_rule(&broken(Policy::Ask), "Bash", &input).is_some());
        assert!(match_rule(&broken(Policy::Allow), "Bash", &input).is_none());
        // A pattern that blows the backtrack budget.
        let heavy = vec![rule(None, Some("(a|aa)+(?!a)c"), None, Policy::Block)];
        assert!(match_rule(&heavy, "Bash", &json!({ "command": "a".repeat(60) })).is_some());
        let heavy_allow = vec![rule(None, Some("(a|aa)+(?!a)c"), None, Policy::Allow)];
        assert!(match_rule(&heavy_allow, "Bash", &json!({ "command": "a".repeat(60) })).is_none());
    }

    #[test]
    fn a_rule_can_tighten_but_not_loosen_a_block_a_protected_check_or_the_guard_file() {
        let edit = classify("Edit", &json!({ "file_path": "src/a.ts" }));
        let force = classify("Bash", &json!({ "command": "git push --force" }));
        let env_read = classify("Read", &json!({ "file_path": ".env" }));
        let guard = classify("Edit", &json!({ "file_path": ".claude/smartworkbench.json" }));

        // An ordinary call becomes a project rule when the rule asks or blocks, and is raised in risk.
        let ask = Rule { reason: Some("review".into()), ..rule(Some("Edit"), None, None, Policy::Ask) };
        let asked = apply_rule(edit.clone(), Some(&ask));
        assert_eq!((asked.category, asked.policy, asked.risk, asked.reason.as_str(), asked.rule), (Category::ProjectRule, Policy::Ask, Risk::Medium, "review", Some(Policy::Ask)));
        let low = classify("Read", &json!({ "file_path": "a.rs" }));
        assert_eq!(apply_rule(low.clone(), Some(&rule(None, None, Some("*"), Policy::Ask))).risk, Risk::Medium);
        let blocked = apply_rule(low, Some(&rule(None, None, Some("*"), Policy::Block)));
        assert_eq!((blocked.policy, blocked.risk, blocked.reason.as_str()), (Policy::Block, Risk::High, "project rule (.claude/smartworkbench.json)"));

        // An allow rule does not loosen a block, a protected check or the ask on the guard file; it only records what the project asked.
        for built_in in [&force, &env_read, &guard] {
            let after = apply_rule(built_in.clone(), Some(&rule(None, None, None, Policy::Allow)));
            assert_eq!((after.category, after.policy, after.risk, after.reason.as_str()), (built_in.category, built_in.policy, built_in.risk, built_in.reason.as_str()));
            assert_eq!(after.rule, Some(Policy::Allow));
        }
        // An allow rule on a plain ask does loosen it.
        let push = classify("Bash", &json!({ "command": "git push" }));
        let allowed = apply_rule(push, Some(&rule(None, None, None, Policy::Allow)));
        assert_eq!((allowed.category, allowed.policy, allowed.reason.as_str()), (Category::ProjectRule, Policy::Allow, "allowed by project rule"));

        // A rule that asks about what is already asked keeps the protected category and takes the rule's reason.
        let same = Rule { reason: Some("by the book".into()), ..rule(Some("Read"), None, None, Policy::Ask) };
        let kept = apply_rule(env_read.clone(), Some(&same));
        assert_eq!((kept.category, kept.reason.as_str(), kept.rule), (Category::SecretOutput, "by the book", Some(Policy::Ask)));
        // A rule that blocks what is asked takes it over.
        assert_eq!(apply_rule(env_read, Some(&rule(None, None, None, Policy::Block))).category, Category::ProjectRule);
        // No rule, no change.
        assert_eq!(apply_rule(force.clone(), None), force);
        // A rule that asks what is blocked: the block stays but what the project asked is recorded (the off profile then asks).
        let ask_force = apply_rule(force.clone(), Some(&rule(None, None, None, Policy::Ask)));
        assert_eq!((ask_force.category, ask_force.policy, ask_force.rule), (Category::DestructiveGit, Policy::Block, Some(Policy::Ask)));
        assert_eq!(decide(&ask_force, Profile::Off, false), Policy::Ask);
        assert_eq!(decide(&ask_force, Profile::Balanced, false), Policy::Block);
    }

    #[test]
    fn odd_input_never_panics() {
        let rules = vec![rule(Some("(?i)bash"), Some("rm\\s+-rf"), Some("**/x?*"), Policy::Block), rule(None, None, Some("[\\"), Policy::Ask)];
        for text in ["", " ", "\\", "\u{0}", "日本語", &"a".repeat(30_000), "../../..", "C:", "C:\\", "//", "~"] {
            let input = json!({ "command": text, "file_path": text, "path": text, "notebook_path": text });
            let _ = match_rule(&rules, text, &input);
            let _ = match_rule_in(&rules, text, &input, Some(text));
            let _ = glob_matches(text, text);
            let _ = parse_guard_file(text);
            let _ = repo_path(text, text);
        }
        for input in [Value::Null, json!([]), json!("x"), json!(3), json!({ "command": 3 }), json!({ "file_path": null, "path": "a" })] {
            let _ = match_rule(&rules, "Bash", &input);
        }
        for text in ["{\"rules\":[[]]}", "{\"rules\":[{\"policy\":[]}]}", "{\"rules\":{}}", "{\"profile\":3,\"rules\":null}", "{\"rules\":[{\"policy\":\"ask\",\"path\":\"[\"}]}"] {
            let _ = parse_guard_file(text);
        }
    }
}
