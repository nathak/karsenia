//! When a scheduled task is next due, in the person's local time.
//!
//! Two kinds of schedule: every N minutes (at least 15), or daily at a time of day on chosen weekdays. Local time comes from the
//! C library (`localtime_r`/`mktime`, which know the time zone and summer time); a fixed-offset clock stands in for it in tests.

/// Fewest minutes between two runs of an "every N minutes" task.
pub const MIN_EVERY_MIN: u32 = 15;
pub const ALL_DAYS: u8 = 0b111_1111;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    /// Every `minutes` minutes after the last due time.
    Every { minutes: u32 },
    /// At `minute_of_day` (0..1440) local time on the weekdays in `weekdays` (bit 0 Monday .. bit 6 Sunday).
    Daily { minute_of_day: u16, weekdays: u8 },
}

impl Schedule {
    pub fn check(&self) -> Result<(), String> {
        match *self {
            Schedule::Every { minutes } if minutes < MIN_EVERY_MIN => Err(format!("an interval is at least {MIN_EVERY_MIN} minutes")),
            Schedule::Every { minutes } if minutes > 60 * 24 * 31 => Err("an interval is at most 31 days".into()),
            Schedule::Daily { minute_of_day, .. } if minute_of_day >= 1440 => Err("the time of day is out of range".into()),
            Schedule::Daily { weekdays, .. } if weekdays & ALL_DAYS == 0 || weekdays & !ALL_DAYS != 0 => Err("choose at least one weekday".into()),
            _ => Ok(()),
        }
    }
}

/// A local calendar date and time of day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    pub year: i32,
    /// 1..=12
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    /// 0 Monday .. 6 Sunday
    pub weekday: u32,
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
    /// The local date and time at `ms`.
    fn local(&self, ms: i64) -> Local;
    /// The moment of a local date and time; a day or hour out of range rolls over (day 32 is the next month's first), and a time
    /// that does not exist (summer time starts) moves forward.
    fn instant(&self, year: i32, month: u32, day: i64, hour: u32, minute: u32) -> i64;
}

/// The next due time strictly after `after_ms` (None for an invalid schedule).
pub fn next_due(schedule: &Schedule, after_ms: i64, clock: &dyn Clock) -> Option<i64> {
    schedule.check().ok()?;
    match *schedule {
        Schedule::Every { minutes } => Some(after_ms + i64::from(minutes) * 60_000),
        Schedule::Daily { minute_of_day, weekdays } => {
            let start = clock.local(after_ms);
            // A week and a day ahead always holds a chosen weekday.
            for offset in 0..=8 {
                let at = clock.instant(start.year, start.month, i64::from(start.day) + offset, u32::from(minute_of_day) / 60, u32::from(minute_of_day) % 60);
                if at <= after_ms {
                    continue;
                }
                let found = clock.local(at);
                // When the clocks go back, the same wall-clock time happens twice: having just passed it, it is not due again an hour later.
                let again = u32::from(minute_of_day) == start.hour * 60 + start.minute && (found.year, found.month, found.day) == (start.year, start.month, start.day);
                if !again && weekdays & (1 << found.weekday) != 0 {
                    return Some(at);
                }
            }
            None
        }
    }
}

/// The machine's own time zone.
pub struct SystemClock;

#[cfg(unix)]
unsafe fn mktime_local(tm: &mut libc::tm) -> libc::time_t {
    libc::mktime(tm)
}

#[cfg(windows)]
unsafe fn mktime_local(tm: &mut libc::tm) -> libc::time_t {
    extern "C" {
        fn _mktime64(tm: *mut libc::tm) -> libc::time_t;
    }
    _mktime64(tm)
}

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        crate::now_ms() as i64
    }

    fn local(&self, ms: i64) -> Local {
        let secs = ms.div_euclid(1000) as libc::time_t;
        // SAFETY: `tm` is plain data; localtime_r fills it from `secs` and does not keep either pointer.
        let tm = unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            #[cfg(unix)]
            libc::localtime_r(&secs, &mut tm);
            #[cfg(windows)]
            libc::localtime_s(&mut tm, &secs);
            tm
        };
        Local { year: tm.tm_year + 1900, month: (tm.tm_mon + 1) as u32, day: tm.tm_mday as u32, hour: tm.tm_hour as u32, minute: tm.tm_min as u32, weekday: ((tm.tm_wday + 6) % 7) as u32 }
    }

    fn instant(&self, year: i32, month: u32, day: i64, hour: u32, minute: u32) -> i64 {
        // SAFETY: as above; mktime normalises the fields and reads tm_isdst = -1 as "work it out".
        let secs = unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            tm.tm_year = year - 1900;
            tm.tm_mon = month as i32 - 1;
            tm.tm_mday = day as i32;
            tm.tm_hour = hour as i32;
            tm.tm_min = minute as i32;
            tm.tm_isdst = -1;
            mktime_local(&mut tm)
        };
        // time_t is 64 bits on this platform and 32 on some others.
        #[allow(clippy::useless_conversion)]
        let secs = i64::from(secs);
        secs * 1000
    }
}

