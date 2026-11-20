//! What is worth a desktop notification, from how the chats look at each refresh: a chat that started to wait for a permission, a turn
//! that took a while and has ended, a program that stopped with an error. A chat the person is looking at needs none (its dialog or
//! its answer is in front of them), unless the app is not the window in front. What was so when the app started is not announced.
use std::collections::HashMap;

/// A turn shorter than this does not get a notice of its end (the person has hardly looked away).
pub const DONE_MIN_MS: u64 = 15_000;

/// What a chat is doing, as far as a notice goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Running,
    Idle,
    /// The program ended with an exit code other than 0 (ended by a signal, as when the app stops it, is `Other`).
    Crashed,
    Other,
}

impl Phase {
    /// How a chat's state reads for a notice: a program that ended with a code other than 0 stopped on its own; one that ended with no
    /// code was ended by a signal, as when this app stops it, and is nothing to tell.
    pub fn of(state: &crate::session::State) -> Self {
        use crate::session::State;
        match state {
            State::Running => Self::Running,
            State::Idle => Self::Idle,
            State::Exited { code: Some(c) } if *c != 0 => Self::Crashed,
            State::Exited { .. } => Self::Other,
        }
    }
}

/// What to tell, the most urgent last (the largest one is the one shown when several are due at once).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reason {
    Done,
    Failed,
    Permission,
}

#[derive(Debug, Clone)]
pub struct Chat {
    pub sid: String,
    /// Permission requests waiting.
    pub pending: u32,
    pub phase: Phase,
}

#[derive(Default)]
pub struct Watch {
    /// By chat: the requests that waited, what it was doing and since when (ms).
    seen: HashMap<String, (u32, Phase, u64)>,
    started: bool,
}

impl Watch {
    /// `chats`: every chat; `current`: the chat on screen; `in_front`: whether this app's window is the one in front. Returns what to
    /// tell, and remembers how things are.
    pub fn update(&mut self, chats: &[Chat], current: Option<&str>, in_front: bool, now_ms: u64) -> Vec<(String, Reason)> {
        let mut tell = Vec::new();
        for chat in chats {
            let before = self.seen.get(&chat.sid).copied();
            let (was_pending, was_phase, since) = before.unwrap_or((0, Phase::Other, now_ms));
            // Not seen by the person: another chat is on screen, or this app is not in front.
            let unseen = current != Some(chat.sid.as_str()) || !in_front;
            if self.started && unseen {
                if chat.pending > was_pending {
                    tell.push((chat.sid.clone(), Reason::Permission));
                }
                if chat.phase == Phase::Crashed && was_phase != Phase::Crashed && before.is_some() {
                    tell.push((chat.sid.clone(), Reason::Failed));
                }
                if was_phase == Phase::Running && chat.phase == Phase::Idle && now_ms.saturating_sub(since) >= DONE_MIN_MS {
                    tell.push((chat.sid.clone(), Reason::Done));
                }
            }
            let since = if chat.phase == Phase::Running && was_phase != Phase::Running { now_ms } else { since };
            self.seen.insert(chat.sid.clone(), (chat.pending, chat.phase, since));
        }
        // A chat that is gone is forgotten.
        self.seen.retain(|sid, _| chats.iter().any(|c| &c.sid == sid));
        self.started = true;
        tell
    }
}

