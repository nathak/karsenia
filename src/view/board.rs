//! What the board page shows: the rows the host made (`board::Row`) filtered, sorted and put into the words of the page. Nothing here
//! reads the clock or the disk: the time is an argument, and the page only draws what it is given.
use super::i18n::{fill, tr, Lang};
use crate::board::{Merge, Row, Signal};

/// Which chats the board lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    /// A question waits for the person.
    Asks,
    Running,
    /// Worktrees that cannot be merged as they are: a conflict, uncommitted work, a blocker, or settings that were not looked at.
    Blocked,
    /// Something is worth a look (a repeated call, a wide spread of edits, a held call, long work, a failed turn, a full context).
    Signals,
}

impl Filter {
    pub const ALL: [Filter; 5] = [Filter::All, Filter::Asks, Filter::Running, Filter::Blocked, Filter::Signals];

    pub fn from_index(i: i32) -> Self {
        usize::try_from(i).ok().and_then(|i| Self::ALL.get(i).copied()).unwrap_or_default()
    }

    fn key(self) -> &'static str {
        match self {
            Filter::All => "board_f_all",
            Filter::Asks => "board_f_asks",
            Filter::Running => "board_f_running",
            Filter::Blocked => "board_f_blocked",
            Filter::Signals => "board_f_signals",
        }
    }

    pub fn names(lang: Lang) -> Vec<String> {
        Self::ALL.iter().map(|f| tr(lang, f.key())).collect()
    }

    fn keeps(self, r: &Row) -> bool {
        match self {
            Filter::All => true,
            Filter::Asks => r.pending > 0,
            Filter::Running => r.state == "running",
            Filter::Blocked => matches!(r.wt.as_ref().map(|w| w.merge), Some(Merge::Conflict { .. } | Merge::Uncommitted { .. } | Merge::Blocked | Merge::UnsafeConfig | Merge::UnsafeOwner)),
            Filter::Signals => anomalies(r),
        }
    }
}

/// The order of the lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// What needs the person first.
    #[default]
    Attention,
    Recent,
    Cost,
    Name,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::Attention, Sort::Recent, Sort::Cost, Sort::Name];

    pub fn from_index(i: i32) -> Self {
        usize::try_from(i).ok().and_then(|i| Self::ALL.get(i).copied()).unwrap_or_default()
    }

    fn key(self) -> &'static str {
        match self {
            Sort::Attention => "board_s_attention",
            Sort::Recent => "board_s_recent",
            Sort::Cost => "board_s_cost",
            Sort::Name => "board_s_name",
        }
    }

    pub fn names(lang: Lang) -> Vec<String> {
        Self::ALL.iter().map(|s| tr(lang, s.key())).collect()
    }
}

/// Signs that something went wrong or may (everything but a waiting question, which has its own place in the line).
fn anomalies(r: &Row) -> bool {
    r.signals.iter().any(|s| s.kind != "ask")
}

/// How much a chat needs the person: lower first.
fn rank(r: &Row) -> u8 {
    if r.state == "exited" {
        return 5;
    }
    if r.pending > 0 {
        return 0;
    }
    if anomalies(r) {
        return 1;
    }
    if r.state == "running" {
        return 2;
    }
    if r.wt.as_ref().is_some_and(|w| !matches!(w.merge, Merge::Unknown | Merge::Gone | Merge::Nothing | Merge::Failed)) {
        return 3;
    }
    4
}

/// The newest sign of life of a chat (a chat with no time in its log is as old as its creation).
fn touched(r: &Row) -> u64 {
    r.counters.last_ts.max(r.created)
}

pub fn sorted(mut rows: Vec<Row>, sort: Sort) -> Vec<Row> {
    match sort {
        Sort::Attention => rows.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| touched(b).cmp(&touched(a))).then_with(|| a.sid.cmp(&b.sid))),
        Sort::Recent => rows.sort_by(|a, b| touched(b).cmp(&touched(a)).then_with(|| a.sid.cmp(&b.sid))),
        Sort::Cost => rows.sort_by(|a, b| b.cost.total_cmp(&a.cost).then_with(|| touched(b).cmp(&touched(a))).then_with(|| a.sid.cmp(&b.sid))),
        Sort::Name => rows.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()).then_with(|| a.sid.cmp(&b.sid))),
    }
    rows
}

