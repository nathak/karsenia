//! The words of the project-settings check: what the `project_check` event found, one line each, for the window's card (the rules are in
//! `crate::project_check`). A line is `bad` (drawn as a warning) when it is a warning or a danger; notes are drawn quietly.
use serde_json::Value;

use crate::view::i18n::{tr, Lang};

/// Whether the card may offer to remember a decision for the project: the core refuses it for a link, a file too big to read and a file that
/// is not valid (see `project_trust::remember_allowed`), so the card does not offer it then.
pub fn can_remember(data: &Value) -> bool {
    let findings = data["findings"].as_array().map(Vec::as_slice).unwrap_or_default();
    !findings.is_empty() && !findings.iter().any(|f| matches!(f["key"].as_str(), Some("link" | "too_big" | "malformed")))
}

pub fn lines(data: &Value, lang: Lang) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for f in data["findings"].as_array().into_iter().flatten() {
        let severity = f["severity"].as_str().unwrap_or("info");
        let bad = matches!(severity, "warn" | "danger");
        let rule = tr(lang, &format!("proj_rule_{}", f["key"].as_str().unwrap_or_default()));
        let text = f["text"].as_str().unwrap_or_default();
        let tail = if text.is_empty() { String::new() } else { format!(": {text}") };
        out.push((format!("{} · {} · {rule}{tail}", tr(lang, &format!("proj_sev_{severity}")), f["file"].as_str().unwrap_or_default()), bad));
    }
    // What needs a decision first.
    out.sort_by_key(|(_, bad)| !*bad);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn remembering_is_offered_unless_a_file_is_a_link_too_big_or_not_valid() {
        assert!(can_remember(&json!({"findings": [{"key": "hooks", "severity": "danger"}]})));
        for key in ["link", "too_big", "malformed"] {
            assert!(!can_remember(&json!({"findings": [{"key": "hooks"}, {"key": key}]})), "{key}");
        }
        assert!(!can_remember(&json!({"findings": []})) && !can_remember(&json!({})));
    }

    #[test]
    fn a_finding_is_one_line_with_its_severity_file_rule_and_text_and_warnings_come_first() {
        let data = json!({"hash": "h", "findings": [
            {"severity": "info", "file": ".claude/settings.json", "key": "env", "text": "A=•••"},
            {"severity": "danger", "file": ".claude/settings.json", "key": "hooks", "text": "Stop (./x.sh)"},
            {"severity": "warn", "file": ".mcp.json", "key": "link", "text": ""},
        ]});
        let ko = lines(&data, Lang::Ko);
        assert_eq!(ko.len(), 3);
        assert!(ko[0].1 && ko[1].1 && !ko[2].1);
        assert!(ko[0].0.contains("Stop (./x.sh)") && ko[0].0.contains(".claude/settings.json"), "{}", ko[0].0);
        assert!(!ko[1].0.ends_with(": "), "{}", ko[1].0);
        let en = lines(&data, Lang::En);
        assert_ne!(ko[0].0, en[0].0, "both languages are worded");
        assert!(lines(&json!({}), Lang::En).is_empty());
    }

    #[test]
    fn every_rule_and_severity_has_words_in_both_languages() {
        for key in ["hooks", "env_redirect", "env_unknown", "env", "helper", "allow_broad", "allow_wide", "mode", "dirs", "mcp_all", "mcp_enabled", "mcp_command", "mcp_url", "plugins", "marketplaces", "status_line", "malformed", "too_big", "link"] {
            for lang in [Lang::Ko, Lang::En] {
                let k = format!("proj_rule_{key}");
                assert_ne!(tr(lang, &k), k, "{k}");
            }
        }
        for sev in ["danger", "warn", "info"] {
            let k = format!("proj_sev_{sev}");
            assert_ne!(tr(Lang::En, &k), k);
        }
    }
}
