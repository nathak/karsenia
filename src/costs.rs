//! What the chats cost, by day and by account or provider: the sums the cost card and the budgets read. A cost is the program's own
//! estimate (`total_cost_usd`, from a price table it carries; for a subscription it is the price the same use would have at the API's,
//! not a bill), so every number here is called an estimate where it is shown.
//!
//! In a long-lived `claude` (our way of running it) the `total_cost_usd` of each result is the running total of the process so far, plus
//! what was restored when it resumed a session; a restart or a `/clear` begins again. So a turn's cost is the difference from the
//! result before (`turn_cost`), and what is kept is only these differences, as numbers: no text of a chat is stored.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;

/// Days kept (the oldest go first), keys in a day, and the largest sum a key may hold in a day.
const MAX_DAYS: usize = 100;
const MAX_KEYS: usize = 96;
const MAX_VALUE: f64 = 1_000_000.0;

/// What one result of a chat says about the turn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Nothing to count, and the baseline stays (no number, an out-of-range one, or the zeroed result of a crash).
    Skip,
    /// The turn cost `delta`; the baseline is now `total`.
    Count { delta: f64, total: f64 },
}

fn sane(value: f64) -> bool {
    value.is_finite() && (0.0..=MAX_VALUE).contains(&value)
}

/// The cost of one turn from a result and the baseline (the running total of the result before; None for a chat that came from a
/// session whose earlier spend we never saw: its first result sets the baseline and counts nothing).
pub fn turn_cost(base: Option<f64>, raw: &Value) -> Step {
    let Some(total) = raw.get("total_cost_usd").and_then(Value::as_f64).filter(|t| sane(*t)) else { return Step::Skip };
    // A crash may end with every number zeroed: that is no turn, and it must not be taken as a restart.
    if raw.get("is_error").and_then(Value::as_bool) == Some(true) && total == 0.0 {
        return Step::Skip;
    }
    let delta = match base {
        None => 0.0,
        Some(b) if total >= b => total - b,
        // A restarted process (or a `/clear`) counts from its own beginning.
        Some(_) => total,
    };
    Step::Count { delta, total }
}