/// One line as the page draws it. Tones: `ok`, `warn`, `err`, `neutral`, `run`.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub sid: String,
    pub title: String,
    pub project: String,
    /// The account's name, when the chats on the board belong to more than one.
    pub chip: String,
    pub state_tone: &'static str,
    pub state_text: String,
    pub progress: String,
    /// The signals in words, for the second line ("" for none).
    pub notes: String,
    pub change: String,
    /// What the change column says is old, or not known yet.
    pub dim: bool,
    pub merge: String,
    pub merge_tone: &'static str,
    pub cost: String,
    pub ask: bool,
    pub worktree: bool,
    /// The sentence a screen reader says for the whole line.
    pub spoken: String,
}

fn minutes(ms: u64, lang: Lang) -> String {
    let mins = ms / 60_000;
    match (mins, lang) {
        (0, Lang::Ko) => format!("{}초", (ms / 1000).max(1)),
        (0, Lang::En) => format!("{}s", (ms / 1000).max(1)),
        (1..=59, Lang::Ko) => format!("{mins}분"),
        (1..=59, Lang::En) => format!("{mins}m"),
        (_, Lang::Ko) => format!("{}시간 {}분", mins / 60, mins % 60),
        (_, Lang::En) => format!("{}h {}m", mins / 60, mins % 60),
    }
}

/// How long ago, in words: 방금, 2분 전, 3시간 전, 4일 전 (just now, 2m ago, 3h ago, 4d ago).
fn since(then: u64, now: u64, lang: Lang) -> String {
    let secs = now.saturating_sub(then) / 1000;
    let (n, unit) = match secs {
        0..=59 => return tr(lang, "board_just_now"),
        60..=3599 => (secs / 60, "m"),
        3600..=86399 => (secs / 3600, "h"),
        _ => (secs / 86400, "d"),
    };
    match (lang, unit) {
        (Lang::Ko, "m") => format!("{n}분 전"),
        (Lang::Ko, "h") => format!("{n}시간 전"),
        (Lang::Ko, _) => format!("{n}일 전"),
        (Lang::En, u) => format!("{n}{u} ago"),
    }
}

fn state_of(r: &Row, lang: Lang) -> (&'static str, String) {
    if r.state == "exited" {
        return ("neutral", tr(lang, "board_st_exit"));
    }
    if r.pending > 0 {
        return ("warn", fill(&tr(lang, "board_st_ask"), &[("n", r.pending.to_string())]));
    }
    if r.state == "running" {
        return ("run", tr(lang, "board_st_run"));
    }
    if r.counters.last_ok == Some(false) {
        return ("err", tr(lang, "board_st_fail"));
    }
    ("ok", tr(lang, "board_st_idle"))
}

fn progress_of(r: &Row, now: u64, lang: Lang) -> String {
    if r.pending > 0 && r.state != "exited" {
        return fill(&tr(lang, "board_prog_ask"), &[("n", r.pending.to_string())]);
    }
    if r.state == "running" {
        let elapsed = (r.counters.run_ts > 0 && now >= r.counters.run_ts).then(|| minutes(now - r.counters.run_ts, lang));
        let what = match &r.counters.top_tool {
            Some((tool, n)) => format!("{tool}×{n}"),
            None => tr(lang, "board_prog_working"),
        };
        return match elapsed {
            Some(e) => format!("{what} · {e}"),
            None => what,
        };
    }
    if r.counters.last_ts == 0 {
        return "—".to_owned();
    }
    since(r.counters.last_ts, now, lang)
}

fn note_of(s: &Signal, lang: Lang) -> Option<String> {
    let (key, n) = match s.kind {
        "loop" => ("board_n_loop", 0),
        "scope" => ("board_n_scope", s.n),
        "guard" => ("board_n_guard", s.n),
        "long" => ("board_n_long", s.n),
        "failed" => ("board_n_failed", 0),
        "ctx" => ("board_n_ctx", s.n),
        _ => return None,
    };
    Some(fill(&tr(lang, key), &[("n", n.to_string())]))
}

fn change_of(r: &Row, lang: Lang) -> (String, bool) {
    let Some(wt) = &r.wt else { return (tr(lang, "board_na"), false) };
    match wt.merge {
        Merge::Unknown => ("…".to_owned(), true),
        Merge::Gone | Merge::UnsafeConfig | Merge::UnsafeOwner | Merge::Failed => ("—".to_owned(), true),
        _ => (format!("+{} −{} ({})", wt.insertions, wt.deletions, wt.files), wt.refreshing || wt.age_ms > 10 * 60_000),
    }
}

