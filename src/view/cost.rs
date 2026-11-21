//! The cost block of the usage card, from the ledger (`costs`): today, the last seven days and this month of the account the card shows,
//! and how near the budgets are. The sums of an account are its own; a budget counts everything (every account and provider), since
//! it is what the person is willing to spend. Every number is an estimate and is said so where it is shown.
use chrono::{Duration, NaiveDate};

use crate::costs::Days;

pub struct CostView {
    pub today: String,
    pub week: String,
    pub month: String,
    /// ok, warn, limit or "" (no budget).
    pub level: &'static str,
    /// "Daily budget 45% · monthly 12%", or "" when there is no budget.
    pub budget_line: String,
    pub has_data: bool,
}

/// `$0.03`, `$12.4`, `<$0.01`, and `$0.00` for nothing.
pub fn usd(value: f64) -> String {
    if value <= 0.0 {
        "$0.00".into()
    } else if value < 0.01 {
        "<$0.01".into()
    } else if value < 100.0 {
        format!("${value:.2}")
    } else {
        format!("${value:.0}")
    }
}

fn sum_where(days: &Days, key: Option<&str>, keep: impl Fn(&str) -> bool) -> f64 {
    days.iter().filter(|(day, _)| keep(day)).flat_map(|(_, keys)| keys.iter()).filter(|(k, _)| key.is_none_or(|want| k.as_str() == want)).map(|(_, v)| *v).sum()
}

/// What the account `key` (None: all) cost today, in the last seven days (today included) and in the calendar month of `today`.
pub fn sums(days: &Days, key: Option<&str>, today: &str) -> (f64, f64, f64) {
    let week_from = NaiveDate::parse_from_str(today, "%Y-%m-%d").ok().and_then(|d| d.checked_sub_signed(Duration::days(6))).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_else(|| today.to_owned());
    let month = today.get(..7).unwrap_or(today).to_owned();
    (
        sum_where(days, key, |d| d == today),
        sum_where(days, key, |d| d >= week_from.as_str() && d <= today),
        sum_where(days, key, |d| d.starts_with(&month)),
    )
}

/// How full a budget is (None without one).
fn share(spent: f64, budget: Option<f64>) -> Option<f64> {
    budget.filter(|b| *b > 0.0).map(|b| spent / b)
}

fn level_of(share: f64) -> &'static str {
    if share >= 1.0 {
        "limit"
    } else if share >= 0.8 {
        "warn"
    } else {
        "ok"
    }
}

pub fn cost_view(days: &Days, key: &str, today: &str, daily: Option<f64>, monthly: Option<f64>, lang: crate::view::i18n::Lang) -> CostView {
    use crate::view::i18n::{fill, tr};
    let (t, w, m) = sums(days, Some(key), today);
    // The budgets count everything the person's chats cost, not one account.
    let (all_today, _, all_month) = sums(days, None, today);
    let (d_share, m_share) = (share(all_today, daily), share(all_month, monthly));
    let level = [d_share, m_share].into_iter().flatten().fold(None::<f64>, |acc, s| Some(acc.map_or(s, |a| a.max(s)))).map_or("", level_of);
    let mut bits = Vec::new();
    if let Some(s) = d_share {
        bits.push(fill(&tr(lang, "cost_budget_day"), &[("pct", format!("{:.0}", s * 100.0)), ("of", usd(daily.unwrap_or(0.0)))]));
    }
    if let Some(s) = m_share {
        bits.push(fill(&tr(lang, "cost_budget_month"), &[("pct", format!("{:.0}", s * 100.0)), ("of", usd(monthly.unwrap_or(0.0)))]));
    }
    CostView { today: usd(t), week: usd(w), month: usd(m), level, budget_line: bits.join(" · "), has_data: !days.is_empty() }
}

/// The budget periods that are used up (the spend reached the budget) as `(is_month, period id)`: the day's date, or the month `YYYY-MM`.
pub fn crossed(days: &Days, today: &str, daily: Option<f64>, monthly: Option<f64>) -> Vec<(bool, String)> {
    let (all_today, _, all_month) = sums(days, None, today);
    let mut out = Vec::new();
    if share(all_today, daily).is_some_and(|s| s >= 1.0) {
        out.push((false, today.to_owned()));
    }
    if share(all_month, monthly).is_some_and(|s| s >= 1.0) {
        out.push((true, today.get(..7).unwrap_or(today).to_owned()));
    }
    out
}

