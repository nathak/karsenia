//! The Files tab, turned into rows for the desktop app. The data is what the web page gets (`fs_listing`, `fs_text`); paths are
//! the project-relative ones the host hands out with `/` between names, and are only ever sent back as they came or joined with a
//! name the host listed: the host decides what a path may be.
use serde_json::Value;

use super::i18n::{fill, tr, Lang};

/// Largest text the window opens (the host reads up to 1 MiB; a text field that long is slow).
pub const MAX_OPEN: usize = 256 * 1024;

/// The folders on the way to `path`: ("/", "") first, then each name with its path.
pub fn crumbs(path: &str) -> Vec<(String, String)> {
    let mut out = vec![("/".to_owned(), String::new())];
    let mut so_far = String::new();
    for name in path.split('/').filter(|n| !n.is_empty()) {
        so_far = join(&so_far, name);
        out.push((name.to_owned(), so_far.clone()));
    }
    out
}

pub fn parent(path: &str) -> String {
    path.trim_end_matches('/').rsplit_once('/').map(|(p, _)| p.to_owned()).unwrap_or_default()
}

/// True when `path` is `root` or lies inside it (`root` is a folder path; the empty root holds everything).
pub fn is_under(path: &str, root: &str) -> bool {
    root.is_empty() || path == root || path.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
}

/// A single name typed in the window: not empty and without a `/` (the host judges the rest and refuses what is not a name).
pub fn single_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\0')
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{}/{name}", dir.trim_end_matches('/'))
    }
}

/// Whether a path is the project's trash or an entry directly in it (`trash-root`, `trash-entry`), or neither (empty).
pub fn trash_place(path: &str) -> String {
    use crate::server::project_fs::TRASH;
    let name = path.rsplit('/').next().unwrap_or_default();
    if path == TRASH {
        "trash-root".to_owned()
    } else if parent(path) == TRASH && name != ".from" && name != ".gitignore" {
        "trash-entry".to_owned()
    } else {
        String::new()
    }
}

pub fn fmt_size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FsRow {
    pub name: String,
    pub path: String,
    /// dir, file or other.
    pub kind: String,
    pub size: String,
    pub tags: String,
    /// The host does not let this one be opened.
    pub blocked: bool,
    /// For an entry of the trash: where it was deleted from (no control characters).
    pub from: String,
    /// `trash-root` for the trash folder, `trash-entry` for an entry directly in it (what can be deleted for good), else empty.
    pub place: String,
}