fn merge_of(r: &Row, lang: Lang) -> (String, &'static str) {
    let Some(wt) = &r.wt else { return (tr(lang, "board_na"), "neutral") };
    let n = |n: usize| [("n", n.to_string())];
    match wt.merge {
        Merge::Unknown => ("…".to_owned(), "neutral"),
        Merge::Gone => (tr(lang, "board_m_gone"), "neutral"),
        Merge::Conflict { n: c } => (fill(&tr(lang, "board_m_conflict"), &n(c)), "err"),
        Merge::Uncommitted { n: c } => (fill(&tr(lang, "board_m_uncommitted"), &n(c)), "warn"),
        Merge::Blocked => (tr(lang, "board_m_blocked"), "warn"),
        Merge::Nothing => (tr(lang, "board_m_nothing"), "neutral"),
        Merge::Behind { n: c } => (fill(&tr(lang, "board_m_behind"), &n(c)), "warn"),
        Merge::Ready => (tr(lang, "board_m_ready"), "ok"),
        Merge::UnsafeConfig => (tr(lang, "board_m_config"), "err"),
        Merge::UnsafeOwner => (tr(lang, "board_m_owner"), "err"),
        Merge::Failed => (tr(lang, "board_m_failed"), "warn"),
    }
}

/// `≈$0.42` (an estimate, as every cost of the app is called), "—" for none.
pub fn money(cost: f64) -> String {
    if cost > 0.0 && cost.is_finite() {
        format!("≈${cost:.2}")
    } else {
        "—".to_owned()
    }
}

pub fn line(r: &Row, many_accounts: bool, now: u64, lang: Lang) -> Line {
    let (state_tone, state_text) = state_of(r, lang);
    let progress = progress_of(r, now, lang);
    let notes = r.signals.iter().filter_map(|s| note_of(s, lang)).collect::<Vec<_>>().join(" · ");
    let (change, dim) = change_of(r, lang);
    let (merge, merge_tone) = merge_of(r, lang);
    let chip = if many_accounts { r.account_name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| tr(lang, "board_acct_default")) } else { String::new() };
    let title = if r.title.trim().is_empty() { tr(lang, "board_untitled") } else { r.title.clone() };
    let spoken = format!("{title}, {state_text}, {progress}, {}: {merge}, {}", tr(lang, "board_c_merge"), money(r.cost));
    Line { sid: r.sid.clone(), title, project: r.project.clone(), chip, state_tone, state_text, progress, notes, change, dim, merge, merge_tone, cost: money(r.cost), ask: r.pending > 0 && r.state != "exited", worktree: r.worktree, spoken }
}

/// The page: the lines to draw, and the sentence over them (counted over every chat on the board, not only the ones the filter keeps).
pub struct Page {
    pub lines: Vec<Line>,
    pub summary: String,
    /// Chats on the board, and how many the filter left out.
    pub total: usize,
}

pub fn page(rows: &[Row], filter: Filter, sort: Sort, now: u64, lang: Lang) -> Page {
    let accounts: std::collections::HashSet<&Option<String>> = rows.iter().map(|r| &r.account).collect();
    let many = accounts.len() > 1;
    let kept: Vec<Row> = rows.iter().filter(|r| filter.keeps(r)).cloned().collect();
    let lines = sorted(kept, sort).iter().map(|r| line(r, many, now, lang)).collect();
    Page { lines, summary: summary(rows, lang), total: rows.len() }
}

