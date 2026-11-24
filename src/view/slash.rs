//! The "/" menu of a chat: the commands of Claude Code a person can run from the composer. The program answers the local ones
//! (`/cost`, `/context`, `/usage`, `/compact`, `/clear`) without a model; `/model name` is done by the app itself over the control
//! channel (see `Session`), so that the header and the saved model stay right. The names of skills and custom commands come from
//! the program's own `init` (empty until a chat's first message has started a process).
use serde_json::Value;

use crate::view::i18n::{tr, Lang};
use crate::workbench::team::role_model_ok;

/// One line of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Without the slash.
    pub name: String,
    /// The text of what it does ("" for a skill or custom command, which is only named).
    pub about: String,
    /// It takes an argument: picking it fills the draft with `/name ` instead of sending it.
    pub arg: bool,
    /// It throws the conversation away: the app asks first.
    pub confirm: bool,
    pub builtin: bool,
}

/// The built-ins: name, i18n key of the description, takes an argument, needs a confirmation.
const BUILTIN: [(&str, &str, bool, bool); 6] = [
    ("compact", "slash_compact", true, false),
    ("context", "slash_context", false, false),
    ("cost", "slash_cost", false, false),
    ("usage", "slash_usage", false, false),
    ("model", "slash_model", true, false),
    ("clear", "slash_clear", false, true),
];

/// Commands that only make sense at a terminal (they open a screen, log in or out, or change the terminal): never offered, though
/// the program lists them.
const TERMINAL_ONLY: [&str; 22] = [
    "config", "doctor", "color", "focus", "reload-plugins", "reload-skills", "heapdump", "login", "logout", "resume", "exit", "quit", "ide",
    "vim", "terminal-setup", "theme", "output-style", "auto-mode-setup", "import", "design-consent", "design-revoke", "__remote-workflow",
];

/// What `init` lists, without the commands that only work in a terminal (its own `terminal_slash_commands` and the fixed list).
pub fn init_names(init: &Value) -> Vec<String> {
    let words = |key: &str| -> Vec<String> { init[key].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default() };
    let terminal = words("terminal_slash_commands");
    let mut out: Vec<String> = words("slash_commands")
        .into_iter()
        .filter(|n| !n.is_empty() && !n.starts_with('_') && !terminal.contains(n) && !TERMINAL_ONLY.contains(&n.as_str()) && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-' | '.')))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The menu for what the person has typed after the slash (`query`, may be empty): the built-ins first, then the other names
/// (those starting with the query before those that only contain it).
pub fn list(lang: Lang, names: &[String], query: &str) -> Vec<Entry> {
    let q = query.trim().trim_start_matches('/').to_lowercase();
    let rank = |name: &str| -> Option<u8> {
        let n = name.to_lowercase();
        if q.is_empty() || n.starts_with(&q) {
            Some(0)
        } else if n.contains(&q) {
            Some(1)
        } else {
            None
        }
    };
    let mut builtin: Vec<(u8, Entry)> = BUILTIN
        .iter()
        .filter_map(|(name, key, arg, confirm)| rank(name).map(|r| (r, Entry { name: (*name).into(), about: tr(lang, key), arg: *arg, confirm: *confirm, builtin: true })))
        .collect();
    builtin.sort_by_key(|(r, _)| *r);
    let mut others: Vec<(u8, Entry)> = names
        .iter()
        .filter(|n| !BUILTIN.iter().any(|(b, ..)| b == n))
        .filter_map(|n| rank(n).map(|r| (r, Entry { name: n.clone(), about: String::new(), arg: true, confirm: false, builtin: false })))
        .collect();
    others.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    builtin.into_iter().chain(others).map(|(_, e)| e).collect()
}

/// What the app does with a message before it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routed {
    /// `/model name` with a model the program accepts by name: done by the app (no message goes to the model).
    Model(String),
    /// Anything else goes to the program as it is.
    Text,
}

pub fn route(text: &str) -> Routed {
    let mut words = text.trim().split_whitespace();
    if words.next() == Some("/model") {
        if let (Some(name), None) = (words.next(), words.next()) {
            if role_model_ok(name) {
                return Routed::Model(name.to_owned());
            }
        }
    }
    Routed::Text
}

/// The draft's menu query: `Some(rest)` while the draft is one slash command being typed (a slash, a word, no space yet), else None.
pub fn draft_query(draft: &str) -> Option<&str> {
    let rest = draft.strip_prefix('/')?;
    (!rest.contains(char::is_whitespace) && rest.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-' | '.'))).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn names(list: &[Entry]) -> Vec<&str> {
        list.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn init_names_drop_terminal_screens_and_odd_names() {
        let init = json!({"slash_commands": ["compact", "doctor", "color", "my-skill", "plug:cmd", "_hidden", "bad name", "login", "my-skill"], "terminal_slash_commands": ["color"]});
        assert_eq!(init_names(&init), ["compact", "my-skill", "plug:cmd"]);
        assert!(init_names(&json!({})).is_empty());
    }

    #[test]
    fn the_menu_lists_built_ins_first_then_prefix_then_contains_matches() {
        let n: Vec<String> = ["compact", "my-skill", "swb", "unclear", "clean-up"].iter().map(|s| s.to_string()).collect();
        let all = list(Lang::En, &n, "");
        assert_eq!(names(&all)[..6], ["compact", "context", "cost", "usage", "model", "clear"]);
        assert_eq!(names(&all)[6..], ["clean-up", "my-skill", "swb", "unclear"]);
        // A built-in is not listed twice, and "cl" finds the built-in, the prefix match, then the one that only contains it.
        assert_eq!(names(&list(Lang::En, &n, "cl")), ["clear", "clean-up", "unclear"]);
        assert_eq!(names(&list(Lang::En, &n, "/SW")), ["swb"]);
        assert!(list(Lang::En, &n, "zzz").is_empty());
        // Without any init names the built-ins are still there.
        assert_eq!(list(Lang::Ko, &[], "").len(), 6);
        let clear = list(Lang::En, &[], "clear").remove(0);
        assert!(clear.confirm && !clear.arg && clear.builtin && !clear.about.is_empty());
        assert!(list(Lang::En, &[], "compact")[0].arg);
    }

    #[test]
    fn a_model_command_is_the_apps_own_and_everything_else_goes_through() {
        assert_eq!(route("/model haiku"), Routed::Model("haiku".into()));
        assert_eq!(route("  /model   claude-opus-5-5 "), Routed::Model("claude-opus-5-5".into()));
        assert_eq!(route("/model"), Routed::Text, "alone it asks the program for the current model");
        assert_eq!(route("/model a b"), Routed::Text);
        assert_eq!(route("/model not a model"), Routed::Text);
        assert_eq!(route("/model \"x\""), Routed::Text);
        assert_eq!(route("/compact now"), Routed::Text);
        assert_eq!(route("tell me about /model haiku"), Routed::Text);
    }

    #[test]
    fn the_menu_opens_only_while_a_slash_word_is_being_typed() {
        assert_eq!(draft_query("/"), Some(""));
        assert_eq!(draft_query("/com"), Some("com"));
        assert_eq!(draft_query("/plug:cmd"), Some("plug:cmd"));
        assert_eq!(draft_query("/compact "), None);
        assert_eq!(draft_query("/home/me/a.rs"), None);
        assert_eq!(draft_query("hello /x"), None);
        assert_eq!(draft_query(""), None);
    }
}