/// The rows of a folder and a line about what is missing from it. `dir` is the folder listed.
pub fn listing_view(data: &Value, dir: &str, lang: Lang) -> (Vec<FsRow>, String) {
    let rows = data["entries"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|e| {
                    // The name shown has no control characters; the path sent back is made of the name as the host wrote it.
                    let raw = e["name"].as_str().unwrap_or_default();
                    let name = raw.chars().filter(|c| !c.is_control()).collect::<String>();
                    let flags: Vec<&str> = e["flags"].as_array().map(|f| f.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                    let tags: Vec<String> = flags.iter().filter(|f| ["blocked", "guarded", "secret", "heavy", "trash"].contains(f)).map(|f| tr(lang, &format!("fs_tag_{f}"))).collect();
                    let kind = e["kind"].as_str().unwrap_or("other").to_owned();
                    FsRow {
                        path: join(dir, raw),
                        size: if kind == "file" { fmt_size(e["size"].as_u64().unwrap_or(0)) } else { String::new() },
                        tags: tags.join(" · "),
                        blocked: flags.contains(&"blocked"),
                        place: trash_place(&join(dir, raw)),
                        from: e["from"].as_str().unwrap_or_default().chars().filter(|c| !c.is_control()).collect(),
                        kind,
                        name,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let mut notes = Vec::new();
    if data["truncated"] == true {
        notes.push(tr(lang, "fs_truncated"));
    }
    if let Some(n) = data["unreadable"].as_u64().filter(|n| *n > 0) {
        notes.push(fill(&tr(lang, "fs_unreadable"), &[("n", n.to_string())]));
    }
    (rows, notes.join(" · "))
}

/// The line numbers beside the editor, one per line ("1\n2\n..."), up to `MAX_GUTTER` lines (a longer file gets none: a text that
/// long is slow to draw twice).
pub const MAX_GUTTER: usize = 20_000;

pub fn gutter(text: &str) -> String {
    let lines = text.split('\n').count();
    if lines > MAX_GUTTER {
        return String::new();
    }
    (1..=lines).map(|n| n.to_string()).collect::<Vec<_>>().join("\n")
}

/// The 1-based line and column of a byte offset in `text` (columns count characters).
pub fn line_col(text: &str, byte: usize) -> (usize, usize) {
    let mut end = byte.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let before = &text[..end];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

/// A file's text as the editor holds it: a CRLF file is edited with LF and written back with CRLF.
pub fn to_editor(text: &str, eol: &str) -> String {
    if eol == "crlf" {
        text.replace("\r\n", "\n")
    } else {
        text.to_owned()
    }
}

pub fn from_editor(text: &str, eol: &str) -> String {
    match eol {
        "crlf" => text.replace("\r\n", "\n").replace('\n', "\r\n"),
        // A CRLF text pasted into an LF file does not bring CRs into it.
        "lf" => text.replace("\r\n", "\n"),
        _ => text.to_owned(),
    }
}

/// Why the editor will not save this file, as an i18n key ("" when it will).
pub fn readonly_reason(eol: &str, guard_kind: &str) -> &'static str {
    if guard_kind == "block" {
        "fs_readonly_blocked"
    } else if eol == "mixed" {
        "fs_readonly_mixed"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_trash_and_what_is_directly_in_it_are_told_apart() {
        assert_eq!(trash_place(".tenrec-trash"), "trash-root");
        assert_eq!(trash_place(".tenrec-trash/1-abcdef01-a.txt"), "trash-entry");
        for other in ["a.txt", ".tenrec-trash/.from", ".tenrec-trash/.gitignore", ".tenrec-trash/x/y", "sub/.tenrec-trash"] {
            assert_eq!(trash_place(other), "", "{other}");
        }
    }

    #[test]
    fn a_path_is_under_a_folder_by_whole_parts_and_a_name_has_no_slash() {
        assert!(is_under("src/a.rs", "src") && is_under("src", "src") && is_under("a", ""));
        assert!(!is_under("src2/a", "src") && !is_under("sr", "src"));
        assert!(single_name("a b.txt") && !single_name("") && !single_name("a/b"));
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn paths_split_join_and_go_up() {
        assert_eq!(crumbs(""), vec![("/".to_owned(), String::new())]);
        assert_eq!(crumbs("a/b"), vec![("/".to_owned(), "".to_owned()), ("a".to_owned(), "a".to_owned()), ("b".to_owned(), "a/b".to_owned())]);
        assert_eq!((parent("a/b").as_str(), parent("a").as_str(), parent("").as_str()), ("a", "", ""));
        assert_eq!((join("", "x").as_str(), join("a/b", "x").as_str()), ("x", "a/b/x"));
        assert_eq!(fmt_size(10), "10 B");
        assert_eq!(fmt_size(2048), "2.0 KB");
        assert_eq!(fmt_size(3 * 1_048_576), "3.0 MB");
    }

    #[test]
    fn a_listing_has_tags_sizes_and_what_is_missing() {
        let data = json!({"entries": [
            {"name": "src", "kind": "dir", "link": false, "size": 0, "flags": []},
            {"name": ".env", "kind": "file", "link": false, "size": 12, "flags": ["secret", "guarded"]},
            {"name": ".git", "kind": "dir", "link": false, "size": 0, "flags": ["blocked", "heavy"]},
            {"name": "bad\u{7}name", "kind": "file", "link": false, "size": 2048, "flags": []}],
            "truncated": true, "unreadable": 2});
        let (rows, notice) = listing_view(&data, "app", Lang::En);
        assert_eq!((rows[0].path.as_str(), rows[0].size.as_str(), rows[0].tags.as_str(), rows[0].blocked), ("app/src", "", "", false));
        assert_eq!((rows[1].tags.as_str(), rows[1].size.as_str()), ("secret · held", "12 B"));
        assert!(rows[2].blocked && rows[2].tags.starts_with("blocked"), "{:?}", rows[2]);
        assert_eq!(rows[3].name, "badname");
        assert_eq!(rows[3].path, "app/bad\u{7}name", "the path keeps the host's own name, only the label is cleaned");
        assert!(notice.contains("2") && notice.contains("incomplete") || !notice.is_empty(), "{notice}");
        assert!(listing_view(&json!({}), "", Lang::En).0.is_empty());
    }

    #[test]
    fn the_gutter_counts_lines_and_the_cursor_is_a_line_and_a_column() {
        assert_eq!(gutter("a\nb\nc"), "1\n2\n3");
        assert_eq!(gutter(""), "1");
        assert_eq!(gutter(&"x\n".repeat(MAX_GUTTER + 5)), "", "a very long file has no gutter");
        assert_eq!(line_col("ab\ncd", 0), (1, 1));
        assert_eq!(line_col("ab\ncd", 4), (2, 2));
        // Columns count characters, and an offset inside a character is moved back to its start.
        assert_eq!(line_col("가나\n다", 3), (1, 2));
        assert_eq!(line_col("가나\n다", 4), (1, 2));
        assert_eq!(line_col("ab", 99), (1, 3));
    }

    #[test]
    fn line_endings_round_trip_and_odd_files_are_read_only() {
        let crlf = "a\r\nb\r\n";
        assert_eq!(to_editor(crlf, "crlf"), "a\nb\n");
        assert_eq!(from_editor("a\nb\n", "crlf"), crlf);
        assert_eq!(from_editor(&to_editor(crlf, "crlf"), "crlf"), crlf);
        assert_eq!((to_editor("a\r\nb", "lf").as_str(), from_editor("a\nb", "lf").as_str(), from_editor("a\r\nb", "lf").as_str()), ("a\r\nb", "a\nb", "a\nb"));
        assert_eq!((readonly_reason("mixed", ""), readonly_reason("lf", "block"), readonly_reason("lf", "")), ("fs_readonly_mixed", "fs_readonly_blocked", ""));
    }
}
