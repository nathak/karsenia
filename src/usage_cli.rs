//! The limits as the program's own `/usage` command says them. The command is answered by `claude` itself with no model call (no cost,
//! no token), so the app can ask it when the person wants the numbers now: the stream's `rate_limit_event` shows only what a turn
//! of this app saw, and the same login used elsewhere moves the same windows. The app asks Anthropic nothing: the program does.
//!
//! The answer is text, in English, like
//! `Current session: 68% used · resets Nov 20, 6:39pm (Asia/Seoul)` and `Current week (all models): 81% used · resets Nov 24, 4:59pm (...)`.
//! It is read into the same `rate_limit_info` shape the stream has (so `usage::merge` checks it like any other source). What is not
//! understood is left out: a line that does not read, a window with a name this does not know, a time that is not a time. Nothing
//! understood at all is None, and the stored numbers stay as they were.
use std::path::Path;
use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveTime, TimeZone};
use serde_json::{json, Value};

use crate::claude::env::EnvPlan;

/// Long enough for the program's start-up hooks (a plug-in's session hooks run before it answers).
const ASK_TIMEOUT: Duration = Duration::from_secs(30);

/// The text the program answered, from its JSON output: the `result` of the last `result` line, unless that was an error.
pub fn result_text(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    text.lines().rev().filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok()).find(|v| v["type"] == "result").and_then(|v| {
        (v["is_error"] != true).then(|| v["result"].as_str().map(str::to_owned)).flatten()
    })
}

/// Asks the account's `claude` for its usage: `claude -p /usage`, run in the account's own folder. The answer is the text.
pub async fn ask(bin: &str, dir: &Path, env: &EnvPlan) -> Result<String, String> {
    let args = ["-p", "/usage", "--output-format", "json", "--no-session-persistence"];
    let (bytes, _) = crate::accounts::run_capped_within(bin, dir, env, &args, crate::accounts::Streams::Stdout, ASK_TIMEOUT).await?;
    result_text(&bytes).ok_or_else(|| "the program gave no usage answer".to_owned())
}

/// The window a line is about, by the name `usage::KNOWN` has: the session is the 5-hour window, the week of all models the weekly one,
/// the week of a model that model's weekly one. Others are not taken.
fn window_key(label: &str) -> Option<&'static str> {
    if label == "session" {
        return Some("five_hour");
    }
    let inner = label.strip_prefix("week (")?.strip_suffix(')')?;
    match inner.trim().to_ascii_lowercase().as_str() {
        "all models" => Some("seven_day"),
        "opus" => Some("seven_day_opus"),
        "sonnet" | "sonnet only" => Some("seven_day_sonnet"),
        _ => None,
    }
}

/// One line: the window, the fraction used and the text after `resets`.
fn read_line(line: &str) -> Option<(&'static str, f64, &str)> {
    let rest = line.trim().strip_prefix("Current ")?;
    let (label, rest) = rest.split_once(": ")?;
    let key = window_key(label.trim())?;
    let (pct, rest) = rest.split_once("% used")?;
    let pct: f64 = pct.trim().parse().ok().filter(|p: &f64| p.is_finite() && (0.0..=1000.0).contains(p))?;
    let when = rest.split_once("resets")?.1.trim();
    Some((key, pct / 100.0, when))
}

/// `6:39pm`, `6pm`, `12am`.
fn read_time(token: &str) -> Option<NaiveTime> {
    let lower = token.to_ascii_lowercase();
    let (digits, pm) = match (lower.strip_suffix("am"), lower.strip_suffix("pm")) {
        (Some(d), _) => (d, false),
        (_, Some(d)) => (d, true),
        _ => return None,
    };
    let (h, m) = match digits.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (digits.parse::<u32>().ok()?, 0),
    };
    if !(1..=12).contains(&h) || m > 59 {
        return None;
    }
    NaiveTime::from_hms_opt(h % 12 + if pm { 12 } else { 0 }, m, 0)
}

fn month_number(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let lower = name.to_ascii_lowercase();
    MONTHS.iter().position(|m| lower.starts_with(m)).map(|i| i as u32 + 1)
}

/// A reset time as `/usage` says it: `Nov 20, 6:39pm (Asia/Seoul)` or `6:39pm`. It is in the zone the program runs in, which is this
/// machine's, so `tz` is the local one; the name in the brackets is not used. Without a day it is today, or tomorrow when that is past;
/// without a year it is this year, or the next when that puts it half a year back (a January answer read in December).
fn read_reset<Tz: TimeZone>(text: &str, now_s: u64, tz: &Tz) -> Option<u64> {
    let text = text.split(" (").next().unwrap_or(text).trim();
    let text = text.replace(',', " ").replace(" at ", " ");
    let mut parts: Vec<&str> = text.split_whitespace().collect();
    let time = read_time(parts.pop()?)?;
    let now = tz.timestamp_opt(i64::try_from(now_s).ok()?, 0).single()?;
    let date = match parts.as_slice() {
        [] => {
            let today = now.date_naive();
            let at = tz.from_local_datetime(&today.and_time(time)).earliest()?;
            if at.timestamp() + 60 < now_s as i64 {
                today.succ_opt()?
            } else {
                today
            }
        }
        [month, day] => {
            let (month, day) = (month_number(month)?, day.parse::<u32>().ok()?);
            let this_year = NaiveDate::from_ymd_opt(now.year(), month, day)?;
            let at = tz.from_local_datetime(&this_year.and_time(time)).earliest()?;
            if at.timestamp() < now_s as i64 - 180 * 86400 {
                NaiveDate::from_ymd_opt(now.year() + 1, month, day)?
            } else {
                this_year
            }
        }
        _ => return None,
    };
    let at = tz.from_local_datetime(&date.and_time(time)).earliest()?;
    u64::try_from(at.timestamp()).ok()
}

