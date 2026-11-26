//! Signs that a chat has lost its way, read from its tool calls alone (no model, no clock): the same call over and over, and a very
//! wide spread of edits. A person looking at a long run cannot see either at a glance; both cost tokens and time when nobody notices.
//!
//! The judgement is made here once and shown in two places (the transcript and the live view), and it depends only on the calls in
//! order, so following a chat and replaying its log say the same things.
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};

use serde_json::Value;

/// The same call this many times in a row is said once. Three raised false alarms in other tools (a test run again after each small
/// edit is not a loop); five in a row, with an identical input, is.
pub const LOOP_AT: u32 = 5;

/// Said again for every this many different files edited in the chat.
pub const SCOPE_STEP: usize = 20;

/// Tools that are asked the same thing again and again by design (waiting for a background job, reading a task list).
const POLLING: [&str; 4] = ["BashOutput", "TaskOutput", "TaskList", "TaskGet"];

const EDIT_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Alert {
    /// The same tool with the same whole input, `times` calls in a row.
    Loop { tool: String, times: u32 },
    /// `files` different files edited in this chat so far.
    Scope { files: usize },
}

#[derive(Debug, Default)]
pub struct Watch {
    last: Option<(String, u64)>,
    run: u32,
    files: HashSet<String>,
    said_files: usize,
    seen: HashSet<String>,
}

impl Watch {
    pub fn new() -> Self {
        Self::default()
    }

    /// One tool call of the chat, in order; what it makes worth saying (usually nothing). A call is counted once however many times its
    /// `id` is sent (a message is written more than once while it streams).
    pub fn call(&mut self, id: &str, name: &str, input: &Value, sub_agent: bool) -> Vec<Alert> {
        if !self.seen.insert(id.to_owned()) {
            return Vec::new();
        }
        self.observe(name, input, sub_agent)
    }

    /// The rules, for a call already known to be new. `sub_agent` is a call a sub-agent made: it neither counts toward the chat's own streak nor
    /// breaks it (a sub-agent doing its own work between two identical calls does not make them different), but the files it edits
    /// count toward the spread.
    fn observe(&mut self, name: &str, input: &Value, sub_agent: bool) -> Vec<Alert> {
        let mut out: Vec<Alert> = self.scope(name, input).into_iter().collect();
        if sub_agent || POLLING.contains(&name) {
            return out;
        }
        let key = (name.to_owned(), fingerprint(input));
        if self.last.as_ref() == Some(&key) {
            self.run += 1;
        } else {
            self.last = Some(key);
            self.run = 1;
        }
        if self.run == LOOP_AT {
            out.push(Alert::Loop { tool: name.chars().take(80).collect(), times: LOOP_AT });
        }
        out
    }

    /// How many different calls were seen in the chat so far.
    pub fn calls(&self) -> usize {
        self.seen.len()
    }

    /// How many different files the chat edited so far.
    pub fn edited(&self) -> usize {
        self.files.len()
    }

    fn scope(&mut self, name: &str, input: &Value) -> Option<Alert> {
        if !EDIT_TOOLS.contains(&name) {
            return None;
        }
        let path = ["file_path", "notebook_path"].iter().find_map(|k| input.get(*k).and_then(Value::as_str))?;
        self.files.insert(path.to_owned());
        if self.files.len() >= self.said_files + SCOPE_STEP {
            self.said_files = self.files.len() / SCOPE_STEP * SCOPE_STEP;
            return Some(Alert::Scope { files: self.said_files });
        }
        None
    }
}

