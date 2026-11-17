//! The list of earlier Claude Code sessions, as rows for the desktop app: what each one is called, where and when, and whether it can
//! be imported (a session already imported, a folder that is gone or that is not where the CLI kept the session cannot).
use std::collections::HashSet;

use super::i18n::{tr, Lang};
use super::panels::{ago, folder_name};
use crate::cli_sessions::CliSession;

/// A session touched this recently may still be open in a `claude` that is running.
pub const RECENT_MS: u64 = 120_000;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImpRow {
    pub id: String,
    pub title: String,
    pub folder: String,
    pub branch: String,
    pub when: String,
    /// "" or why it cannot be imported: done, missing or mismatch.
    pub blocked: String,
    pub blocked_text: String,
    /// Touched moments ago: asked about before it is imported.
    pub recent: bool,
}

/// The rows that match `filter` (title, folder or branch, ignoring case). `imported` are the ids already imported and `exists`
/// says whether a session's folder is a directory and `fit` whether the app can import into it (not `/`, not the home folder, not a link).
pub fn rows(list: &[CliSession], imported: &HashSet<String>, exists: &dyn Fn(&str) -> bool, fit: &dyn Fn(&str) -> bool, filter: &str, now_ms: u64, lang: Lang) -> Vec<ImpRow> {
    let needle = filter.trim().to_lowercase();
    list.iter()
        .filter(|s| needle.is_empty() || [&s.title, &s.cwd, &s.branch].iter().any(|f| f.to_lowercase().contains(&needle)))
        .map(|s| {
            let blocked = if imported.contains(&s.id) {
                "done"
            } else if !exists(&s.cwd) {
                "missing"
            } else if !s.folder_ok {
                "mismatch"
            } else if !fit(&s.cwd) {
                "unfit"
            } else {
                ""
            };
            ImpRow {
                id: s.id.clone(),
                title: if s.title.is_empty() { tr(lang, "imp_untitled") } else { s.title.clone() },
                folder: format!("{} · {}", folder_name(&s.cwd), s.cwd),
                branch: s.branch.clone(),
                when: ago(s.modified_ms, now_ms, lang),
                blocked: blocked.to_owned(),
                blocked_text: if blocked.is_empty() { String::new() } else { tr(lang, &format!("imp_{blocked}")) },
                recent: now_ms.saturating_sub(s.modified_ms) < RECENT_MS,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: &str, title: &str, cwd: &str, ok: bool, modified: u64) -> CliSession {
        CliSession { id: id.into(), path: "/x".into(), cwd: cwd.into(), title: title.into(), branch: "main".into(), modified_ms: modified, size: 1, folder_ok: ok }
    }

    #[test]
    fn rows_say_what_cannot_be_imported_and_why_and_filter_by_any_field() {
        let list = vec![s("a", "Fix the bug", "/w/alpha", true, 1_000), s("b", "", "/w/gone", true, 1_000), s("c", "Old", "/w/beta", false, 1_000), s("d", "Done one", "/w/delta", true, 3_600_000 + 1_000), s("e", "Home", "/w/alpha2", true, 1_000)];
        let imported: HashSet<String> = ["d".to_owned()].into();
        let exists = |p: &str| p != "/w/gone";
        let fit = |p: &str| p != "/w/alpha2";
        let now = 3_600_000 + 61_000;
        let all = rows(&list, &imported, &exists, &fit, "", now, Lang::En);
        assert_eq!(all.iter().map(|r| r.blocked.as_str()).collect::<Vec<_>>(), vec!["", "missing", "mismatch", "done", "unfit"]);
        assert_eq!(all[1].title, "(untitled)");
        assert_eq!(all[3].blocked_text, "Already imported");
        assert_eq!((all[0].when.as_str(), all[0].recent, all[3].recent), ("1h", false, true));
        assert!(all[0].folder.starts_with("alpha"));
        assert_eq!(rows(&list, &imported, &exists, &fit, " BUG ", now, Lang::En).len(), 1);
        assert_eq!(rows(&list, &imported, &exists, &fit, "beta", now, Lang::En)[0].id, "c");
        assert!(rows(&list, &imported, &exists, &fit, "nothing", now, Lang::En).is_empty());
    }
}