/// The `rate_limit_info` the answer is worth, in the shape of the stream's `unifiedWindows`; None when no window could be read.
pub fn to_info<Tz: TimeZone>(text: &str, now_s: u64, tz: &Tz) -> Option<Value> {
    let mut windows = serde_json::Map::new();
    for line in text.lines() {
        let Some((key, util, when)) = read_line(line) else { continue };
        let Some(resets_at) = read_reset(when, now_s, tz) else { continue };
        windows.insert(key.to_owned(), json!({"utilization": util, "resetsAt": resets_at, "status": ""}));
    }
    (!windows.is_empty()).then(|| json!({"unifiedWindows": windows}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn seoul() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).unwrap()
    }

    fn ts(y: i32, m: u32, d: u32, h: u32, min: u32) -> u64 {
        seoul().with_ymd_and_hms(y, m, d, h, min, 0).single().unwrap().timestamp() as u64
    }

    /// Nov 20 2026, 14:00 in Seoul.
    fn now() -> u64 {
        ts(2026, 11, 20, 14, 0)
    }

    fn at(m: u32, d: u32, h: u32, min: u32) -> u64 {
        ts(2026, m, d, h, min)
    }

    const SAMPLE: &str = "You are currently using your subscription to power your Claude Code usage\n\nCurrent session: 68% used · resets Nov 20, 6:39pm (Asia/Seoul)\nCurrent week (all models): 81% used · resets Nov 24, 4:59pm (Asia/Seoul)\n\nWhat's contributing to your usage?\n  82%  Start fresh when you switch tasks";

    #[test]
    fn the_answer_the_program_gave_is_read_into_windows() {
        let info = to_info(SAMPLE, now(), &seoul()).unwrap();
        let w = &info["unifiedWindows"];
        assert_eq!((w["five_hour"]["utilization"].as_f64(), w["five_hour"]["resetsAt"].as_u64()), (Some(0.68), Some(at(11, 20, 18, 39))));
        assert_eq!((w["seven_day"]["utilization"].as_f64(), w["seven_day"]["resetsAt"].as_u64()), (Some(0.81), Some(at(11, 24, 16, 59))));
        // The lines about what contributes are not windows.
        assert_eq!(w.as_object().unwrap().len(), 2);
    }

    #[test]
    fn a_time_without_a_day_is_today_or_tomorrow_and_noon_and_midnight_are_right() {
        let t = |s: &str| read_reset(s, now(), &seoul());
        assert_eq!(t("6:39pm"), Some(at(11, 20, 18, 39)));
        assert_eq!(t("6pm (Asia/Seoul)"), Some(at(11, 20, 18, 0)));
        assert_eq!(t("1:30pm"), Some(at(11, 21, 13, 30)), "already past: tomorrow");
        assert_eq!(t("12am"), Some(at(11, 21, 0, 0)));
        assert_eq!(t("12pm"), Some(at(11, 21, 12, 0)));
        assert_eq!(t("Nov 21 at 12:05AM"), Some(at(11, 21, 0, 5)));
        for bad in ["", "soon", "Nov 20", "25pm", "13:00pm", "6:75pm", "Foo 20, 6pm", "Nov 31, 6pm", "Nov 20, 21, 6pm"] {
            assert_eq!(t(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_date_that_is_half_a_year_back_is_next_year() {
        // Read on Dec 31 2026, "Jan 1" has passed this year by 360 days, so it is next year's; "Dec 31" is today's.
        let dec = ts(2026, 12, 31, 20, 0);
        assert_eq!(read_reset("Jan 1, 2am", dec, &seoul()), Some(ts(2027, 1, 1, 2, 0)));
        assert_eq!(read_reset("Dec 31, 11pm", dec, &seoul()), Some(ts(2026, 12, 31, 23, 0)));
        let jan = ts(2027, 1, 2, 10, 0);
        assert_eq!(read_reset("Jan 3, 9am", jan, &seoul()), Some(ts(2027, 1, 3, 9, 0)));
    }

    #[test]
    fn models_are_named_and_what_is_not_known_or_not_readable_is_left_out() {
        let text = "Current week (Opus): 10% used · resets Nov 24, 4:59pm (x)\nCurrent week (Sonnet only): 20% used · resets Nov 24, 4:59pm\nCurrent week (Fable): 30% used · resets Nov 24, 4:59pm\nCurrent session: lots used · resets 6pm\nCurrent session: 50% used · resets whenever";
        let info = to_info(text, now(), &seoul()).unwrap();
        let keys: Vec<&str> = info["unifiedWindows"].as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["seven_day_opus", "seven_day_sonnet"]);
        assert!(to_info("nothing here\nCurrent session: ?", now(), &seoul()).is_none());
        assert!(to_info("", now(), &seoul()).is_none());
    }

    #[test]
    fn the_result_is_the_last_result_line_unless_it_was_an_error() {
        let ok = "{\"type\":\"system\",\"subtype\":\"init\"}\n{\"type\":\"result\",\"is_error\":false,\"result\":\"hello\"}\n";
        assert_eq!(result_text(ok.as_bytes()).as_deref(), Some("hello"));
        assert_eq!(result_text(b"{\"type\":\"result\",\"is_error\":true,\"result\":\"boom\"}"), None);
        assert_eq!(result_text(b"not json"), None);
        assert_eq!(result_text(b"{\"type\":\"result\",\"is_error\":false}"), None);
    }
}