/// A clock a fixed number of minutes ahead of UTC with a settable "now" (tests).
pub struct FixedClock {
    pub offset_min: i64,
    pub now: std::sync::atomic::AtomicI64,
}

impl FixedClock {
    pub fn new(offset_min: i64, now_ms: i64) -> Self {
        Self { offset_min, now: std::sync::atomic::AtomicI64::new(now_ms) }
    }

    pub fn set(&self, ms: i64) {
        self.now.store(ms, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Days since 1970-01-01 of a civil date (the day may be out of range: it rolls over).
fn days_from_civil(year: i32, month: u32, day: i64) -> i64 {
    // Months past December or before January move the year.
    let m0 = i64::from(month) - 1;
    let year = i64::from(year) + m0.div_euclid(12);
    let month = m0.rem_euclid(12) + 1;
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy + day - 1 - 719_468
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.now.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn local(&self, ms: i64) -> Local {
        let local_min = ms.div_euclid(60_000) + self.offset_min;
        let days = local_min.div_euclid(1440);
        let in_day = local_min.rem_euclid(1440);
        let (year, month, day) = civil_from_days(days);
        // 1970-01-01 was a Thursday.
        Local { year, month, day, hour: (in_day / 60) as u32, minute: (in_day % 60) as u32, weekday: (days + 3).rem_euclid(7) as u32 }
    }

    fn instant(&self, year: i32, month: u32, day: i64, hour: u32, minute: u32) -> i64 {
        let days = days_from_civil(year, month, day);
        (days * 1440 + i64::from(hour) * 60 + i64::from(minute) - self.offset_min) * 60_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    fn at(clock: &FixedClock, y: i32, m: u32, d: i64, h: u32, mi: u32) -> i64 {
        clock.instant(y, m, d, h, mi)
    }

    #[test]
    fn the_calendar_maths_agrees_with_known_dates() {
        let clock = FixedClock::new(0, 0);
        // 2026-11-07 is a Saturday.
        let l = clock.local(at(&clock, 2026, 11, 7, 12, 30));
        assert_eq!((l.year, l.month, l.day, l.hour, l.minute, l.weekday), (2026, 11, 7, 12, 30, 5));
        // 1970-01-01 Thursday; 2000-02-29 Tuesday; the day rolls over month and year ends.
        assert_eq!(clock.local(0).weekday, 3);
        assert_eq!(clock.local(at(&clock, 2000, 2, 29, 0, 0)).weekday, 1);
        let l = clock.local(at(&clock, 2026, 12, 32, 0, 0));
        assert_eq!((l.year, l.month, l.day), (2027, 1, 1));
        let l = clock.local(at(&clock, 2026, 2, 29, 0, 0));
        assert_eq!((l.year, l.month, l.day), (2026, 3, 1));
        assert_eq!(clock.local(at(&clock, 2026, 11, 7, 12, 30) + 5 * MIN).minute, 35);
    }

    #[test]
    fn a_daily_task_runs_at_its_time_on_its_weekdays() {
        // UTC+9 (Korea): 09:00 on Monday..Friday.
        let clock = FixedClock::new(9 * 60, 0);
        let schedule = Schedule::Daily { minute_of_day: 9 * 60, weekdays: 0b001_1111 };
        // Saturday 2026-11-07 10:00 local: the next one is Monday 09:00.
        let sat = at(&clock, 2026, 11, 7, 10, 0);
        let next = next_due(&schedule, sat, &clock).unwrap();
        let l = clock.local(next);
        assert_eq!((l.month, l.day, l.hour, l.minute, l.weekday), (11, 9, 9, 0, 0));
        // Before the time on the same day: that day. At exactly the time: the next allowed day (strictly after).
        let mon_early = at(&clock, 2026, 11, 9, 8, 0);
        assert_eq!(next_due(&schedule, mon_early, &clock), Some(at(&clock, 2026, 11, 9, 9, 0)));
        assert_eq!(next_due(&schedule, at(&clock, 2026, 11, 9, 9, 0), &clock), Some(at(&clock, 2026, 11, 10, 9, 0)));
        // Friday after the time: Monday.
        let fri = at(&clock, 2026, 11, 13, 9, 1);
        assert_eq!(clock.local(next_due(&schedule, fri, &clock).unwrap()).weekday, 0);
    }

    #[test]
    fn a_daily_task_crosses_midnight_and_a_year_end() {
        let clock = FixedClock::new(0, 0);
        let schedule = Schedule::Daily { minute_of_day: 30, weekdays: ALL_DAYS };
        let next = next_due(&schedule, at(&clock, 2026, 12, 31, 23, 50), &clock).unwrap();
        let l = clock.local(next);
        assert_eq!((l.year, l.month, l.day, l.hour, l.minute), (2027, 1, 1, 0, 30));
        // Only Sundays, asked on a Sunday after the time: seven days on.
        let sunday = Schedule::Daily { minute_of_day: 600, weekdays: 0b100_0000 };
        let sun = at(&clock, 2026, 11, 8, 11, 0);
        assert_eq!(next_due(&sunday, sun, &clock), Some(at(&clock, 2026, 11, 15, 10, 0)));
    }

    #[test]
    fn an_interval_adds_its_minutes_and_a_bad_schedule_has_no_time() {
        let clock = FixedClock::new(0, 0);
        assert_eq!(next_due(&Schedule::Every { minutes: 30 }, 1000, &clock), Some(1000 + 30 * MIN));
        assert_eq!(next_due(&Schedule::Every { minutes: 14 }, 1000, &clock), None);
        assert_eq!(next_due(&Schedule::Daily { minute_of_day: 1440, weekdays: ALL_DAYS }, 0, &clock), None);
        assert_eq!(next_due(&Schedule::Daily { minute_of_day: 10, weekdays: 0 }, 0, &clock), None);
        assert_eq!(next_due(&Schedule::Daily { minute_of_day: 10, weekdays: 0b1000_0000 }, 0, &clock), None);
        assert!(Schedule::Every { minutes: 15 }.check().is_ok());
        assert!(Schedule::Every { minutes: 60 * 24 * 32 }.check().is_err());
    }

    /// A clock in a zone whose clocks go back one hour at `switch`, and whose `instant` answers the LATER of a repeated wall-clock time
    /// (some C libraries do; glibc answers the earlier, so the system clock cannot show this).
    struct FoldClock {
        switch: i64,
    }

    impl Clock for FoldClock {
        fn now_ms(&self) -> i64 {
            0
        }

        fn local(&self, ms: i64) -> Local {
            FixedClock::new(if ms < self.switch { 120 } else { 60 }, 0).local(ms)
        }

        fn instant(&self, year: i32, month: u32, day: i64, hour: u32, minute: u32) -> i64 {
            let later = FixedClock::new(60, 0).instant(year, month, day, hour, minute);
            if later >= self.switch { later } else { FixedClock::new(120, 0).instant(year, month, day, hour, minute) }
        }
    }

    #[test]
    fn a_wall_clock_time_that_happens_twice_is_not_due_again_an_hour_later() {
        let utc = FixedClock::new(0, 0);
        let clock = FoldClock { switch: utc.instant(2026, 10, 25, 1, 0) };
        let schedule = Schedule::Daily { minute_of_day: 2 * 60 + 30, weekdays: ALL_DAYS };
        // The first 02:30 is 00:30 UTC; the second is 01:30 UTC. Asked just after the first, the answer is tomorrow's, not the second.
        let first = utc.instant(2026, 10, 25, 0, 30);
        assert_eq!(clock.local(first).hour, 2);
        let next = next_due(&schedule, first + 20_000, &clock).unwrap();
        let l = clock.local(next);
        assert_eq!((l.month, l.day, l.hour, l.minute), (10, 26, 2, 30), "not the repeated 02:30 of the same day");
        // Asked before the first one, it is due that day.
        assert_eq!(clock.local(next_due(&schedule, utc.instant(2026, 10, 24, 12, 0), &clock).unwrap()).day, 25);
    }
}
