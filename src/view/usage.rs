//! The limits `claude` reported for an account, as rows for the desktop app: a label, a percentage and a bar for each window (5 hours,
//! the week, a model's week), when it resets, and one short line for the header. A value is as old as the last turn of that account
//! (the card says when it was seen); a window whose reset time has passed is shown as reset, not as the old percentage.
use super::i18n::{fill, tr, Lang};
use super::panels::ago;
use crate::usage::{AccountUsage, Win};

/// How long after its reset time a window is still believed to be over (clock differences).
const SLACK_S: u64 = 60;

#[derive(Debug, Clone, PartialEq)]
pub struct WinView {
    pub key: String,
    pub label: String,
    /// "85%", or "—" when the stream gave no number.
    pub pct: String,
    /// 0..=1, for the bar.
    pub frac: f32,
    /// ok, warn, limit, reset or unknown.
    pub level: &'static str,
    pub reset: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageView {
    pub has_data: bool,
    pub wins: Vec<WinView>,
    /// The header chip: the most used window that has not reset, "5시간 85%".
    pub chip: String,
    pub chip_level: &'static str,
    /// The two first windows on one line, for the account's row.
    pub compact: String,
    pub compact_level: &'static str,
    /// "2m 기준": when the newest of these was seen.
    pub seen: String,
}

fn order(key: &str) -> (u8, String) {
    let rank = match key {
        "five_hour" => 0,
        "seven_day" => 1,
        "seven_day_opus" => 2,
        "seven_day_sonnet" => 3,
        "seven_day_overage_included" => 5,
        "overage" => 6,
        k if k.starts_with("seven_day_") => 4,
        _ => 7,
    };
    (rank, key.to_owned())
}

/// The window's name. A weekly window of a model the app does not know by name is "주간 · <Model>" (`seven_day_fable` says Fable);
/// what `claude` calls `seven_day_overage_included` is a per-model weekly bucket, and its name here is a guess.
pub fn label(key: &str, lang: Lang) -> String {
    match key {
        "five_hour" => tr(lang, "us_5h"),
        "seven_day" => tr(lang, "us_week"),
        "seven_day_opus" => tr(lang, "us_week_opus"),
        "seven_day_sonnet" => tr(lang, "us_week_sonnet"),
        "seven_day_overage_included" => tr(lang, "us_week_incl"),
        "overage" => tr(lang, "us_overage"),
        k => match k.strip_prefix("seven_day_") {
            Some(model) => {
                let mut chars = model.chars();
                let name: String = chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default();
                fill(&tr(lang, "us_week_m"), &[("m", name)])
            }
            None => fill(&tr(lang, "us_other"), &[("k", k.to_owned())]),
        },
    }
}

/// "2시간 10분", "35분", "1일 3시간".
pub fn remaining(secs: u64, lang: Lang) -> String {
    let (d, h, m) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
    if d > 0 {
        fill(&tr(lang, "us_d"), &[("d", d.to_string()), ("h", h.to_string())])
    } else if h > 0 {
        fill(&tr(lang, "us_hm"), &[("h", h.to_string()), ("m", m.to_string())])
    } else {
        fill(&tr(lang, "us_m"), &[("m", m.max(1).to_string())])
    }
}

fn win_view(key: &str, w: &Win, now_s: u64, lang: Lang) -> WinView {
    let over = w.resets_at + SLACK_S < now_s;
    let level = if over {
        "reset"
    } else if w.status == "rejected" || w.util.is_some_and(|u| u >= 1.0) {
        "limit"
    } else if w.status == "allowed_warning" || w.util.is_some_and(|u| u >= 0.8) {
        "warn"
    } else if w.util.is_none() {
        "unknown"
    } else {
        "ok"
    };
    let pct = match (over, w.util) {
        (true, _) => String::new(),
        (false, Some(u)) => format!("{}%", (u * 100.0).round() as i64),
        (false, None) => "—".to_owned(),
    };
    let reset = if over { tr(lang, "us_reset_done") } else { fill(&tr(lang, "us_reset_in"), &[("t", remaining(w.resets_at.saturating_sub(now_s), lang))]) };
    let frac = if over { 0.0 } else { w.util.unwrap_or(0.0).clamp(0.0, 1.0) as f32 };
    WinView { key: key.to_owned(), label: label(key, lang), pct, frac, level, reset }
}

pub fn usage_view(usage: Option<&AccountUsage>, now_ms: u64, lang: Lang) -> UsageView {
    let now_s = now_ms / 1000;
    let mut keys: Vec<&String> = usage.map(|u| u.keys().collect()).unwrap_or_default();
    keys.sort_by_key(|k| order(k));
    let wins: Vec<WinView> = keys.iter().filter_map(|k| usage.and_then(|u| u.get(*k)).map(|w| win_view(k, w, now_s, lang))).collect();
    let live: Vec<&WinView> = wins.iter().filter(|w| w.level != "reset").collect();
    let top = live.iter().filter(|w| w.level != "unknown").max_by_key(|w| (w.level == "limit", w.level == "warn", (w.frac * 1000.0) as i64)).or(live.first());
    let (chip, chip_level) = match top {
        Some(w) => (format!("{} {}", w.label, w.pct), w.level),
        None => (tr(lang, "us_chip_none"), "unknown"),
    };
    let shown: Vec<&&WinView> = live.iter().take(2).collect();
    let compact = shown.iter().map(|w| format!("{} {}", w.label, w.pct)).collect::<Vec<_>>().join(" · ");
    // The colour is that of what the line says, not of a window it leaves out.
    let compact_level = ["limit", "warn", "ok"].into_iter().find(|l| shown.iter().any(|w| w.level == *l)).unwrap_or("unknown");
    let newest = usage.and_then(|u| u.values().map(|w| w.seen_at).max());
    let seen = newest.map(|t| fill(&tr(lang, "us_seen"), &[("t", ago(t, now_ms, lang))])).unwrap_or_default();
    UsageView { has_data: !wins.is_empty(), wins, chip, chip_level, compact, compact_level, seen }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const NOW: u64 = 1_800_000_000_000;

    fn win(util: Option<f64>, resets_in: i64, status: &str) -> Win {
        Win { util, resets_at: (NOW as i64 / 1000 + resets_in) as u64, status: status.into(), seen_at: NOW - 120_000 }
    }

    #[test]
    fn windows_are_named_ordered_and_levelled() {
        let mut u: AccountUsage = BTreeMap::new();
        u.insert("seven_day".into(), win(Some(0.1), 3 * 86400 + 3600, "allowed"));
        u.insert("five_hour".into(), win(Some(0.85), 2 * 3600 + 600, "allowed_warning"));
        u.insert("seven_day_fable".into(), win(Some(1.02), 86400, "allowed"));
        u.insert("seven_day_overage_included".into(), win(None, 86400, ""));
        let v = usage_view(Some(&u), NOW, Lang::Ko);
        let keys: Vec<&str> = v.wins.iter().map(|w| w.key.as_str()).collect();
        assert_eq!(keys, vec!["five_hour", "seven_day", "seven_day_fable", "seven_day_overage_included"]);
        assert_eq!((v.wins[0].label.as_str(), v.wins[0].pct.as_str(), v.wins[0].level), ("5시간", "85%", "warn"));
        assert_eq!(v.wins[0].reset, "2시간 10분 후 초기화");
        assert_eq!(v.wins[1].reset, "3일 1시간 후 초기화");
        assert_eq!((v.wins[2].label.as_str(), v.wins[2].pct.as_str(), v.wins[2].level), ("주간 · Fable", "102%", "limit"));
        assert!((v.wins[2].frac - 1.0).abs() < f32::EPSILON, "the bar stops at full");
        assert_eq!((v.wins[3].pct.as_str(), v.wins[3].level), ("—", "unknown"));
        assert_eq!(label("seven_day_fable", Lang::En), "Weekly · Fable");
        // The header chip is the window to worry about first; the row line is the first two.
        assert_eq!((v.chip.as_str(), v.chip_level), ("주간 · Fable 102%", "limit"));
        assert_eq!((v.compact.as_str(), v.compact_level), ("5시간 85% · 주간 10%", "warn"));
        assert_eq!(v.seen, "2m 기준");
    }

    #[test]
    fn a_window_that_has_reset_is_not_shown_as_its_old_percentage_and_no_data_says_so() {
        let mut u: AccountUsage = BTreeMap::new();
        u.insert("five_hour".into(), win(Some(0.9), -3600, "allowed_warning"));
        u.insert("seven_day".into(), win(Some(0.3), 86400, "allowed"));
        let v = usage_view(Some(&u), NOW, Lang::Ko);
        assert_eq!((v.wins[0].level, v.wins[0].pct.as_str(), v.wins[0].reset.as_str()), ("reset", "", "초기화됨"));
        assert_eq!((v.chip.as_str(), v.chip_level), ("주간 30%", "ok"));
        assert_eq!(v.compact, "주간 30%");
        let none = usage_view(None, NOW, Lang::Ko);
        assert!(!none.has_data && none.wins.is_empty());
        assert_eq!((none.chip.as_str(), none.chip_level, none.compact.as_str(), none.seen.as_str()), ("한도 —", "unknown", "", ""));
        // Rejected is a limit whatever the number.
        let mut r: AccountUsage = BTreeMap::new();
        r.insert("five_hour".into(), win(Some(0.5), 600, "rejected"));
        assert_eq!(usage_view(Some(&r), NOW, Lang::En).wins[0].level, "limit");
        assert_eq!(remaining(30, Lang::En), "1m");
    }
}
