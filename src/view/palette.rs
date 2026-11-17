//! The command palette (Ctrl+K): the list of what it can do and the filter over it. The window only draws the rows and runs the one
//! picked; which commands exist, and in what order a typed text puts them, is decided here.
use super::i18n::{tr, Lang};

/// One command: an id the window acts on, and the i18n key of its words.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cmd {
    pub id: &'static str,
    pub key: &'static str,
    /// The keys that do the same without the palette ("" for none).
    pub hint: &'static str,
}

const ALL: &[Cmd] = &[
    Cmd { id: "go_chat", key: "pal_go_chat", hint: "⌥1" },
    Cmd { id: "go_memory", key: "pal_go_memory", hint: "⌥2" },
    Cmd { id: "go_skills", key: "pal_go_skills", hint: "⌥3" },
    Cmd { id: "go_sched", key: "pal_go_sched", hint: "⌥4" },
    Cmd { id: "go_files", key: "pal_go_files", hint: "⌥5" },
    Cmd { id: "go_team", key: "pal_go_team", hint: "⌥6" },
    Cmd { id: "go_vhdb", key: "pal_go_vhdb", hint: "⌥7" },
    Cmd { id: "new_chat", key: "pal_new_chat", hint: "" },
    Cmd { id: "settings", key: "pal_settings", hint: "" },
    Cmd { id: "terminal", key: "pal_terminal", hint: "" },
    Cmd { id: "theme_dark", key: "pal_theme_dark", hint: "" },
    Cmd { id: "theme_light", key: "pal_theme_light", hint: "" },
    Cmd { id: "theme_system", key: "pal_theme_system", hint: "" },
    Cmd { id: "notify_on", key: "pal_notify_on", hint: "" },
    Cmd { id: "notify_off", key: "pal_notify_off", hint: "" },
];

/// The commands that exist in this run: the terminal one only when the terminal is on.
pub fn commands(terminal: bool) -> Vec<Cmd> {
    ALL.iter().copied().filter(|c| terminal || c.id != "terminal").collect()
}

/// The commands whose words contain the typed text (any case), the ones that start with it first; the order inside each group is
/// the list's own. An empty text gives all of them.
pub fn filter(query: &str, lang: Lang, cmds: &[Cmd]) -> Vec<Cmd> {
    let q = query.trim().to_lowercase();
    let label = |c: &Cmd| tr(lang, c.key).to_lowercase();
    let (mut starts, mut has) = (Vec::new(), Vec::new());
    for c in cmds {
        let l = label(c);
        if q.is_empty() || l.starts_with(&q) {
            starts.push(*c);
        } else if l.contains(&q) || c.id.contains(&q) {
            has.push(*c);
        }
    }
    starts.extend(has);
    starts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terminal_command_exists_only_when_the_terminal_is_on() {
        assert!(commands(true).iter().any(|c| c.id == "terminal"));
        assert!(!commands(false).iter().any(|c| c.id == "terminal"));
        let ids: Vec<_> = commands(true).iter().map(|c| c.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "every id once");
    }

    #[test]
    fn typing_narrows_the_list_and_puts_the_ones_that_start_with_it_first() {
        let all = commands(true);
        assert_eq!(filter("", Lang::En, &all).len(), all.len());
        assert_eq!(filter("   ", Lang::En, &all).len(), all.len());
        let themes = filter("THEME", Lang::En, &all);
        assert!(themes.len() >= 3 && themes.iter().all(|c| c.id.starts_with("theme") || tr(Lang::En, c.key).to_lowercase().contains("theme")), "{themes:?}");
        assert!(filter("zzzz", Lang::En, &all).is_empty());
        // Korean words filter the Korean labels.
        let ko = filter("테마", Lang::Ko, &all);
        assert!(!ko.is_empty() && ko.iter().all(|c| c.id.starts_with("theme")), "{ko:?}");
        // A command whose words start with the text comes before one that only contains it.
        let ordered = filter("g", Lang::En, &all);
        let first_contains = ordered.iter().position(|c| !tr(Lang::En, c.key).to_lowercase().starts_with('g'));
        let last_starts = ordered.iter().rposition(|c| tr(Lang::En, c.key).to_lowercase().starts_with('g'));
        if let (Some(a), Some(b)) = (first_contains, last_starts) {
            assert!(b < a, "{ordered:?}");
        }
    }
}
