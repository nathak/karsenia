//! The limits Claude reports for each login: the 5-hour window, the weekly one and the weekly ones of a model. The sources are the
//! `rate_limit_event` a running `claude` prints in its stream, so a value is as fresh as the last turn a chat of that account ran, and,
//! when the person asks, what the program's own `/usage` command says (see `usage_cli`; the program asks, the app asks Anthropic nothing:
//! no token leaves the app). The default account is kept under the empty name.
//!
//! What a stream says is not trusted for size or shape: a window's name is a short lowercase word, a value is a finite number in a
//! sane range, a reset time is a sane date, and an account holds a few windows. A chat of a provider is never counted (it has no
//! Anthropic limits), and nothing a chat wrote is stored here, only these numbers.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;

/// The windows `claude` names (anything else it may add is kept too, up to `MAX_KEYS`).
pub const KNOWN: [&str; 6] = ["five_hour", "seven_day", "seven_day_opus", "seven_day_sonnet", "seven_day_overage_included", "overage"];
const MAX_KEYS: usize = 12;
const MAX_ACCOUNTS: usize = 64;
/// Latest reset time taken at all (the year 2200, in seconds), checked before any arithmetic on it.
const MAX_RESET_S: u64 = 7_258_118_400;
/// A reset time further back than this is not a window.
const MAX_AGE_S: u64 = 35 * 24 * 3600;
/// Time a window stays listed as "reset" before it is dropped to make room (seconds), and the slack allowed for clocks.
const RESET_SLACK_S: u64 = 60;
/// Two reset times this close are one window: `/usage` says the minute (18:40) where the stream says the second (18:39:42).
const SAME_WINDOW_S: u64 = 90;
/// A value seen again unchanged is written back only when its time is this old.
const REFRESH_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Win {
    /// The fraction of the window used (above 1 when it ran past its cap); None when the stream gave no number.
    pub util: Option<f64>,
    /// Unix seconds.
    pub resets_at: u64,
    /// `allowed`, `allowed_warning`, `rejected` or "".
    pub status: String,
    /// When it was last seen, ms.
    pub seen_at: u64,
}

pub type AccountUsage = BTreeMap<String, Win>;