/// Of the used-up periods (`crossed`), those the person was not told about yet: `told_day` and `told_month` are the day and the month
/// that were last told.
pub fn untold(crossed: &[(bool, String)], told_day: Option<&str>, told_month: Option<&str>) -> Vec<(bool, String)> {
    crossed.iter().filter(|(month, id)| Some(id.as_str()) != if *month { told_month } else { told_day }).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::i18n::Lang;

    fn days(list: &[(&str, &str, f64)]) -> Days {
        let mut d = Days::new();
        for (day, key, v) in list {
            d.entry((*day).to_owned()).or_default().insert((*key).to_owned(), *v);
        }
        d
    }

    #[test]
    fn today_the_last_seven_days_and_the_month_are_summed_for_one_account() {
        let d = days(&[
            ("2026-11-21", "", 1.0),
            ("2026-11-21", "p:air", 5.0),
            ("2026-11-15", "", 2.0), // six days back: in the week
            ("2026-11-14", "", 4.0), // seven days back: out of it
            ("2026-11-01", "", 8.0),
            ("2026-10-31", "", 16.0), // last month
        ]);
        let (t, w, m) = sums(&d, Some(""), "2026-11-21");
        assert_eq!((t, w, m), (1.0, 3.0, 15.0));
        assert_eq!(sums(&d, Some("p:air"), "2026-11-21"), (5.0, 5.0, 5.0));
        // Across a month's edge the week still counts the days of the month before.
        assert_eq!(sums(&d, Some(""), "2026-11-02").1, 8.0 + 16.0);
    }

    #[test]
    fn a_budget_counts_everything_and_warns_at_eighty_and_stops_at_a_hundred_percent() {
        let d = days(&[("2026-11-21", "", 3.0), ("2026-11-21", "p:air", 5.0)]);
        let v = cost_view(&d, "", "2026-11-21", Some(10.0), None, Lang::En);
        assert_eq!(v.level, "warn", "8 of 10 over all keys, though the account itself spent 3");
        assert!(v.budget_line.contains("80%") && v.budget_line.contains("$10.00"), "{}", v.budget_line);
        assert_eq!(cost_view(&d, "", "2026-11-21", Some(8.0), None, Lang::En).level, "limit");
        assert_eq!(cost_view(&d, "", "2026-11-21", Some(100.0), None, Lang::En).level, "ok");
        let none = cost_view(&d, "", "2026-11-21", None, None, Lang::En);
        assert_eq!((none.level, none.budget_line.as_str()), ("", ""));
        assert!(!cost_view(&Days::new(), "", "2026-11-21", None, None, Lang::En).has_data);
    }

    #[test]
    fn used_up_budgets_are_named_once_per_period() {
        let d = days(&[("2026-11-20", "", 20.0), ("2026-11-21", "", 6.0)]);
        assert_eq!(crossed(&d, "2026-11-21", Some(5.0), Some(30.0)), [(false, "2026-11-21".to_owned())]);
        assert_eq!(crossed(&d, "2026-11-21", None, Some(26.0)), [(true, "2026-11".to_owned())]);
        assert!(crossed(&d, "2026-11-21", Some(7.0), Some(30.0)).is_empty());
        assert!(crossed(&d, "2026-11-21", Some(0.0), None).is_empty(), "a zero budget is no budget");
    }

    #[test]
    fn a_period_is_told_once_and_the_next_day_or_month_is_news_again() {
        let c = vec![(false, "2026-11-21".to_owned()), (true, "2026-11".to_owned())];
        assert_eq!(untold(&c, None, None), c);
        assert_eq!(untold(&c, Some("2026-11-21"), None), [(true, "2026-11".to_owned())]);
        assert!(untold(&c, Some("2026-11-21"), Some("2026-11")).is_empty());
        // Tomorrow the day's budget is a new period; the month's was told already.
        let tomorrow = vec![(false, "2026-11-22".to_owned()), (true, "2026-11".to_owned())];
        assert_eq!(untold(&tomorrow, Some("2026-11-21"), Some("2026-11")), [(false, "2026-11-22".to_owned())]);
    }

    #[test]
    fn money_reads_short() {
        assert_eq!((usd(0.0).as_str(), usd(0.004).as_str(), usd(0.0287).as_str(), usd(12.345).as_str(), usd(1234.5).as_str()), ("$0.00", "<$0.01", "$0.03", "$12.35", "$1234"));
    }
}