/// What to say now. `waiting` is what was due and not yet said: it is kept (merged with `new`, the most urgent reason wins) until a
/// notice may go out, so one that came within `gap_ms` of the last is delayed, never lost; `last_ms` is when the last one went out.
pub fn gate(waiting: &mut Option<Reason>, new: impl IntoIterator<Item = Reason>, now_ms: u64, last_ms: u64, gap_ms: u64) -> Option<Reason> {
    *waiting = new.into_iter().chain(*waiting).max();
    if now_ms.saturating_sub(last_ms) < gap_ms {
        return None;
    }
    waiting.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(sid: &str, pending: u32, phase: Phase) -> Chat {
        Chat { sid: sid.into(), pending, phase }
    }

    #[test]
    fn a_chat_that_starts_to_wait_is_announced_once_and_what_waited_at_the_start_is_not() {
        let mut w = Watch::default();
        assert!(w.update(&[chat("a", 1, Phase::Idle), chat("b", 0, Phase::Idle)], Some("b"), true, 0).is_empty());
        assert!(w.update(&[chat("a", 1, Phase::Idle), chat("b", 0, Phase::Idle)], Some("b"), true, 1).is_empty(), "still waiting: nothing new");
        assert_eq!(w.update(&[chat("a", 1, Phase::Idle), chat("b", 0, Phase::Idle), chat("c", 1, Phase::Idle)], Some("b"), true, 2), [("c".into(), Reason::Permission)]);
        // One more request in a chat that already waits is news again; fewer is not.
        assert_eq!(w.update(&[chat("a", 2, Phase::Idle), chat("b", 0, Phase::Idle), chat("c", 0, Phase::Idle)], Some("b"), true, 3), [("a".into(), Reason::Permission)]);
        assert!(w.update(&[chat("a", 1, Phase::Idle), chat("b", 0, Phase::Idle), chat("c", 0, Phase::Idle)], Some("b"), true, 4).is_empty());
    }

    #[test]
    fn the_chat_in_front_needs_none_but_the_same_chat_with_the_app_behind_another_window_does() {
        let mut w = Watch::default();
        w.update(&[chat("a", 0, Phase::Idle)], Some("a"), true, 0);
        assert!(w.update(&[chat("a", 1, Phase::Idle)], Some("a"), true, 1).is_empty(), "its dialog is in front of the person");
        w.update(&[chat("a", 0, Phase::Idle)], Some("a"), true, 2);
        assert_eq!(w.update(&[chat("a", 1, Phase::Idle)], Some("a"), false, 3), [("a".into(), Reason::Permission)], "the app is not the window in front");
        // Gone and back: it counts from nothing again.
        w.update(&[], None, true, 4);
        assert_eq!(w.update(&[chat("a", 1, Phase::Idle)], None, true, 5), [("a".into(), Reason::Permission)]);
    }

    #[test]
    fn the_end_of_a_long_turn_is_announced_and_a_short_one_is_not() {
        let mut w = Watch::default();
        w.update(&[chat("a", 0, Phase::Idle)], None, true, 0);
        w.update(&[chat("a", 0, Phase::Running)], None, true, 1_000);
        assert!(w.update(&[chat("a", 0, Phase::Idle)], None, true, 4_000).is_empty(), "three seconds");
        w.update(&[chat("a", 0, Phase::Running)], None, true, 10_000);
        w.update(&[chat("a", 0, Phase::Running)], None, true, 20_000);
        assert_eq!(w.update(&[chat("a", 0, Phase::Idle)], None, true, 10_000 + DONE_MIN_MS), [("a".into(), Reason::Done)]);
        // Said once; and a turn of the chat on screen, with the app in front, is not.
        assert!(w.update(&[chat("a", 0, Phase::Idle)], None, true, 100_000).is_empty());
        w.update(&[chat("a", 0, Phase::Running)], Some("a"), true, 110_000);
        assert!(w.update(&[chat("a", 0, Phase::Idle)], Some("a"), true, 200_000).is_empty());
    }

    #[test]
    fn a_state_reads_as_a_phase_and_only_a_nonzero_exit_is_a_crash() {
        use crate::session::State;
        assert_eq!(Phase::of(&State::Running), Phase::Running);
        assert_eq!(Phase::of(&State::Idle), Phase::Idle);
        assert_eq!(Phase::of(&State::Exited { code: Some(2) }), Phase::Crashed);
        assert_eq!(Phase::of(&State::Exited { code: Some(0) }), Phase::Other);
        assert_eq!(Phase::of(&State::Exited { code: None }), Phase::Other, "ended by a signal: this app stopping it");
    }

    #[test]
    fn a_crash_already_there_at_the_first_look_or_in_the_chat_in_front_is_not_announced() {
        let mut w = Watch::default();
        // Met for the first time already crashed: not news (even once the start is over).
        w.update(&[chat("x", 0, Phase::Idle)], None, true, 0);
        assert!(w.update(&[chat("x", 0, Phase::Idle), chat("late", 0, Phase::Crashed)], None, true, 1).is_empty());
        // The chat in front, the app in front: the person sees it.
        w.update(&[chat("a", 0, Phase::Running)], Some("a"), true, 2);
        assert!(w.update(&[chat("a", 0, Phase::Crashed)], Some("a"), true, 3).is_empty());
    }

    #[test]
    fn a_notice_within_the_gap_waits_and_is_said_later_the_most_urgent_first() {
        let mut waiting = None;
        // A notice of an end, then a permission request two seconds later: neither is lost.
        assert_eq!(gate(&mut waiting, [Reason::Done], 100_000, 0, 5_000), Some(Reason::Done));
        assert_eq!(gate(&mut waiting, [Reason::Permission], 102_000, 100_000, 5_000), None, "too soon");
        assert_eq!(gate(&mut waiting, [], 103_000, 100_000, 5_000), None, "still too soon, and kept");
        assert_eq!(gate(&mut waiting, [Reason::Done], 106_000, 100_000, 5_000), Some(Reason::Permission), "the more urgent one is said");
        assert_eq!(gate(&mut waiting, [], 120_000, 106_000, 5_000), None, "nothing more is due");
    }

    #[test]
    fn a_program_that_stopped_with_an_error_is_announced_once_and_a_stop_by_signal_is_not() {
        let mut w = Watch::default();
        w.update(&[chat("a", 0, Phase::Running), chat("b", 0, Phase::Running)], None, true, 0);
        let told = w.update(&[chat("a", 0, Phase::Crashed), chat("b", 0, Phase::Other)], None, true, 1);
        assert_eq!(told, [("a".into(), Reason::Failed)]);
        assert!(w.update(&[chat("a", 0, Phase::Crashed), chat("b", 0, Phase::Other)], None, true, 2).is_empty());
        assert!(Reason::Permission > Reason::Failed && Reason::Failed > Reason::Done);
    }
}