/// `YYYY-MM-DD`.
pub fn valid_day(day: &str) -> bool {
    let b = day.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// "": the default account; an account's id; `p:` and a provider's id.
pub fn valid_key(key: &str) -> bool {
    key.is_empty()
        || crate::accounts::valid_id(key)
        || key.strip_prefix("p:").is_some_and(|id| (1..=40).contains(&id.len()) && id.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-'))
}

/// By day, then by key.
pub type Days = BTreeMap<String, BTreeMap<String, f64>>;

#[derive(Serialize, Deserialize, Default)]
struct File {
    v: u32,
    days: Days,
}

/// Drops what is not valid, and the days past the newest `MAX_DAYS`.
fn clean(days: Days) -> Days {
    let mut days: Days = days
        .into_iter()
        .filter(|(day, _)| valid_day(day))
        .map(|(day, keys)| (day, keys.into_iter().filter(|(k, v)| valid_key(k) && sane(*v)).take(MAX_KEYS).collect::<BTreeMap<_, _>>()))
        .collect();
    while days.len() > MAX_DAYS {
        let oldest = days.keys().next().cloned().unwrap_or_default();
        days.remove(&oldest);
    }
    days
}

pub struct CostStore {
    path: PathBuf,
    days: Mutex<Days>,
    events: broadcast::Sender<Value>,
}

impl CostStore {
    /// Reads `path`; a file that is missing or not understood starts an empty store (its content is never logged).
    pub fn open(path: &Path, events: broadcast::Sender<Value>) -> Self {
        let days = match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<File>(&text) {
                Ok(file) => clean(file.days),
                Err(_) => {
                    tracing::warn!("costs.json is not understood and is started again");
                    Days::new()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Days::new(),
            Err(error) => {
                tracing::warn!("costs.json could not be read: {}", error.kind());
                Days::new()
            }
        };
        Self { path: path.to_owned(), days: Mutex::new(days), events }
    }

    /// Adds `delta` to `key` on `day`. False (nothing changed) for a delta that is zero or not sane, a key or day that is not valid, a
    /// sum that would pass the limit, or a day that has no room for another key. Tells the pages when it changed something.
    pub fn add(&self, key: &str, day: &str, delta: f64) -> bool {
        if delta <= 0.0 || !sane(delta) || !valid_key(key) || !valid_day(day) {
            return false;
        }
        {
            // The file is written while the map is held, so two chats cannot save their snapshots in the wrong order.
            let mut days = self.days.lock().unwrap_or_else(|p| p.into_inner());
            let keys = days.entry(day.to_owned()).or_default();
            if !keys.contains_key(key) && keys.len() >= MAX_KEYS {
                return false;
            }
            let sum = keys.get(key).copied().unwrap_or(0.0) + delta;
            if !sane(sum) {
                return false;
            }
            keys.insert(key.to_owned(), sum);
            *days = clean(std::mem::take(&mut *days));
            // A clock far in the past puts the day beyond the newest `MAX_DAYS`: it is cut at once, and nothing was counted.
            if !days.get(day).is_some_and(|keys| keys.contains_key(key)) {
                return false;
            }
            self.save(&days);
        }
        let _ = self.events.send(json!({"v": 1, "kind": "cost_changed", "data": {}}));
        true
    }

    /// Replaces the file whole (written beside it, then renamed over it).
    fn save(&self, days: &Days) {
        let Ok(text) = serde_json::to_string(&File { v: 1, days: days.clone() }) else { return };
        let tmp = self.path.with_extension("json.tmp");
        let written = crate::private::write(&tmp, &text).and_then(|()| std::fs::rename(&tmp, &self.path));
        if let Err(error) = written {
            tracing::warn!("costs.json was not saved: {error}");
        }
    }

    pub fn snapshot(&self) -> Days {
        self.days.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// An account that was removed: its sums go.
    pub fn forget(&self, key: &str) {
        let changed = {
            let mut days = self.days.lock().unwrap_or_else(|p| p.into_inner());
            let mut changed = false;
            for keys in days.values_mut() {
                changed |= keys.remove(key).is_some();
            }
            if changed {
                self.save(&days);
            }
            changed
        };
        if changed {
            let _ = self.events.send(json!({"v": 1, "kind": "cost_changed", "data": {}}));
        }
    }
}

/// Today's date where the person is (the zone of this computer).
pub fn local_day() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(total: f64) -> Value {
        json!({"type": "result", "subtype": "success", "is_error": false, "total_cost_usd": total})
    }

    #[test]
    fn a_turn_costs_the_difference_from_the_result_before() {
        assert_eq!(turn_cost(Some(0.0), &result(0.1)), Step::Count { delta: 0.1, total: 0.1 });
        match turn_cost(Some(0.1), &result(0.3)) {
            Step::Count { delta, total } => assert!((delta - 0.2).abs() < 1e-9 && total == 0.3),
            other => panic!("{other:?}"),
        }
        // The same total again is a turn that cost nothing (and the baseline is the same).
        assert_eq!(turn_cost(Some(0.3), &result(0.3)), Step::Count { delta: 0.0, total: 0.3 });
        // A restarted process or a `/clear` begins again: the total is the turn.
        assert_eq!(turn_cost(Some(0.9), &result(0.2)), Step::Count { delta: 0.2, total: 0.2 });
        // A chat from a session we never saw: the first result sets the baseline and counts nothing.
        assert_eq!(turn_cost(None, &result(5.0)), Step::Count { delta: 0.0, total: 5.0 });
    }

    #[test]
    fn what_is_not_a_cost_or_is_a_crashs_zeroes_is_skipped() {
        for bad in [json!({"type": "result"}), json!({"total_cost_usd": "1"}), json!({"total_cost_usd": -1.0}), json!({"total_cost_usd": 1e9}), json!({"total_cost_usd": null})] {
            assert_eq!(turn_cost(Some(1.0), &bad), Step::Skip, "{bad}");
        }
        // The result of a crash with zeroed numbers is no restart: the baseline stays.
        assert_eq!(turn_cost(Some(1.0), &json!({"is_error": true, "total_cost_usd": 0.0})), Step::Skip);
        // An error result with a real cost counts like any other.
        assert!(matches!(turn_cost(Some(1.0), &json!({"is_error": true, "total_cost_usd": 1.5})), Step::Count { .. }));
    }

    fn store(dir: &Path) -> CostStore {
        let (tx, _rx) = broadcast::channel(8);
        CostStore::open(&dir.join("costs.json"), tx)
    }

    #[test]
    fn sums_are_kept_by_day_and_key_validated_and_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        assert!(s.add("", "2026-11-21", 0.25));
        assert!(s.add("", "2026-11-21", 0.25));
        assert!(s.add("p:air", "2026-11-21", 1.0));
        assert!(s.add("", "2026-11-22", 2.0));
        for (key, day, delta) in [("", "2026-11-21", 0.0), ("", "2026-11-21", -1.0), ("", "2026-11-21", f64::NAN), ("../x", "2026-11-21", 1.0), ("p:Bad Id", "2026-11-21", 1.0), ("", "21-11-2026", 1.0), ("", "2026-11-21", 2e6)] {
            assert!(!s.add(key, day, delta), "{key:?} {day:?} {delta}");
        }
        let snap = s.snapshot();
        assert_eq!(snap["2026-11-21"][""], 0.5);
        assert_eq!(snap["2026-11-21"]["p:air"], 1.0);
        assert_eq!(snap["2026-11-22"][""], 2.0);
        // Reopened: the same, and the file is private.
        let again = store(dir.path());
        assert_eq!(again.snapshot(), snap);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.path().join("costs.json")).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn only_the_newest_hundred_days_and_a_bounded_number_of_keys_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        for i in 0..101u32 {
            assert!(s.add("", &format!("2026-{:02}-{:02}", i / 28 % 12 + 1, i % 28 + 1), 0.01));
        }
        let snap = s.snapshot();
        assert!(snap.len() <= MAX_DAYS, "{}", snap.len());
        for i in 0..MAX_KEYS {
            assert!(s.add(&format!("p:k{i}"), "2027-01-01", 0.01), "{i}");
        }
        assert!(!s.add("p:one-too-many", "2027-01-01", 0.01));
    }

    #[test]
    fn a_file_that_is_wrong_starts_empty_and_forged_entries_are_dropped_when_it_is_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("costs.json"), "not json").unwrap();
        assert!(store(dir.path()).snapshot().is_empty());
        std::fs::write(dir.path().join("costs.json"), r#"{"v":1,"days":{"2026-11-21":{"":1.5,"../x":3,"p:ok":-2,"p:fine":1e12},"yesterday":{"":9}}}"#).unwrap();
        let snap = store(dir.path()).snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap["2026-11-21"].len(), 1);
        assert_eq!(snap["2026-11-21"][""], 1.5);
    }

    #[test]
    fn a_removed_account_loses_its_sums_and_many_threads_lose_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let s = std::sync::Arc::new(store(dir.path()));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let s = s.clone();
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        assert!(s.add("", "2026-11-21", 0.5));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(s.snapshot()["2026-11-21"][""], 100.0);
        assert_eq!(store(dir.path()).snapshot()["2026-11-21"][""], 100.0, "the file is whole");
        s.add("0123456789abcdef", "2026-11-21", 1.0);
        s.forget("0123456789abcdef");
        assert!(!s.snapshot()["2026-11-21"].contains_key("0123456789abcdef"));
    }

    #[test]
    fn the_local_day_is_a_valid_day() {
        assert!(valid_day(&local_day()));
    }
}