/// A hash of the whole input that does not depend on the order its keys came in (a map may be written in either order).
fn fingerprint(value: &Value) -> u64 {
    let mut text = String::new();
    canonical(value, &mut text);
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for key in keys {
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                canonical(&map[key], out);
                out.push(',');
            }
            out.push('}');
        }
        Value::Array(list) => {
            out.push('[');
            for one in list {
                canonical(one, out);
                out.push(',');
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn run(watch: &mut Watch, name: &str, input: Value, times: usize) -> Vec<Vec<Alert>> {
        (0..times).map(|_| watch.observe(name, &input, false)).collect()
    }

    #[test]
    fn the_same_call_five_times_in_a_row_is_said_once_and_not_again_while_it_goes_on() {
        let mut w = Watch::new();
        let said = run(&mut w, "Bash", json!({"command": "cargo test"}), 8);
        assert_eq!(said.iter().filter(|a| !a.is_empty()).count(), 1);
        assert_eq!(said[4], [Alert::Loop { tool: "Bash".into(), times: 5 }], "on the fifth, not the fourth or sixth");
    }

    #[test]
    fn a_different_call_in_between_starts_the_count_again_and_a_new_streak_can_be_said_again() {
        let mut w = Watch::new();
        assert!(run(&mut w, "Read", json!({"file_path": "a.rs"}), 4).iter().all(Vec::is_empty));
        assert!(w.observe("Read", &json!({"file_path": "b.rs"}), false).is_empty());
        assert!(run(&mut w, "Read", json!({"file_path": "a.rs"}), 4).iter().all(Vec::is_empty), "four after the break is not five");
        assert!(!w.observe("Read", &json!({"file_path": "a.rs"}), false).is_empty(), "the fifth of the new streak");
        // The same tool with another input is another call, and a same input under another tool is too.
        let mut w = Watch::new();
        for i in 0..10 {
            assert!(w.observe("Bash", &json!({"command": format!("echo {i}")}), false).is_empty());
        }
        for _ in 0..4 {
            assert!(w.observe("Bash", &json!({"command": "ls"}), false).is_empty());
        }
        assert!(w.observe("Grep", &json!({"command": "ls"}), false).is_empty());
    }

    #[test]
    fn key_order_does_not_make_two_inputs_different_and_nested_values_count() {
        let mut w = Watch::new();
        let a = json!({"pattern": "x", "path": "src", "opts": {"i": true, "n": 2}});
        let b = json!({"opts": {"n": 2, "i": true}, "path": "src", "pattern": "x"});
        for _ in 0..2 {
            w.observe("Grep", &a, false);
            w.observe("Grep", &b, false);
        }
        assert!(!w.observe("Grep", &a, false).is_empty(), "five calls, however the keys were written");
        let mut w = Watch::new();
        for n in [1, 2, 1, 2, 1, 2] {
            assert!(w.observe("Grep", &json!({"opts": {"n": n}}), false).is_empty());
        }
    }

    #[test]
    fn sub_agent_calls_and_polling_tools_neither_count_nor_break_a_streak() {
        let mut w = Watch::new();
        let same = json!({"command": "make"});
        for _ in 0..2 {
            w.observe("Bash", &same, false);
            assert!(w.observe("Read", &json!({"file_path": "x"}), true).is_empty());
        }
        w.observe("BashOutput", &json!({"bash_id": "1"}), false);
        w.observe("Bash", &same, false);
        for _ in 0..30 {
            assert!(w.observe("BashOutput", &json!({"bash_id": "1"}), false).is_empty(), "polling is no loop");
        }
        assert!(w.observe("Bash", &same, false).is_empty(), "the fourth of the chat's own calls");
        assert_eq!(w.observe("Bash", &same, false), [Alert::Loop { tool: "Bash".into(), times: 5 }], "the fifth of the chat's own calls");
    }

    #[test]
    fn edited_files_are_counted_once_each_and_said_at_twenty_and_forty() {
        let mut w = Watch::new();
        let mut said = Vec::new();
        for i in 0..45 {
            // The same file twice does not count twice.
            for _ in 0..2 {
                said.extend(w.observe("Edit", &json!({"file_path": format!("src/f{i}.rs"), "old_string": format!("{i}a"), "new_string": "b"}), false));
            }
        }
        assert_eq!(said, [Alert::Scope { files: 20 }, Alert::Scope { files: 40 }]);
        let mut w = Watch::new();
        for i in 0..19 {
            assert!(w.observe("Write", &json!({"file_path": format!("n{i}")}), false).is_empty());
        }
        assert!(!w.observe("NotebookEdit", &json!({"notebook_path": "a.ipynb"}), false).is_empty(), "the twentieth, by any edit tool");
        assert!(w.observe("Edit", &json!({"old_string": "no path"}), false).is_empty(), "an edit with no path is not counted");
        assert!(w.observe("Read", &json!({"file_path": "r"}), false).is_empty(), "a read is not an edit");
    }

    #[test]
    fn files_a_sub_agent_edits_count_toward_the_spread() {
        let mut w = Watch::new();
        let mut last = Vec::new();
        for i in 0..20 {
            last = w.observe("Edit", &json!({"file_path": format!("s{i}.rs")}), true);
        }
        assert_eq!(last, [Alert::Scope { files: 20 }]);
    }

    #[test]
    fn the_name_is_cut_short_and_the_result_is_the_same_in_a_second_run() {
        let long = "T".repeat(300);
        let go = || {
            let mut w = Watch::new();
            (0..5).flat_map(|_| w.observe(&long, &json!({}), false)).collect::<Vec<_>>()
        };
        let first = go();
        let Some(Alert::Loop { tool, .. }) = first.first() else { panic!("{first:?}") };
        assert_eq!(tool.chars().count(), 80);
        assert_eq!(first, go(), "a replay says the same thing");
    }

    #[test]
    fn a_call_that_is_both_the_fifth_repeat_and_the_twentieth_file_says_both() {
        let mut w = Watch::new();
        for i in 0..19 {
            w.observe("Edit", &json!({"file_path": format!("a{i}")}), false);
        }
        let same = json!({"file_path": "z"});
        let mut seen = Vec::new();
        for _ in 0..5 {
            seen.extend(w.observe("Write", &same, false));
        }
        assert_eq!(seen, [Alert::Scope { files: 20 }, Alert::Loop { tool: "Write".into(), times: 5 }]);
    }

    #[test]
    fn a_call_sent_again_with_the_same_id_is_one_call() {
        let mut w = Watch::new();
        let input = json!({"command": "ls"});
        for _ in 0..6 {
            assert!(w.call("same-id", "Bash", &input, false).is_empty());
        }
        for n in 0..3 {
            assert!(w.call(&format!("other-{n}"), "Bash", &input, false).is_empty(), "four distinct calls so far");
        }
        assert_eq!(w.call("last", "Bash", &input, false), [Alert::Loop { tool: "Bash".into(), times: 5 }], "the fifth distinct call");
    }
}