/// "실행 2 · 질문 대기 1 · 병합 가능 3 · 합계 ≈$1.83": parts that are zero are left out, the total stays.
pub fn summary(rows: &[Row], lang: Lang) -> String {
    let running = rows.iter().filter(|r| r.state == "running").count();
    let asking = rows.iter().filter(|r| r.pending > 0 && r.state != "exited").count();
    let ready = rows.iter().filter(|r| matches!(r.wt.as_ref().map(|w| w.merge), Some(Merge::Ready | Merge::Behind { .. }))).count();
    let total: f64 = rows.iter().map(|r| r.cost).filter(|c| c.is_finite()).sum();
    let mut parts = Vec::new();
    for (key, n) in [("board_sum_running", running), ("board_sum_asks", asking), ("board_sum_ready", ready)] {
        if n > 0 {
            parts.push(fill(&tr(lang, key), &[("n", n.to_string())]));
        }
    }
    parts.push(fill(&tr(lang, "board_sum_total"), &[("cost", money(total))]));
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{Counters, WtRow};

    fn row(sid: &str) -> Row {
        Row { sid: sid.to_owned(), title: sid.to_owned(), project: "proj".into(), account: None, account_name: None, created: 100, worktree: false, provider: false, container: false, state: "idle", pending: 0, archived: false, counters: Counters::default(), cost: 0.0, ctx_pct: None, wt: None, signals: Vec::new() }
    }

    fn with_wt(mut r: Row, merge: Merge) -> Row {
        r.worktree = true;
        r.wt = Some(WtRow { insertions: 12, deletions: 3, files: 2, ..WtRow::of(merge) });
        r
    }

    fn sig(kind: &'static str, n: u32) -> Signal {
        Signal { kind, n }
    }

    #[test]
    fn the_default_order_puts_what_waits_for_the_person_first_then_trouble_then_work_then_what_can_be_merged() {
        let mut ask = row("ask");
        ask.pending = 1;
        ask.state = "running";
        let mut trouble = row("trouble");
        trouble.signals = vec![sig("loop", 1)];
        let mut running = row("running");
        running.state = "running";
        let ready = with_wt(row("ready"), Merge::Ready);
        let idle = row("idle");
        let mut gone = row("exited");
        gone.state = "exited";
        gone.pending = 2;
        let order: Vec<String> = sorted(vec![gone.clone(), idle.clone(), ready.clone(), running.clone(), trouble.clone(), ask.clone()], Sort::Attention).into_iter().map(|r| r.sid).collect();
        assert_eq!(order, ["ask", "trouble", "running", "ready", "idle", "exited"]);
        // A chat that went down is last, whatever it was waiting for.
        assert_eq!(rank(&gone), 5);
    }

    #[test]
    fn within_a_rank_the_newest_comes_first_and_ties_keep_a_fixed_order() {
        let mut a = row("a");
        a.counters.last_ts = 500;
        let mut b = row("b");
        b.counters.last_ts = 900;
        let c = row("c");
        let d = row("d");
        let order: Vec<String> = sorted(vec![d, c, a, b], Sort::Attention).into_iter().map(|r| r.sid).collect();
        assert_eq!(order, ["b", "a", "c", "d"], "c and d have no time: their creation (equal) then their id");
    }

    #[test]
    fn the_other_orders() {
        let mut a = row("Beta");
        a.created = 0;
        a.cost = 1.0;
        a.counters.last_ts = 10;
        let mut b = row("alpha");
        b.created = 0;
        b.cost = 3.0;
        b.counters.last_ts = 20;
        let mut c = row("gamma");
        c.created = 0;
        c.cost = 2.0;
        c.counters.last_ts = 30;
        let names = |s: Sort| sorted(vec![a.clone(), b.clone(), c.clone()], s).into_iter().map(|r| r.sid).collect::<Vec<_>>();
        assert_eq!(names(Sort::Recent), ["gamma", "alpha", "Beta"]);
        assert_eq!(names(Sort::Cost), ["alpha", "gamma", "Beta"]);
        assert_eq!(names(Sort::Name), ["alpha", "Beta", "gamma"], "case does not decide");
    }

    #[test]
    fn the_filters_keep_what_they_say() {
        let mut ask = row("ask");
        ask.pending = 2;
        let mut run = row("run");
        run.state = "running";
        let conflict = with_wt(row("conflict"), Merge::Conflict { n: 2 });
        let ready = with_wt(row("ready"), Merge::Ready);
        let mut loopy = row("loopy");
        loopy.signals = vec![sig("loop", 1)];
        let mut asky = row("asky");
        asky.signals = vec![sig("ask", 1)];
        let all = [ask, run, conflict, ready, loopy, asky];
        let keep = |f: Filter| all.iter().filter(|r| f.keeps(r)).map(|r| r.sid.as_str()).collect::<Vec<_>>();
        assert_eq!(keep(Filter::All).len(), 6);
        assert_eq!(keep(Filter::Asks), ["ask"]);
        assert_eq!(keep(Filter::Running), ["run"]);
        assert_eq!(keep(Filter::Blocked), ["conflict"]);
        assert_eq!(keep(Filter::Signals), ["loopy"], "a waiting question is not an anomaly");
        assert_eq!(Filter::from_index(3), Filter::Blocked);
        assert_eq!(Filter::from_index(-1), Filter::All);
        assert_eq!(Sort::from_index(9), Sort::Attention);
    }

    #[test]
    fn a_waiting_chat_says_so_in_its_state_and_its_progress() {
        let mut r = row("a");
        r.pending = 1;
        r.state = "running";
        let l = line(&r, false, 1000, Lang::Ko);
        assert_eq!((l.state_tone, l.state_text.as_str(), l.progress.as_str(), l.ask), ("warn", "질문 1", "권한 대기 1건", true));
    }

    #[test]
    fn a_working_chat_says_what_it_does_most_and_for_how_long() {
        let mut r = row("a");
        r.state = "running";
        r.counters.run_ts = 1_000;
        r.counters.top_tool = Some(("Edit".into(), 12));
        let l = line(&r, false, 1_000 + 4 * 60_000 + 5_000, Lang::Ko);
        assert_eq!((l.state_tone, l.progress.as_str()), ("run", "Edit×12 · 4분"));
        assert_eq!(line(&r, false, 1_000 + 65 * 60_000, Lang::En).progress, "Edit×12 · 1h 5m");
        assert_eq!(line(&r, false, 1_000 + 20_000, Lang::Ko).progress, "Edit×12 · 20초");
        r.counters.top_tool = None;
        assert_eq!(line(&r, false, 2_000, Lang::Ko).progress, "작업 중 · 1초");
        // A clock that went back, or a start nobody wrote down: no time is made up.
        r.counters.run_ts = 0;
        assert_eq!(line(&r, false, 2_000, Lang::Ko).progress, "작업 중");
        r.counters.run_ts = 9_000;
        assert_eq!(line(&r, false, 2_000, Lang::Ko).progress, "작업 중");
    }

    #[test]
    fn a_chat_that_stopped_says_how_long_ago_and_a_chat_that_failed_says_that() {
        let mut r = row("a");
        r.counters.last_ts = 1_000_000;
        let l = line(&r, false, 1_000_000 + 2 * 60_000, Lang::Ko);
        assert_eq!((l.state_tone, l.state_text.as_str(), l.progress.as_str()), ("ok", "끝남", "2분 전"));
        assert_eq!(line(&r, false, 1_000_000 + 3 * 3_600_000, Lang::Ko).progress, "3시간 전");
        assert_eq!(line(&r, false, 1_000_000 + 2 * 86_400_000, Lang::En).progress, "2d ago");
        assert_eq!(line(&r, false, 1_000_000 + 30_000, Lang::Ko).progress, "방금");
        r.counters.last_ok = Some(false);
        assert_eq!((line(&r, false, 1_000_000, Lang::Ko).state_tone, line(&r, false, 1_000_000, Lang::Ko).state_text), ("err", "실패".to_owned()));
        let mut none = row("n");
        none.counters.last_ts = 0;
        assert_eq!(line(&none, false, 5, Lang::Ko).progress, "—");
        let mut exited = row("e");
        exited.state = "exited";
        exited.pending = 3;
        let l = line(&exited, false, 5, Lang::Ko);
        assert_eq!((l.state_text.as_str(), l.ask), ("종료", false));
    }

    #[test]
    fn the_merge_and_change_columns_say_not_applicable_for_a_chat_without_a_worktree() {
        let l = line(&row("a"), false, 1, Lang::Ko);
        assert_eq!((l.merge.as_str(), l.merge_tone, l.change.as_str(), l.worktree), ("해당 없음", "neutral", "해당 없음", false));
        assert_eq!(line(&row("a"), false, 1, Lang::En).merge, "n/a");
    }

    #[test]
    fn every_merge_state_has_its_words_and_its_tone() {
        let words = |m: Merge| {
            let l = line(&with_wt(row("a"), m), false, 1, Lang::Ko);
            (l.merge, l.merge_tone, l.change)
        };
        assert_eq!(words(Merge::Ready), ("가능".into(), "ok", "+12 −3 (2)".into()));
        assert_eq!(words(Merge::Behind { n: 4 }).0, "가능 · 뒤처짐 4");
        assert_eq!(words(Merge::Conflict { n: 2 }), ("충돌 2".into(), "err", "+12 −3 (2)".into()));
        assert_eq!(words(Merge::Uncommitted { n: 1 }).0, "미커밋 1");
        assert_eq!(words(Merge::Blocked).0, "막힘");
        assert_eq!(words(Merge::Nothing).0, "변경 없음");
        assert_eq!(words(Merge::Gone), ("정리됨".into(), "neutral", "—".into()));
        assert_eq!(words(Merge::UnsafeConfig), ("git 설정 변경".into(), "err", "—".into()));
        assert_eq!(words(Merge::UnsafeOwner).1, "err");
        assert_eq!(words(Merge::Failed).1, "warn");
        assert_eq!(words(Merge::Unknown), ("…".into(), "neutral", "…".into()));
    }

    #[test]
    fn a_change_that_is_old_or_being_looked_at_again_is_dimmed() {
        let mut r = with_wt(row("a"), Merge::Ready);
        assert!(!line(&r, false, 1, Lang::Ko).dim);
        r.wt.as_mut().unwrap().refreshing = true;
        assert!(line(&r, false, 1, Lang::Ko).dim);
        r.wt.as_mut().unwrap().refreshing = false;
        r.wt.as_mut().unwrap().age_ms = 11 * 60_000;
        assert!(line(&r, false, 1, Lang::Ko).dim);
    }

    #[test]
    fn the_signals_are_said_in_words_on_a_second_line() {
        let mut r = row("a");
        r.signals = vec![sig("ask", 1), sig("loop", 1), sig("scope", 40), sig("guard", 2), sig("long", 14), sig("failed", 1), sig("ctx", 91)];
        let l = line(&r, false, 1, Lang::Ko);
        assert_eq!(l.notes, "같은 호출 반복 · 수정한 파일 40개 · 가드 차단 2 · 오래 걸림 14분 · 직전 턴 실패 · 컨텍스트 91%");
        assert_eq!(line(&row("b"), false, 1, Lang::Ko).notes, "");
    }

    #[test]
    fn the_account_is_named_only_when_the_board_has_more_than_one() {
        let mut work = row("w");
        work.account = Some("w1".into());
        work.account_name = Some("Work".into());
        let home = row("h");
        let one = page(&[work.clone()], Filter::All, Sort::Attention, 1, Lang::Ko);
        assert_eq!(one.lines[0].chip, "");
        let two = page(&[work, home], Filter::All, Sort::Recent, 1, Lang::Ko);
        let chips: Vec<_> = two.lines.iter().map(|l| (l.sid.as_str(), l.chip.as_str())).collect();
        assert!(chips.contains(&("w", "Work")) && chips.contains(&("h", "기본")), "{chips:?}");
    }

    #[test]
    fn the_summary_counts_every_chat_not_only_the_ones_kept_and_leaves_out_what_is_zero() {
        let mut a = row("a");
        a.state = "running";
        a.cost = 1.0;
        let mut b = row("b");
        b.pending = 1;
        b.cost = 0.83;
        let c = with_wt(row("c"), Merge::Behind { n: 1 });
        let p = page(&[a, b, c], Filter::Asks, Sort::Attention, 1, Lang::Ko);
        assert_eq!(p.lines.len(), 1);
        assert_eq!((p.total, p.summary.as_str()), (3, "실행 1 · 질문 대기 1 · 병합 가능 1 · 합계 ≈$1.83"));
        assert_eq!(summary(&[], Lang::Ko), "합계 —");
        assert_eq!(summary(&[row("x")], Lang::En), "Total —");
    }

    #[test]
    fn money_is_an_estimate_and_nothing_for_nothing() {
        assert_eq!((money(0.424).as_str(), money(0.0).as_str(), money(f64::NAN).as_str()), ("≈$0.42", "—", "—"));
    }

    #[test]
    fn a_chat_without_a_title_gets_a_name_and_the_line_can_be_read_aloud() {
        let mut r = row("a");
        r.title = "  ".into();
        let l = line(&r, false, 1, Lang::Ko);
        assert_eq!(l.title, "제목 없음");
        assert!(l.spoken.contains("제목 없음") && l.spoken.contains("끝남"));
    }
}