#[derive(Serialize, Deserialize, Default)]
struct File {
    v: u32,
    accounts: HashMap<String, AccountUsage>,
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 40 && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn clean_util(value: &Value) -> Option<f64> {
    value.as_f64().filter(|v| v.is_finite() && (0.0..=10.0).contains(v))
}

fn clean_status(value: &Value) -> String {
    match value.as_str() {
        Some(s @ ("allowed" | "allowed_warning" | "rejected")) => s.to_owned(),
        _ => String::new(),
    }
}

/// A reset time in unix seconds, when it is a whole number that can be one.
/// How far ahead a window of this kind can reset: a window that claims more is not one, and one forged far ahead would keep out the
/// real ones (an earlier reset is taken for an older event).
fn horizon_s(key: &str) -> u64 {
    match key {
        "five_hour" => 6 * 3600,
        k if k.starts_with("seven_day") => 8 * 24 * 3600,
        _ => MAX_AGE_S,
    }
}

fn plausible(reset: u64, key: &str, now_s: u64) -> bool {
    reset < MAX_RESET_S && reset.saturating_add(MAX_AGE_S) > now_s && reset <= now_s.saturating_add(horizon_s(key))
}

fn clean_reset(value: &Value, key: &str, now_s: u64) -> Option<u64> {
    value.as_u64().filter(|r| plausible(*r, key, now_s))
}

/// Takes what one `rate_limit_info` says into `cur`. Returns whether anything worth saving changed.
///
/// A window is replaced by one that resets later; one that resets at the same time keeps the larger use (it cannot fall inside a
/// window) and takes the newer status; one that resets earlier is an older event (two chats can finish in either order) and is
/// ignored. The per-window form (`unifiedWindows`) wins over the single one (`rateLimitType`) of the same event.
pub fn merge(cur: &mut AccountUsage, info: &Value, now_ms: u64) -> bool {
    let now_s = now_ms / 1000;
    let mut incoming: Vec<(String, Option<f64>, u64, String)> = Vec::new();
    if let Some(windows) = info["unifiedWindows"].as_object() {
        for (key, w) in windows {
            if let (true, Some(reset)) = (valid_key(key), clean_reset(&w["resetsAt"], key, now_s)) {
                incoming.push((key.clone(), clean_util(&w["utilization"]), reset, clean_status(&w["status"])));
            }
        }
    }
    if let Some(key) = info["rateLimitType"].as_str().filter(|k| valid_key(k) && !incoming.iter().any(|(have, ..)| have == k)) {
        if let Some(reset) = clean_reset(&info["resetsAt"], key, now_s) {
            incoming.push((key.to_owned(), clean_util(&info["utilization"]), reset, clean_status(&info["status"])));
        }
    }
    let mut changed = false;
    for (key, util, resets_at, status) in incoming {
        match cur.get_mut(&key) {
            Some(old) if resets_at.abs_diff(old.resets_at) > SAME_WINDOW_S && resets_at < old.resets_at => {}
            Some(old) if resets_at.abs_diff(old.resets_at) <= SAME_WINDOW_S => {
                old.resets_at = old.resets_at.max(resets_at);
                let merged = match (old.util, util) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => b.or(a),
                };
                let status = if status.is_empty() { old.status.clone() } else { status };
                if merged != old.util || status != old.status || now_ms.saturating_sub(old.seen_at) > REFRESH_MS {
                    changed = true;
                }
                old.util = merged;
                old.status = status;
                old.seen_at = now_ms;
            }
            Some(old) => {
                *old = Win { util, resets_at, status, seen_at: now_ms };
                changed = true;
            }
            None => {
                if cur.len() >= MAX_KEYS && !KNOWN.contains(&key.as_str()) {
                    // Windows that are over make room first.
                    cur.retain(|_, w| w.resets_at + RESET_SLACK_S >= now_s);
                    if cur.len() >= MAX_KEYS {
                        continue;
                    }
                }
                cur.insert(key, Win { util, resets_at, status, seen_at: now_ms });
                changed = true;
            }
        }
    }
    changed
}

pub struct UsageStore {
    path: PathBuf,
    map: Mutex<HashMap<String, AccountUsage>>,
    events: broadcast::Sender<Value>,
    /// By account: when `/usage` was last asked (ms) and whether an ask is running now.
    probes: Mutex<HashMap<String, (u64, bool)>>,
}

/// Why an ask for the usage was not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// One is running for this account.
    Busy,
    /// The last one started less than `PROBE_GAP_MS` ago.
    Soon,
}

/// The least time between two asks for one account.
pub const PROBE_GAP_MS: u64 = 60_000;

/// Held while an ask runs: it keeps a second one out, and ends that when dropped (an ask that fails or is cut off included).
pub struct ProbeGuard<'a> {
    store: &'a UsageStore,
    key: String,
}

impl Drop for ProbeGuard<'_> {
    fn drop(&mut self) {
        if let Some(p) = self.store.probes.lock().unwrap_or_else(|p| p.into_inner()).get_mut(&self.key) {
            p.1 = false;
        }
    }
}

impl UsageStore {
    /// Reads `path`; a file that is missing or not understood starts an empty store (its content is never logged).
    pub fn open(path: &Path, events: broadcast::Sender<Value>) -> Self {
        Self::open_at(path, events, crate::now_ms())
    }

