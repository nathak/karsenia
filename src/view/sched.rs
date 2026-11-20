//! The Schedule panel and the Providers card, turned into rows and operations for the desktop app. The data is what the web page
//! gets (`sched`, `sched_run_detail` and `providers` replies); the operations are the ones the page sends. Nothing here talks to a
//! store, and a provider's key only ever goes into an operation: it is never kept in a row.
use serde_json::{json, Value};

use super::i18n::{fill, tr, Lang};
use crate::schedule::Clock;

/// The models the form offers.
pub const MODELS: [&str; 3] = ["sonnet", "haiku", "opus"];

pub fn fmt_minute(m: u16) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// "9:05" or "09:05" as minutes of the day; nothing for "24:00", "9:60", "" or words.
pub fn parse_hhmm(text: &str) -> Option<u16> {
    let (h, m) = text.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 || !h.chars().all(|c| c.is_ascii_digit()) || !m.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Bit 0 is Monday.
pub fn mask_to_days(mask: u8) -> [bool; 7] {
    std::array::from_fn(|i| mask & (1 << i) != 0)
}

pub fn days_to_mask(days: &[bool; 7]) -> u8 {
    days.iter().enumerate().fold(0, |m, (i, on)| if *on { m | (1 << i) } else { m })
}

fn day_name(i: usize, lang: Lang) -> String {
    tr(lang, &format!("sc_d{i}"))
}

/// "daily 09:00", "Mon Tue Wed 09:00", "every 60 min".
pub fn summary(task: &Value, lang: Lang) -> String {
    if task["kind"] == "every" {
        return fill(&tr(lang, "sc_every_txt"), &[("n", task["every_min"].as_u64().unwrap_or(0).to_string())]);
    }
    let time = fmt_minute(task["minute_of_day"].as_u64().unwrap_or(0).min(1439) as u16);
    let mask = task["weekdays"].as_u64().unwrap_or(127) as u8;
    if mask & 127 == 127 {
        return format!("{} {time}", tr(lang, "sc_daily_txt"));
    }
    let days: Vec<String> = mask_to_days(mask).iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| day_name(i, lang)).collect();
    format!("{} {time}", days.join(if lang == Lang::Ko { "" } else { " " }))
}

/// A moment as the person's clock shows it: the time of day for today, the date and time otherwise.
pub fn fmt_when(ms: i64, now: i64, clock: &dyn Clock) -> String {
    if ms <= 0 {
        return String::new();
    }
    let (t, n) = (clock.local(ms), clock.local(now));
    let time = format!("{:02}:{:02}", t.hour, t.minute);
    if (t.year, t.month, t.day) == (n.year, n.month, n.day) {
        time
    } else if t.year == n.year {
        format!("{:02}-{:02} {time}", t.month, t.day)
    } else {
        format!("{:04}-{:02}-{:02} {time}", t.year, t.month, t.day)
    }
}

pub fn status_label(status: &str, lang: Lang) -> String {
    let key = match status {
        "ok" => "st_ok",
        "error" => "st_error",
        "timeout" => "st_timeout",
        "cap" => "st_cap",
        "skipped" => "st_skipped",
        "missed" => "st_missed",
        "running" => "st_running",
        "stopped" => "st_stopped",
        other => return other.chars().filter(|c| !c.is_control()).take(40).collect(),
    };
    tr(lang, key)
}

/// 1 good, 2 bad, 0 neither: the colour of a run's tag.
pub fn status_tone(status: &str) -> i32 {
    match status {
        "ok" => 1,
        "error" | "timeout" => 2,
        _ => 0,
    }
}

pub fn cost(usd: f64) -> String {
    format!("${usd:.3}")
}

