//! A worktree chat's header card, turned into text for the desktop app: what the merge check found, what stands in the way, and the
//! questions asked before a merge or a clean-up. The data is the `wt_status` reply (the host's `Facts`). What the host says on an
//! error (its own sentence) is shown as it is.
use serde_json::Value;

use super::i18n::{fill, tr, Lang};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WtView {
    /// "3 commits · 2 files +10 −4 · main is 1 commit ahead".
    pub summary: String,
    /// What stands in the way, or that it can be merged; the flag is true for a problem.
    pub lines: Vec<(String, bool)>,
    pub can_merge: bool,
    pub missing: bool,
}

fn n(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or(0)
}

fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().chars().filter(|c| !c.is_control()).collect()
}

/// One blocker as a sentence; an unknown code is shown as it is.
pub fn blocker_text(code: &str, f: &Value, lang: Lang) -> String {
    let conflicts: Vec<&str> = f["conflicts"].as_array().map(|c| c.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let more = conflicts.len().saturating_sub(3) as u64 + n(f, "conflicts_more");
    let mut files = conflicts.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    if more > 0 {
        files = format!("{files} +{more}");
    }
    let key = format!("wt_b_{code}");
    let sentence = tr(lang, &key);
    if sentence == key {
        return code.chars().filter(|c| !c.is_control()).take(40).collect();
    }
    fill(&sentence, &[("main", f["main_branch"].as_str().unwrap_or("?").to_owned()), ("base", text(f, "base_branch")), ("files", files)])
}

pub fn status_view(f: &Value, lang: Lang) -> WtView {
    let blockers: Vec<String> = f["blockers"].as_array().map(|b| b.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
    let missing = f["missing"] == true;
    let mut lines: Vec<(String, bool)> = blockers.iter().map(|b| (blocker_text(b, f, lang), true)).collect();
    if lines.is_empty() && !missing {
        lines.push((tr(lang, "wt_ok"), false));
    }
    let summary = if missing {
        String::new()
    } else {
        fill(&tr(lang, "wt_summary"), &[("n", n(f, "ahead").to_string()), ("f", n(f, "files").to_string()), ("a", n(f, "insertions").to_string()), ("d", n(f, "deletions").to_string()), ("base", text(f, "base_branch")), ("b", n(f, "behind").to_string())])
    };
    WtView { summary, lines, can_merge: blockers.is_empty() && !missing, missing }
}

/// The questions before a clean-up: (changes that would be lost, the first question, the second one when commits would be left on
/// no other branch).
pub fn remove_prompts(f: &Value, lang: Lang) -> (bool, String, Option<String>) {
    let uncommitted = n(f, "uncommitted");
    let vars = [("path", text(f, "path")), ("base", text(f, "base_branch")), ("branch", text(f, "branch")), ("n", uncommitted.to_string())];
    let force = uncommitted > 0;
    let mut first = fill(&tr(lang, "wt_remove1"), &vars);
    if force {
        first.push_str(&fill(&tr(lang, "wt_remove_loss"), &vars));
    }
    let ahead = n(f, "ahead");
    // Asked unless every commit is in the base's history; content that only looks merged (a squash) is said so, and still asked.
    let second = (ahead > 0 && f["in_base"] != true).then(|| {
        let key = if f["same_content"] == true { "wt_remove2b" } else { "wt_remove2" };
        fill(&tr(lang, key), &[("base", text(f, "base_branch")), ("branch", text(f, "branch")), ("n", ahead.to_string())])
    });
    (force, first, second)
}

pub fn merge_prompt(f: &Value, lang: Lang) -> String {
    fill(&tr(lang, "wt_merge_confirm"), &[("branch", text(f, "branch")), ("n", n(f, "ahead").to_string()), ("repo", text(f, "repo")), ("base", text(f, "base_branch"))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn facts() -> Value {
        json!({"missing": false, "main_branch": "main", "base_branch": "main", "ahead": 3, "behind": 1, "files": 2, "insertions": 10, "deletions": 4, "uncommitted": 0,
            "in_base": false, "same_content": false, "blockers": [], "conflicts": [], "conflicts_more": 0, "tip": "abc", "main_head": "def", "path": "/w/p", "branch": "tenrec/x", "repo": "/r"})
    }

    #[test]
    fn a_clean_worktree_can_be_merged_and_says_what_it_holds() {
        let v = status_view(&facts(), Lang::En);
        assert!(v.can_merge && !v.missing);
        assert_eq!(v.summary, "3 commits · 2 files +10 −4 · main is 1 commits ahead");
        assert_eq!(v.lines, vec![("It can be merged.".to_owned(), false)]);
    }

    #[test]
    fn what_stands_in_the_way_is_said_in_words() {
        let mut f = facts();
        f["blockers"] = json!(["uncommitted", "wrong_branch", "conflict", "strange"]);
        f["main_branch"] = json!("dev");
        f["conflicts"] = json!(["a", "b", "c", "d"]);
        f["conflicts_more"] = json!(2);
        let v = status_view(&f, Lang::En);
        assert!(!v.can_merge && v.lines.iter().all(|(_, bad)| *bad));
        assert!(v.lines[1].0.contains("dev") && v.lines[1].0.contains("main"), "{:?}", v.lines[1]);
        assert!(v.lines[2].0.contains("a, b, c +3"), "{:?}", v.lines[2]);
        assert_eq!(v.lines[3].0, "strange");
        let gone = status_view(&json!({"missing": true, "blockers": ["missing"]}), Lang::En);
        assert!(gone.missing && !gone.can_merge && gone.summary.is_empty() && gone.lines.len() == 1);
    }

    #[test]
    fn a_clean_up_asks_once_or_twice() {
        let (force, first, second) = remove_prompts(&facts(), Lang::En);
        assert!(!force && first.contains("/w/p") && !first.contains("lost"));
        assert!(second.as_deref().is_some_and(|s| s.contains("3 commits") && s.contains("tenrec/x")), "{second:?}");
        let mut dirty = facts();
        dirty["uncommitted"] = json!(2);
        dirty["in_base"] = json!(true);
        let (force, first, second) = remove_prompts(&dirty, Lang::En);
        assert!(force && first.contains("2 uncommitted"), "{first}");
        assert!(second.is_none(), "every commit is in the base: the branch can go without a question");
        let mut squashed = facts();
        squashed["same_content"] = json!(true);
        assert!(remove_prompts(&squashed, Lang::En).2.is_some_and(|s| s.contains("looks")));
        assert!(merge_prompt(&facts(), Lang::En).contains("3 commits") && merge_prompt(&facts(), Lang::En).contains("/r"));
    }
}