    /// `open`, judging what the file holds against `now_ms`.
    pub fn open_at(path: &Path, events: broadcast::Sender<Value>, now_ms: u64) -> Self {
        let mut map: HashMap<String, AccountUsage> = HashMap::new();
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<File>(&text) {
                Ok(file) => {
                    let now_s = now_ms / 1000;
                    for (account, windows) in file.accounts.into_iter().take(MAX_ACCOUNTS) {
                        // The file is read as the stream is: a date that cannot be a window's is not kept.
                        let windows: AccountUsage = windows.into_iter().filter(|(k, w)| valid_key(k) && plausible(w.resets_at, k, now_s)).take(MAX_KEYS).collect();
                        map.insert(account, windows);
                    }
                }
                Err(_) => tracing::warn!("usage.json is not understood and is started again"),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!("usage.json could not be read: {}", error.kind()),
        }
        Self { path: path.to_owned(), map: Mutex::new(map), events, probes: Mutex::new(HashMap::new()) }
    }

    /// Leave to ask `/usage` for `account` ("" is the default one): not while one runs, not within a minute of the last one.
    pub fn begin_probe(&self, account: &str, now_ms: u64) -> Result<ProbeGuard<'_>, Skip> {
        let mut probes = self.probes.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, running)) = probes.get(account) {
            if *running {
                return Err(Skip::Busy);
            }
            if now_ms.saturating_sub(*at) < PROBE_GAP_MS {
                return Err(Skip::Soon);
            }
        }
        if probes.len() >= MAX_ACCOUNTS && !probes.contains_key(account) {
            // Only entries that are over (not running, and a minute old) make room; when none is, this one waits.
            probes.retain(|_, (at, running)| *running || now_ms.saturating_sub(*at) < PROBE_GAP_MS);
            if probes.len() >= MAX_ACCOUNTS {
                return Err(Skip::Busy);
            }
        }
        probes.insert(account.to_owned(), (now_ms, true));
        Ok(ProbeGuard { store: self, key: account.to_owned() })
    }

    /// Takes one event of a chat of `account` (None: the default one). Tells the pages when it changed something.
    pub fn record(&self, account: Option<&str>, info: &Value, now_ms: u64) -> bool {
        let key = account.unwrap_or_default().to_owned();
        {
            // The file is written while the map is held, so two chats (or a removal) cannot save their snapshots in the wrong order.
            let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
            if !map.contains_key(&key) && map.len() >= MAX_ACCOUNTS {
                return false;
            }
            if !merge(map.entry(key).or_default(), info, now_ms) {
                return false;
            }
            self.save(&map);
        }
        let _ = self.events.send(json!({"v": 1, "kind": "usage_changed", "data": {"account": account}}));
        true
    }

    /// Replaces the file whole (written beside it, then renamed over it): a crash or a second writer never leaves half of one.
    fn save(&self, map: &HashMap<String, AccountUsage>) {
        let Ok(text) = serde_json::to_string(&File { v: 1, accounts: map.clone() }) else { return };
        let tmp = self.path.with_extension("json.tmp");
        let written = crate::private::write(&tmp, &text).and_then(|()| std::fs::rename(&tmp, &self.path));
        if let Err(error) = written {
            tracing::warn!("usage.json was not saved: {error}");
        }
    }

    pub fn snapshot(&self) -> HashMap<String, AccountUsage> {
        self.map.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// An account that was removed: its numbers go (and the pages are told).
    pub fn forget(&self, id: &str) {
        let removed = {
            let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
            let removed = map.remove(id).is_some();
            if removed {
                self.save(&map);
            }
            removed
        };
        if removed {
            let _ = self.events.send(json!({"v": 1, "kind": "usage_changed", "data": {"account": id}}));
        }
    }

    /// Keeps only the default account and these (a removal that an earlier run did not finish).
    pub fn retain(&self, ids: &HashSet<String>) {
        let gone: Vec<String> = self.map.lock().unwrap_or_else(|p| p.into_inner()).keys().filter(|k| !k.is_empty() && !ids.contains(*k)).cloned().collect();
        for id in gone {
            self.forget(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;
    const RESET: u64 = 1_800_003_600;

    fn unified() -> Value {
        json!({"status": "allowed", "rateLimitType": "five_hour", "unifiedWindows": {"five_hour": {"utilization": 0.85, "resetsAt": RESET, "status": "allowed_warning"}, "seven_day": {"utilization": 0.1, "resetsAt": RESET + 86400, "status": "allowed"}}})
    }

    #[test]
    fn both_forms_of_the_event_are_read_and_the_per_window_one_wins() {
        let mut cur = AccountUsage::new();
        assert!(merge(&mut cur, &unified(), NOW));
        assert_eq!((cur["five_hour"].util, cur["five_hour"].status.as_str()), (Some(0.85), "allowed_warning"));
        assert_eq!(cur["seven_day"].util, Some(0.1));
        // A single window (no numbers but the status) for another type.
        let mut single = AccountUsage::new();
        merge(&mut single, &json!({"status": "allowed", "rateLimitType": "seven_day_opus", "resetsAt": RESET, "utilization": 0.5}), NOW);
        assert_eq!(single["seven_day_opus"].util, Some(0.5));
        // The top-level type of the same event does not override its per-window entry.
        let mut both = AccountUsage::new();
        merge(&mut both, &json!({"rateLimitType": "five_hour", "utilization": 0.99, "resetsAt": RESET, "unifiedWindows": {"five_hour": {"utilization": 0.2, "resetsAt": RESET}}}), NOW);
        assert_eq!(both["five_hour"].util, Some(0.2));
    }

    #[test]
    fn odd_values_are_dropped_or_blanked() {
        let mut cur = AccountUsage::new();
        let info = json!({"unifiedWindows": {
            "five_hour": {"utilization": -1.0, "resetsAt": RESET},
            "seven_day": {"utilization": 11.0, "resetsAt": RESET, "status": "weird"},
            "seven_day_opus": {"utilization": 0.3, "resetsAt": 0},
            "seven_day_sonnet": {"utilization": 0.3, "resetsAt": "soon"},
            "Bad Key": {"utilization": 0.3, "resetsAt": RESET},
            "../x": {"utilization": 0.3, "resetsAt": RESET},
            "overage": {"utilization": 0.3, "resetsAt": 9_999_999_999_999u64}}});
        merge(&mut cur, &info, NOW);
        assert_eq!(cur.keys().map(String::as_str).collect::<Vec<_>>(), vec!["five_hour", "seven_day"]);
        assert_eq!(cur["five_hour"].util, None);
        assert_eq!((cur["seven_day"].util, cur["seven_day"].status.as_str()), (None, ""));
        // A reset time that cannot be one is refused before any sum is made with it (u64::MAX would overflow), and a window cannot
        // claim to reset far later than its kind allows (that would keep the real ones out).
        let mut far = AccountUsage::new();
        for reset in [u64::MAX, 4_102_444_800u64, RESET + 7 * 3600] {
            assert!(!merge(&mut far, &json!({"rateLimitType": "five_hour", "resetsAt": reset, "utilization": 1.0}), NOW), "{reset}");
        }
        assert!(far.is_empty());
        assert!(merge(&mut far, &json!({"rateLimitType": "seven_day", "resetsAt": NOW / 1000 + 7 * 86400, "utilization": 0.1}), NOW));
    }

    #[test]
    fn a_window_grows_resets_and_ignores_older_events() {
        let mut cur = AccountUsage::new();
        merge(&mut cur, &unified(), NOW);
        // Same reset time: the larger use stays and the status is the newer one.
        let lower = json!({"unifiedWindows": {"five_hour": {"utilization": 0.4, "resetsAt": RESET, "status": "allowed"}}});
        assert!(merge(&mut cur, &lower, NOW + 1000), "the status changed");
        assert_eq!((cur["five_hour"].util, cur["five_hour"].status.as_str()), (Some(0.85), "allowed"));
        // Seen again unchanged: nothing to save until the time is old.
        assert!(!merge(&mut cur, &lower, NOW + 2000));
        assert!(merge(&mut cur, &lower, NOW + 2000 + REFRESH_MS + 1));
        // A later reset time replaces it, an earlier one (an older event) is ignored.
        assert!(merge(&mut cur, &json!({"unifiedWindows": {"five_hour": {"utilization": 0.05, "resetsAt": RESET + 18000}}}), NOW + 70_000));
        assert_eq!((cur["five_hour"].util, cur["five_hour"].resets_at), (Some(0.05), RESET + 18000));
        assert!(!merge(&mut cur, &json!({"unifiedWindows": {"five_hour": {"utilization": 0.9, "resetsAt": RESET}}}), NOW + 80_000));
        assert_eq!(cur["five_hour"].util, Some(0.05));
    }

    #[test]
    fn a_stream_cannot_make_an_account_hold_many_windows() {
        let mut cur = AccountUsage::new();
        for n in 0..40 {
            merge(&mut cur, &json!({"rateLimitType": format!("seven_day_x{n}"), "resetsAt": RESET, "utilization": 0.1}), NOW);
        }
        assert_eq!(cur.len(), MAX_KEYS);
        // A window `claude` is known to use still gets in.
        assert!(merge(&mut cur, &json!({"rateLimitType": "five_hour", "resetsAt": RESET, "utilization": 0.1}), NOW));
        assert!(cur.contains_key("five_hour"));
    }

    #[test]
    fn the_store_saves_tells_the_pages_forgets_and_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let (tx, mut rx) = broadcast::channel(8);
        let store = UsageStore::open_at(&path, tx.clone(), NOW);
        assert!(store.record(Some("acc1"), &unified(), NOW));
        assert!(!store.record(Some("acc1"), &unified(), NOW + 1));
        assert!(store.record(None, &unified(), NOW));
        let event = rx.try_recv().unwrap();
        assert_eq!((event["kind"].as_str(), event["data"]["account"].as_str()), (Some("usage_changed"), Some("acc1")));
        let again = UsageStore::open_at(&path, tx.clone(), NOW);
        assert_eq!(again.snapshot()["acc1"]["five_hour"].util, Some(0.85));
        assert!(again.snapshot().contains_key(""));
        again.retain(&HashSet::new());
        assert!(!again.snapshot().contains_key("acc1") && again.snapshot().contains_key(""));
        assert!(!UsageStore::open_at(&path, tx, NOW).snapshot().contains_key("acc1"), "the removal was saved");
        // A date a window cannot have is not read back from the file either, so a forged one cannot stay.
        std::fs::write(&path, r#"{"v":1,"accounts":{"":{"five_hour":{"util":1.0,"resets_at":4102444800,"status":"rejected","seen_at":1},"seven_day":{"util":0.2,"resets_at":RESET_NOW,"status":"allowed","seen_at":1}}}}"#.replace("RESET_NOW", &(NOW / 1000 + 86400).to_string())).unwrap();
        let loaded = UsageStore::open_at(&path, broadcast::channel(1).0, NOW).snapshot();
        assert_eq!(loaded[""].keys().map(String::as_str).collect::<Vec<_>>(), vec!["seven_day"]);
        // A file that is not understood starts empty.
        std::fs::write(&path, "{not json").unwrap();
        assert!(UsageStore::open_at(&path, broadcast::channel(1).0, NOW).snapshot().is_empty());
    }

    #[test]
    fn a_reset_time_within_a_minute_and_a_half_is_the_same_window_whichever_comes_first() {
        let mut cur = AccountUsage::new();
        let reset = RESET;
        // The stream gives the second, `/usage` the minute (before or after it): one window, the larger use stays.
        assert!(merge(&mut cur, &json!({"unifiedWindows": {"five_hour": {"utilization": 0.4, "resetsAt": reset + 42, "status": "allowed"}}}), NOW));
        merge(&mut cur, &json!({"unifiedWindows": {"five_hour": {"utilization": 1.0, "resetsAt": reset + 60, "status": ""}}}), NOW + 1000);
        assert_eq!((cur["five_hour"].util, cur["five_hour"].resets_at), (Some(1.0), reset + 60));
        merge(&mut cur, &json!({"unifiedWindows": {"five_hour": {"utilization": 0.5, "resetsAt": reset + 42}}}), NOW + 2000);
        assert_eq!((cur["five_hour"].util, cur["five_hour"].resets_at), (Some(1.0), reset + 60), "an older second is not a new window and use does not fall");
        // A reset a whole window earlier is still an older event, and one later replaces.
        assert!(!merge(&mut cur, &json!({"unifiedWindows": {"five_hour": {"utilization": 0.1, "resetsAt": reset - 600}}}), NOW + 3000));
        assert!(merge(&mut cur, &json!({"unifiedWindows": {"five_hour": {"utilization": 0.05, "resetsAt": reset + 18000}}}), NOW + 70_000));
    }

    #[test]
    fn an_ask_is_held_to_one_at_a_time_and_a_minute_apart_per_account() {
        let (tx, _rx) = broadcast::channel(4);
        let store = UsageStore::open(&std::env::temp_dir().join("no-such-usage.json"), tx);
        let first = store.begin_probe("a", NOW).unwrap();
        assert_eq!(store.begin_probe("a", NOW + 1).err(), Some(Skip::Busy));
        assert!(store.begin_probe("b", NOW + 1).is_ok(), "another account is its own");
        drop(first);
        assert_eq!(store.begin_probe("a", NOW + 30_000).err(), Some(Skip::Soon));
        assert!(store.begin_probe("a", NOW + PROBE_GAP_MS).is_ok());
    }

    #[test]
    fn a_full_table_keeps_running_and_recent_asks_and_makes_room_only_from_old_ones() {
        let (tx, _rx) = broadcast::channel(4);
        let store = UsageStore::open(&std::env::temp_dir().join("no-such-usage-2.json"), tx);
        let held: Vec<_> = (0..MAX_ACCOUNTS).map(|i| store.begin_probe(&format!("r{i}"), NOW).unwrap()).collect();
        // Every place is a running ask: another account waits, and nothing running is let go.
        assert_eq!(store.begin_probe("new", NOW + PROBE_GAP_MS + 1).err(), Some(Skip::Busy));
        assert_eq!(store.begin_probe("r0", NOW + PROBE_GAP_MS + 1).err(), Some(Skip::Busy), "still running");
        drop(held);
        // Over (not running, a minute old): they make room.
        assert!(store.begin_probe("new", NOW + PROBE_GAP_MS + 1).is_ok());
    }

    #[test]
    fn chats_saving_at_once_never_leave_a_broken_file_or_a_removed_account_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let store = std::sync::Arc::new(UsageStore::open(&path, broadcast::channel(8).0));
        let now = crate::now_ms();
        let reset = now / 1000 + 3600;
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let store = store.clone();
                std::thread::spawn(move || {
                    for i in 0..40 {
                        let info = json!({"unifiedWindows": {"five_hour": {"utilization": 0.01 * (i as f64), "resetsAt": reset, "status": "allowed"}}});
                        store.record(Some(&format!("acc{n}")), &info, now + i * 100_000);
                        if i == 20 {
                            store.forget(&format!("acc{n}"));
                        }
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let from_file: File = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).expect("the file is whole");
        assert_eq!(from_file.accounts.len(), 8);
        assert_eq!(serde_json::to_value(&from_file.accounts).unwrap(), serde_json::to_value(store.snapshot()).unwrap(), "the file is what the map holds");
        assert!(!path.with_extension("json.tmp").exists());
    }
}