fn s(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SchedHead {
    pub available: bool,
    pub on: bool,
    pub notice: String,
    pub unseen: i32,
    pub count: i32,
    pub full: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskRow {
    pub id: i64,
    pub name: String,
    /// "files" or "web".
    pub profile: String,
    pub summary: String,
    /// "", paused or off.
    pub state: String,
    /// The folder, or the sites it may open.
    pub detail: String,
    pub tools: String,
    pub next: String,
    pub enabled: bool,
    pub running: bool,
    pub paused_note: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunRow {
    pub id: i64,
    pub status: String,
    pub tone: i32,
    pub name: String,
    pub when: String,
    pub unseen: bool,
}

/// How many recent runs the panel lists.
pub const RUNS_SHOWN: usize = 30;

pub fn sched_view(data: &Value, lang: Lang, now: i64, clock: &dyn Clock) -> (SchedHead, Vec<TaskRow>, Vec<RunRow>) {
    let tasks_json = data["tasks"].as_array().cloned().unwrap_or_default();
    let head = SchedHead {
        available: data["available"] == true,
        on: data["on"] == true,
        notice: s(data, "notice"),
        unseen: data["unseen"].as_i64().unwrap_or(0).clamp(0, 9999) as i32,
        count: tasks_json.len() as i32,
        full: tasks_json.len() >= crate::sched_store::MAX_TASKS,
    };
    if !head.available {
        return (head, Vec::new(), Vec::new());
    }
    let running: Vec<i64> = data["running"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
    let tasks = tasks_json
        .iter()
        .map(|t| {
            let id = t["id"].as_i64().unwrap_or(0);
            let enabled = t["enabled"] == true;
            let paused = s(t, "paused_reason");
            let paused_note = if paused == crate::sched_store::NEEDS_DOMAINS { tr(lang, "sc_needs_sites") } else { paused.chars().take(200).collect() };
            let web = t["profile"] == "web";
            let detail = if web {
                let sites: Vec<&str> = t["domains"].as_array().map(|d| d.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                format!("{}: {}", tr(lang, "sc_sites_label"), sites.join(", "))
            } else {
                s(t, "cwd")
            };
            TaskRow {
                id,
                name: s(t, "name"),
                profile: if web { "web".into() } else { "files".into() },
                summary: summary(t, lang),
                state: if !paused.is_empty() { tr(lang, "sc_paused") } else if !enabled { tr(lang, "sc_off") } else { String::new() },
                detail,
                tools: s(t, "tools"),
                next: if enabled { fmt_when(t["next_due"].as_i64().unwrap_or(0), now, clock) } else { String::new() },
                enabled,
                running: running.contains(&id),
                paused_note,
            }
        })
        .collect();
    let runs = data["runs"]
        .as_array()
        .map(|runs| {
            runs.iter()
                .take(RUNS_SHOWN)
                .map(|r| {
                    let status = s(r, "status");
                    let at = r["started"].as_i64().filter(|t| *t > 0).or(r["due"].as_i64()).unwrap_or(0);
                    RunRow { id: r["id"].as_i64().unwrap_or(0), tone: status_tone(&status), status: status_label(&status, lang), name: s(r, "task_name"), when: fmt_when(at, now, clock), unseen: r["seen"] != true }
                })
                .collect()
        })
        .unwrap_or_default();
    (head, tasks, runs)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunDetail {
    pub title: String,
    pub meta: String,
    pub note: String,
    /// A web run: its text came from outside.
    pub external: bool,
    pub denied: String,
    pub result: String,
    pub def: String,
}

pub fn run_detail(run: &Value, lang: Lang, clock: &dyn Clock) -> RunDetail {
    let status = s(run, "status");
    let at = run["started"].as_i64().filter(|t| *t > 0).or(run["due"].as_i64()).unwrap_or(0);
    let mut meta = vec![status_label(&status, lang), fmt_when(at, clock.now_ms(), clock)];
    if let Some(c) = run["cost"].as_f64().filter(|c| *c > 0.0) {
        meta.push(format!("{} {}", tr(lang, "sc_cost"), cost(c)));
    }
    if let Some(n) = run["turns"].as_u64().filter(|n| *n > 0) {
        meta.push(format!("{n} {}", tr(lang, "sc_turns_n")));
    }
    meta.retain(|m| !m.is_empty());
    let denied: Vec<String> = run["denied"].as_array().map(|d| d.iter().filter_map(Value::as_str).map(|x| x.chars().filter(|c| !c.is_control()).collect()).collect()).unwrap_or_default();
    let result = s(run, "result");
    RunDetail {
        title: s(run, "task_name"),
        meta: meta.join(" · "),
        note: s(run, "note"),
        external: run["def"]["profile"] == "web",
        denied: denied.join("\n"),
        result: if result.trim().is_empty() { tr(lang, "sc_no_result") } else { result },
        def: if run["def"].is_null() { String::new() } else { serde_json::to_string_pretty(&run["def"]).unwrap_or_default() },
    }
}

/// What the task form holds, as text and switches (the values of the form's fields).
#[derive(Debug, Clone, PartialEq)]
pub struct SchedForm {
    /// 0 for a new task.
    pub id: i64,
    pub name: String,
    pub prompt: String,
    pub web: bool,
    pub cwd: String,
    pub sites: String,
    pub daily: bool,
    pub time: String,
    pub days: [bool; 7],
    pub every: String,
    /// An index into `MODELS`.
    pub model: i32,
    pub turns: String,
    pub budget: String,
    pub timeout: String,
}

impl SchedForm {
    pub fn new_default() -> Self {
        SchedForm { id: 0, name: String::new(), prompt: String::new(), web: false, cwd: String::new(), sites: String::new(), daily: true, time: "09:00".into(), days: mask_to_days(0b0011111), every: "60".into(), model: 0, turns: "20".into(), budget: "0.5".into(), timeout: "600".into() }
    }
}

pub fn form_from_task(t: &Value) -> SchedForm {
    let sites: Vec<&str> = t["domains"].as_array().map(|d| d.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    SchedForm {
        id: t["id"].as_i64().unwrap_or(0),
        name: s(t, "name"),
        prompt: s(t, "prompt"),
        web: t["profile"] == "web",
        cwd: s(t, "cwd"),
        sites: sites.join("\n"),
        daily: t["kind"] != "every",
        time: fmt_minute(t["minute_of_day"].as_u64().unwrap_or(540).min(1439) as u16),
        days: mask_to_days(t["weekdays"].as_u64().unwrap_or(127) as u8),
        every: t["every_min"].as_u64().filter(|n| *n > 0).unwrap_or(60).to_string(),
        model: MODELS.iter().position(|m| *m == t["model"].as_str().unwrap_or_default()).unwrap_or(0) as i32,
        turns: t["max_turns"].as_u64().unwrap_or(20).to_string(),
        budget: t["budget_usd"].as_f64().unwrap_or(0.5).to_string(),
        timeout: t["timeout_s"].as_u64().unwrap_or(600).to_string(),
    }
}

/// The `sched_save` operation for a form. A field that is not a number is refused here: sent as null, the server would quietly
/// use its default instead. The Err is an i18n key.
pub fn sched_op(f: &SchedForm) -> Result<Value, &'static str> {
    let number = |text: &str| text.trim().parse::<u64>().map_err(|_| "sc_bad_number");
    let max_turns = number(&f.turns)?;
    let timeout_s = number(&f.timeout)?;
    let budget_usd: f64 = f.budget.trim().parse().ok().filter(|b: &f64| b.is_finite() && *b >= 0.0).ok_or("sc_bad_number")?;
    let mut op = json!({
        "op": "sched_save", "id": f.id, "name": f.name.trim(), "prompt": f.prompt.trim(),
        "profile": if f.web { "web" } else { "folder" },
        "cwd": if f.web { String::new() } else { f.cwd.trim().to_owned() },
        "domains": if f.web { f.sites.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect::<Vec<_>>() } else { Vec::new() },
        "model": MODELS.get(f.model.max(0) as usize).copied().unwrap_or("sonnet"),
        "max_turns": max_turns, "budget_usd": budget_usd, "timeout_s": timeout_s,
    });
    if f.daily {
        op["kind"] = json!("daily");
        op["minute_of_day"] = json!(parse_hhmm(&f.time).ok_or("sc_bad_time")?);
        op["weekdays"] = json!(days_to_mask(&f.days));
    } else {
        op["kind"] = json!("every");
        op["every_min"] = json!(number(&f.every)?);
    }
    Ok(op)
}

// ---- providers ----

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProvHead {
    pub notice: String,
    pub ctr_runtime: String,
    pub ctr_image: String,
    pub ctr_ready: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProvRow {
    pub id: String,
    pub name: String,
    pub sub: String,
    pub has_key: bool,
    pub chats: i32,
}

pub fn prov_view(data: &Value, lang: Lang) -> (ProvHead, Vec<ProvRow>) {
    let head = ProvHead { notice: s(data, "notice"), ctr_runtime: s(&data["container"], "runtime"), ctr_image: s(&data["container"], "image"), ctr_ready: data["container"]["ready"] == true };
    let rows = data["providers"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|p| {
                    let chats = p["chats"].as_u64().unwrap_or(0) as i32;
                    let key = if p["has_token"] == true { tr(lang, "pv_has_key") } else { tr(lang, "pv_no_key") };
                    let mut sub = vec![s(p, "host"), key];
                    if chats > 0 {
                        sub.push(fill(&tr(lang, "pv_chats"), &[("n", chats.to_string())]));
                    }
                    ProvRow { id: s(p, "id"), name: s(p, "name"), sub: sub.join(" · "), has_key: p["has_token"] == true, chats }
                })
                .collect()
        })
        .unwrap_or_default();
    (head, rows)
}

/// What the provider form holds. The key is here only while the operation is made.
pub struct ProvForm<'a> {
    pub id: Option<&'a str>,
    pub name: &'a str,
    pub url: &'a str,
    /// 0 bearer, 1 x-api-key.
    pub auth: i32,
    pub key: &'a str,
    pub clear: bool,
    pub model: &'a str,
    pub aliases: [&'a str; 3],
    /// The header lines (`Name: value`, `{key}` where the key goes).
    pub headers: &'a str,
}

/// The `provider_save` operation. An empty key field puts no "token" in it at all (the saved key stays); an empty id is a new
/// provider.
pub fn prov_op(f: &ProvForm) -> Value {
    let mut op = json!({
        "op": "provider_save", "name": f.name.trim(), "base_url": f.url.trim(), "auth": if f.auth == 1 { "x_api_key" } else { "bearer" },
        "model": f.model.trim(), "models": {"opus": f.aliases[0].trim(), "sonnet": f.aliases[1].trim(), "haiku": f.aliases[2].trim()},
        "headers": f.headers.trim(),
    });
    if let Some(id) = f.id.filter(|id| !id.is_empty()) {
        op["id"] = json!(id);
    }
    if !f.key.is_empty() {
        op["token"] = json!(f.key);
    }
    if f.clear {
        op["clear_token"] = json!(true);
    }
    op
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::FixedClock;

    // 2026-11-13 00:00 UTC.
    const DAY0: i64 = 1_794_528_000_000;

    #[test]
    fn times_and_weekdays_round_trip() {
        assert_eq!(fmt_minute(545), "09:05");
        assert_eq!(parse_hhmm("9:05"), Some(545));
        assert_eq!(parse_hhmm("09:05"), Some(545));
        for bad in ["24:00", "9:60", "", "ab", "9:5", "009:00", "9:05pm"] {
            assert_eq!(parse_hhmm(bad), None, "{bad}");
        }
        let weekdays = mask_to_days(0b0011111);
        assert_eq!(weekdays, [true, true, true, true, true, false, false]);
        assert_eq!(days_to_mask(&weekdays), 0b0011111);
        assert_eq!(days_to_mask(&mask_to_days(127)), 127);
    }

    #[test]
    fn summaries_in_both_languages() {
        let daily = json!({"kind": "daily", "minute_of_day": 540, "weekdays": 127});
        assert_eq!(summary(&daily, Lang::En), "daily 09:00");
        assert_eq!(summary(&daily, Lang::Ko), "매일 09:00");
        let week = json!({"kind": "daily", "minute_of_day": 545, "weekdays": 0b0000111});
        assert_eq!(summary(&week, Lang::En), "Mon Tue Wed 09:05");
        assert_eq!(summary(&week, Lang::Ko), "월화수 09:05");
        assert_eq!(summary(&json!({"kind": "every", "every_min": 60}), Lang::En), "every 60 min");
        assert_eq!(summary(&json!({"kind": "every", "every_min": 60}), Lang::Ko), "60분마다");
    }

    #[test]
    fn moments_follow_the_clock_and_say_less_for_today() {
        let clock = FixedClock::new(540, DAY0 + 3_600_000);
        let now = clock.now_ms();
        assert_eq!(fmt_when(DAY0, now, &clock), "09:00");
        assert_eq!(fmt_when(DAY0 + 86_400_000 * 2, now, &clock), "11-15 09:00");
        assert_eq!(fmt_when(DAY0 + 86_400_000 * 400, now, &clock), "2027-12-18 09:00");
        assert_eq!(fmt_when(0, now, &clock), "");
        assert_eq!((status_tone("ok"), status_tone("error"), status_tone("timeout"), status_tone("cap")), (1, 2, 2, 0));
        assert_eq!(status_label("ok", Lang::En), "ok");
        assert_eq!(status_label("weird\u{7}", Lang::En), "weird");
        assert_eq!(cost(0.01), "$0.010");
    }

    fn listing() -> Value {
        json!({"available": true, "on": true, "unseen": 2, "running": [2], "notice": "",
            "tasks": [
                {"id": 1, "name": "folder", "profile": "folder", "kind": "daily", "minute_of_day": 540, "weekdays": 127, "enabled": true, "paused_reason": "", "next_due": DAY0 + 3_600_000, "cwd": "/w/p", "tools": "Read,Grep,Glob", "domains": []},
                {"id": 2, "name": "web", "profile": "web", "kind": "every", "every_min": 60, "enabled": false, "paused_reason": crate::sched_store::NEEDS_DOMAINS, "next_due": 0, "cwd": "", "tools": "WebFetch,WebSearch", "domains": ["a.example", "b.example"]}],
            "runs": [{"id": 9, "task_id": 1, "task_name": "folder", "status": "ok", "started": DAY0, "due": DAY0, "seen": false}, {"id": 8, "task_id": 2, "task_name": "web", "status": "error", "started": 0, "due": DAY0, "seen": true}]})
    }

    #[test]
    fn the_listing_becomes_task_and_run_rows() {
        let clock = FixedClock::new(540, DAY0 + 3_600_000);
        let (head, tasks, runs) = sched_view(&listing(), Lang::En, clock.now_ms(), &clock);
        assert!(head.available && head.on && head.unseen == 2 && head.count == 2 && !head.full);
        assert_eq!(tasks[0].summary, "daily 09:00");
        assert_eq!((tasks[0].state.as_str(), tasks[0].next.as_str(), tasks[0].detail.as_str(), tasks[0].running), ("", "10:00", "/w/p", false));
        assert_eq!(tasks[1].state, "Paused");
        assert!(tasks[1].running && tasks[1].next.is_empty() && tasks[1].paused_note.starts_with("List its sites"), "{:?}", tasks[1]);
        assert_eq!(tasks[1].detail, "Sites: a.example, b.example");
        assert_eq!((runs[0].status.as_str(), runs[0].tone, runs[0].unseen, runs[0].when.as_str()), ("ok", 1, true, "09:00"));
        assert_eq!((runs[1].tone, runs[1].unseen), (2, false));
        let full = json!({"available": true, "tasks": (0..30).map(|i| json!({"id": i + 1, "name": "x", "profile": "web", "kind": "every", "every_min": 60, "enabled": true})).collect::<Vec<_>>()});
        assert!(sched_view(&full, Lang::En, 0, &clock).0.full);
        assert!(sched_view(&json!({"available": false, "notice": "no"}), Lang::En, 0, &clock).1.is_empty());
    }

    #[test]
    fn a_run_detail_names_what_a_web_run_may_have_read() {
        let clock = FixedClock::new(0, DAY0);
        let run = json!({"task_name": "web", "status": "ok", "started": DAY0, "cost": 0.01, "turns": 2, "result": "fake report", "denied": ["Bash: ls\u{1b}"], "note": "n", "def": {"profile": "web", "name": "web"}});
        let d = run_detail(&run, Lang::En, &clock);
        assert!(d.external && d.result == "fake report" && d.denied == "Bash: ls" && d.meta.contains("$0.010") && d.meta.contains("2 turns"), "{d:?}");
        assert!(d.def.contains("\"profile\": \"web\""));
        assert_eq!(run_detail(&json!({"status": "error"}), Lang::En, &clock).result, "(no result)");
    }

    #[test]
    fn the_task_form_makes_the_operation_and_refuses_what_the_server_would_quietly_default() {
        let mut f = SchedForm::new_default();
        f.name = " daily ".into();
        f.cwd = "/w/p".into();
        let op = sched_op(&f).expect("op");
        assert_eq!((op["profile"].as_str(), op["cwd"].as_str(), op["kind"].as_str(), op["minute_of_day"].as_u64(), op["weekdays"].as_u64(), op["name"].as_str()), (Some("folder"), Some("/w/p"), Some("daily"), Some(540), Some(31), Some("daily")));
        assert_eq!(op["domains"], json!([]));
        f.web = true;
        f.sites = " a.example \n\n b.example\n".into();
        f.cwd = "/ignored".into();
        f.daily = false;
        let op = sched_op(&f).expect("web op");
        assert_eq!((op["cwd"].as_str(), op["kind"].as_str(), op["every_min"].as_u64()), (Some(""), Some("every"), Some(60)));
        assert_eq!(op["domains"], json!(["a.example", "b.example"]));
        f.daily = true;
        f.time = "25:00".into();
        assert_eq!(sched_op(&f), Err("sc_bad_time"));
        f.time = "09:00".into();
        f.budget = "lots".into();
        assert_eq!(sched_op(&f), Err("sc_bad_number"));
        f.budget = "0.5".into();
        f.turns = "-1".into();
        assert_eq!(sched_op(&f), Err("sc_bad_number"));
        // A task goes into the form and comes out as the same operation.
        let t = &listing()["tasks"][1];
        let back = form_from_task(t);
        assert!(back.web && !back.daily && back.sites == "a.example\nb.example" && back.every == "60" && back.id == 2);
    }

    #[test]
    fn providers_show_the_host_and_whether_a_key_is_set_and_a_save_never_sends_an_empty_key() {
        let data = json!({"providers": [{"id": "p1", "name": "Gateway", "host": "gw.example", "has_token": true, "chats": 2}, {"id": "p2", "name": "Local", "host": "localhost", "has_token": false, "chats": 0}], "notice": "", "container": {"runtime": "/usr/bin/docker", "image": "img", "ready": true}});
        let (head, rows) = prov_view(&data, Lang::En);
        assert!(head.ctr_ready && head.ctr_runtime == "/usr/bin/docker");
        assert_eq!(rows[0].sub, "gw.example · key set · 2 chats");
        assert_eq!(rows[1].sub, "localhost · no key");
        let new = prov_op(&ProvForm { id: None, name: " N ", url: "https://x.example", auth: 1, key: "abcdefgh12", clear: false, model: "", aliases: ["", "", ""], headers: "" });
        assert_eq!((new["token"].as_str(), new["auth"].as_str(), new["name"].as_str()), (Some("abcdefgh12"), Some("x_api_key"), Some("N")));
        assert!(new.get("id").is_none() && new.get("clear_token").is_none());
        let keep = prov_op(&ProvForm { id: Some("p1"), name: "G", url: "https://x.example", auth: 0, key: "", clear: false, model: "m", aliases: ["o", "s", "h"], headers: "" });
        assert!(keep.get("token").is_none(), "an empty key field keeps the saved key");
        assert_eq!((keep["id"].as_str(), keep["models"]["haiku"].as_str()), (Some("p1"), Some("h")));
        let clear = prov_op(&ProvForm { id: Some("p1"), name: "G", url: "https://x.example", auth: 0, key: "", clear: true, model: "", aliases: ["", "", ""], headers: "" });
        assert_eq!(clear["clear_token"], true);
    }
}
