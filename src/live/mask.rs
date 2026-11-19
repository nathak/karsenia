//! A text made fit to leave the app: control characters out, secrets hidden (`risk::redact`, which hides the whole text when it cannot be
//! scanned to its end), then cut to a length. The order matters: redacting first means a secret cut in half by the limit is never shown.
use crate::workbench::risk::redact;

/// The most that is scanned of a text (a longer one is cut before it is read, so a huge result costs little).
const SCAN: usize = 64 * 1024;

/// The text, masked and cut to `limit` characters, and whether it was cut.
pub fn mask(text: &str, limit: usize) -> (String, bool) {
    let mut cut = false;
    let mut scanned: String = text.chars().filter(|c| *c == '\n' || *c == '\t' || !c.is_control()).take(SCAN + 1).collect();
    if scanned.chars().count() > SCAN {
        scanned = scanned.chars().take(SCAN).collect();
        cut = true;
    }
    let hidden = redact(&scanned);
    if hidden.chars().count() > limit {
        return (hidden.chars().take(limit).collect(), true);
    }
    (hidden, cut)
}

/// As `mask`, for a place that has no use for the flag.
pub fn mask_text(text: &str, limit: usize) -> String {
    mask(text, limit).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_hidden_and_a_secret_cut_by_the_limit_is_never_shown() {
        let key = format!("sk-ant-{}", "a1b2c3d4e5".repeat(6));
        let (out, _) = mask(&format!("use {key} now"), 2000);
        assert!(!out.contains("a1b2c3d4e5"), "{out}");
        // The key is hidden before the limit is looked at, so a limit that would have cut it in half shows nothing of it.
        let (out, cut) = mask(&format!("0123456789 {key}"), 20);
        assert!(!cut && out == "0123456789 ***", "{out}");
        let (out, cut) = mask(&format!("{} {key}", "w".repeat(15)), 18);
        assert!(cut && !out.contains("sk-ant"), "{out}");
        let (out, _) = mask("curl https://user:hunter2@example.com/x", 2000);
        assert!(!out.contains("hunter2"), "{out}");
        let (out, _) = mask("export API_KEY=\"abcdef123456\"", 2000);
        assert!(!out.contains("abcdef123456"), "{out}");
        let (out, _) = mask("-----BEGIN PRIVATE KEY-----\nMIIBVgIBADANBg\n-----END PRIVATE KEY-----", 2000);
        assert!(!out.contains("MIIBVg"), "{out}");
        let (out, _) = mask("Authorization: Bearer abc.def.ghi123456", 2000);
        assert!(!out.contains("ghi123456"), "{out}");
    }

    #[test]
    fn the_limit_counts_characters_and_says_when_it_cut() {
        let (out, cut) = mask(&"가".repeat(130), 120);
        assert_eq!((out.chars().count(), cut), (120, true));
        let (out, cut) = mask("short", 120);
        assert_eq!((out.as_str(), cut), ("short", false));
        let (out, cut) = mask(&"x".repeat(2000), 2000);
        assert_eq!((out.chars().count(), cut), (2000, false), "a text of exactly the limit is not cut");
        let huge = "y ".repeat(100_000);
        let (out, cut) = mask(&huge, 2000);
        assert!(cut && out.chars().count() <= 2000);
    }

    #[test]
    fn control_characters_go_but_lines_and_tabs_stay() {
        let (out, _) = mask("a\u{1b}[31mb\nc\td\u{0}e", 100);
        assert!(!out.contains('\u{1b}') && !out.contains('\u{0}'), "{out:?}");
        assert!(out.contains('\n') && out.contains('\t'));
    }
}
