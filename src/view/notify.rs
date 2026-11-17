//! Which chats have started to wait for an answer since the last look, for a desktop notification. A chat on screen needs none (its
//! dialog is in front of the person), and what was already waiting when the app started is not announced.
use std::collections::HashMap;

#[derive(Default)]
pub struct Waiting {
    seen: HashMap<String, u32>,
    started: bool,
}

impl Waiting {
    /// `chats`: every chat with its number of waiting permission requests; `current`: the chat on screen. Returns the chats to tell
    /// the person about (those whose number grew), and remembers the numbers.
    pub fn update(&mut self, chats: &[(String, u32)], current: Option<&str>) -> Vec<String> {
        let mut tell = Vec::new();
        for (sid, n) in chats {
            let before = self.seen.get(sid).copied().unwrap_or(0);
            if self.started && *n > before && current != Some(sid.as_str()) {
                tell.push(sid.clone());
            }
            self.seen.insert(sid.clone(), *n);
        }
        // A chat that is gone is forgotten.
        self.seen.retain(|sid, _| chats.iter().any(|(s, _)| s == sid));
        self.started = true;
        tell
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chats(list: &[(&str, u32)]) -> Vec<(String, u32)> {
        list.iter().map(|(s, n)| (s.to_string(), *n)).collect()
    }

    #[test]
    fn a_chat_that_starts_to_wait_is_announced_once() {
        let mut w = Waiting::default();
        // What waits at the start is not announced.
        assert!(w.update(&chats(&[("a", 1), ("b", 0)]), Some("b")).is_empty());
        assert!(w.update(&chats(&[("a", 1), ("b", 0)]), Some("b")).is_empty(), "still waiting: nothing new");
        assert_eq!(w.update(&chats(&[("a", 1), ("b", 0), ("c", 1)]), Some("b")), ["c"]);
        assert!(w.update(&chats(&[("a", 1), ("b", 0), ("c", 1)]), Some("b")).is_empty());
        // One more request in a chat that already waits is news again; fewer is not.
        assert_eq!(w.update(&chats(&[("a", 2), ("b", 0), ("c", 0)]), Some("b")), ["a"]);
        assert!(w.update(&chats(&[("a", 1), ("b", 0), ("c", 0)]), Some("b")).is_empty());
    }

    #[test]
    fn the_chat_on_screen_and_a_forgotten_chat_are_not_announced() {
        let mut w = Waiting::default();
        w.update(&chats(&[("a", 0)]), Some("a"));
        assert!(w.update(&chats(&[("a", 1)]), Some("a")).is_empty(), "its dialog is in front of the person");
        // Gone and back: it counts from nothing again.
        w.update(&chats(&[]), None);
        assert_eq!(w.update(&chats(&[("a", 1)]), None), ["a"]);
    }
}
