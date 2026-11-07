//! The system clock across summer-time changes. One test in its own binary: it sets the process's time zone.
use swb_app::schedule::{next_due, Clock, Schedule, SystemClock, ALL_DAYS};

/// Every due time of a daily task from `from` to `to`, found the way the scheduler does: asked a little after each one.
fn dues(schedule: &Schedule, from: i64, to: i64) -> Vec<i64> {
    let clock = SystemClock;
    let mut out = Vec::new();
    let mut after = from;
    while let Some(next) = next_due(schedule, after, &clock) {
        if next > to {
            break;
        }
        out.push(next);
        after = next + 20_000;
    }
    out
}

#[test]
fn a_daily_task_runs_once_a_day_across_both_summer_time_changes() {
    std::env::set_var("TZ", "Europe/Berlin");
    extern "C" {
        fn tzset();
    }
    // SAFETY: tzset re-reads TZ; nothing else in this test binary uses the time zone at the same time.
    unsafe {
        tzset();
    }
    let clock = SystemClock;
    let schedule = Schedule::Daily { minute_of_day: 2 * 60 + 30, weekdays: ALL_DAYS };

    // 2026-03-29: clocks go from 02:00 to 03:00, so 02:30 does not exist that day. One run a day, in order, one per local date.
    let from = clock.instant(2026, 3, 28, 12, 0);
    let to = clock.instant(2026, 3, 30, 12, 0);
    let found = dues(&schedule, from, to);
    let days: Vec<(u32, u32)> = found.iter().map(|&t| (clock.local(t).month, clock.local(t).day)).collect();
    assert_eq!(days, vec![(3, 29), (3, 30)], "one run for each of the two days, none lost, none doubled: {days:?}");
    assert!(found.windows(2).all(|w| w[0] < w[1]));
    let last = clock.local(found[1]);
    assert_eq!((last.hour, last.minute), (2, 30), "the day after the change is at 02:30 again");

    // 2026-10-25: clocks go from 03:00 back to 02:00, so 02:30 happens twice. The task runs at the first one only.
    let from = clock.instant(2026, 10, 24, 12, 0);
    let to = clock.instant(2026, 10, 26, 12, 0);
    let found = dues(&schedule, from, to);
    let days: Vec<(u32, u32)> = found.iter().map(|&t| (clock.local(t).month, clock.local(t).day)).collect();
    assert_eq!(days, vec![(10, 25), (10, 26)], "02:30 that happens twice is run once: {days:?}");

    // A run asked for at an hour that exists is exactly there: 09:00 stays 09:00 on both sides of the change.
    let nine = Schedule::Daily { minute_of_day: 9 * 60, weekdays: ALL_DAYS };
    for (y, m, d) in [(2026, 3, 29), (2026, 3, 30), (2026, 10, 25), (2026, 10, 26)] {
        let next = next_due(&nine, clock.instant(y, m, i64::from(d), 0, 5), &clock).unwrap();
        let l = clock.local(next);
        assert_eq!((l.day, l.hour, l.minute), (d, 9, 0));
    }
}
