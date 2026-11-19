//! The workbench of a chat: the task contract (goal, rules, non-goals, done conditions), the pinned context and the guard's settings.
//! It is this app's own version of what the SmartWorkbench plug-in does, kept in a file of the chat (see `store`) and added to what the
//! model is told (see `inject`). Nothing here runs a model or reads a secret; the person's words are never logged.
pub mod hook;
pub mod inject;
pub mod risk;
pub mod rules;
pub mod store;

use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
const MAX_TEXT: usize = 500;
const MAX_ITEMS: usize = 30;
const MAX_PINS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Off,
    Permissive,
    #[default]
    Balanced,
    Strict,
}

impl Profile {
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "off" => Self::Off,
            "low" | "permissive" => Self::Permissive,
            "medium" | "balanced" => Self::Balanced,
            "high" | "strict" => Self::Strict,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Constraint {
    pub id: u32,
    pub text: String,
    /// Hard (`true`) or soft.
    pub hard: bool,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    pub id: u32,
    pub text: String,
    /// The kind of check that proves it: test, build, typecheck, lint or ci (None: a person marks it).
    pub link: Option<String>,
    pub met: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PinKind {
    /// A file of the project, read when a message is sent (`from`/`to`: a line range, 1-based, both inclusive).
    File { path: String, from: Option<u32>, to: Option<u32> },
    Note { text: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub id: u32,
    #[serde(flatten)]
    pub kind: PinKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Task {
    pub locked: bool,
    pub goal: String,
    pub constraints: Vec<Constraint>,
    pub non_goals: Vec<String>,
    pub done: Vec<Done>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workbench {
    pub version: u32,
    pub rev: u64,
    pub updated: u64,
    pub next_id: u32,
    pub task: Task,
    pub pins: Vec<Pin>,
    pub profile: Profile,
    pub paused: bool,
}

impl Default for Workbench {
    fn default() -> Self {
        Self { version: VERSION, rev: 0, updated: 0, next_id: 1, task: Task::default(), pins: Vec::new(), profile: Profile::default(), paused: false }
    }
}

/// One change to a workbench, from the person (a button) or later from Claude's tools (which can only propose).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    SetGoal { text: String },
    AddConstraint { text: String, hard: bool },
    RemoveConstraint { id: u32 },
    ToggleConstraint { id: u32 },
    SetConstraintHard { id: u32, hard: bool },
    AddNonGoal { text: String },
    RemoveNonGoal { index: usize },
    AddDone { text: String, link: Option<String> },
    RemoveDone { id: u32 },
    MarkDone { id: u32, met: bool },
    PinFile { path: String, from: Option<u32>, to: Option<u32> },
    PinNote { text: String },
    Unpin { id: u32 },
    Lock,
    Unlock,
    /// A fresh contract (the pins and the guard settings stay).
    New,
    SetProfile { profile: Profile },
    SetPaused { paused: bool },
}

pub const LINKS: [&str; 5] = ["test", "build", "typecheck", "lint", "ci"];

/// Control and invisible characters out, ends trimmed, cut to the limit; None when nothing is left.
pub fn clean(text: &str, max: usize) -> Option<String> {
    let t: String = text.chars().filter(|c| (!c.is_control() && !risk::is_invisible(*c)) || *c == ' ').collect::<String>().trim().chars().take(max).collect();
    (!t.is_empty()).then_some(t)
}

fn text_of(text: &str) -> Result<String, &'static str> {
    clean(text, MAX_TEXT).ok_or("wb_err_blank")
}

impl Workbench {
    fn id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Applies one change. Everything that edits the contract is refused while it is locked (unlock first), and a lock needs a goal.
    pub fn apply(&mut self, op: Op) -> Result<(), &'static str> {
        let editing = !matches!(op, Op::Lock | Op::Unlock | Op::New | Op::SetProfile { .. } | Op::SetPaused { .. } | Op::PinFile { .. } | Op::PinNote { .. } | Op::Unpin { .. });
        if editing && self.task.locked {
            return Err("wb_err_locked");
        }
        match op {
            Op::SetGoal { text } => self.task.goal = text_of(&text)?,
            Op::AddConstraint { text, hard } => {
                let text = text_of(&text)?;
                if self.task.constraints.len() >= MAX_ITEMS {
                    return Err("wb_err_many");
                }
                let id = self.id();
                self.task.constraints.push(Constraint { id, text, hard, active: true });
            }
            Op::RemoveConstraint { id } => {
                let before = self.task.constraints.len();
                self.task.constraints.retain(|c| c.id != id);
                if self.task.constraints.len() == before {
                    return Err("wb_err_gone");
                }
            }
            Op::ToggleConstraint { id } => {
                let c = self.task.constraints.iter_mut().find(|c| c.id == id).ok_or("wb_err_gone")?;
                c.active = !c.active;
            }
            Op::SetConstraintHard { id, hard } => self.task.constraints.iter_mut().find(|c| c.id == id).ok_or("wb_err_gone")?.hard = hard,
            Op::AddNonGoal { text } => {
                let text = text_of(&text)?;
                if self.task.non_goals.len() >= MAX_ITEMS {
                    return Err("wb_err_many");
                }
                self.task.non_goals.push(text);
            }
            Op::RemoveNonGoal { index } => {
                if index >= self.task.non_goals.len() {
                    return Err("wb_err_gone");
                }
                self.task.non_goals.remove(index);
            }
            Op::AddDone { text, link } => {
                let text = text_of(&text)?;
                let link = match link.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
                    None => None,
                    Some(l) if LINKS.contains(&l) => Some(l.to_owned()),
                    Some(_) => return Err("wb_err_link"),
                };
                if self.task.done.len() >= MAX_ITEMS {
                    return Err("wb_err_many");
                }
                let id = self.id();
                self.task.done.push(Done { id, text, link, met: false });
            }
            Op::RemoveDone { id } => {
                let before = self.task.done.len();
                self.task.done.retain(|d| d.id != id);
                if self.task.done.len() == before {
                    return Err("wb_err_gone");
                }
            }
            Op::MarkDone { id, met } => self.task.done.iter_mut().find(|d| d.id == id).ok_or("wb_err_gone")?.met = met,
            Op::PinFile { path, from, to } => {
                let path = inject::pin_path(&path)?;
                // Lines count from 1, an end is not before its start, and a start may stand alone.
                if from == Some(0) || to == Some(0) || matches!((from, to), (Some(a), Some(b)) if b < a) || (from.is_none() && to.is_some()) {
                    return Err("wb_err_range");
                }
                self.add_pin(PinKind::File { path, from, to })?;
            }
            Op::PinNote { text } => {
                let text = text_of(&text)?;
                self.add_pin(PinKind::Note { text })?;
            }
            Op::Unpin { id } => {
                let before = self.pins.len();
                self.pins.retain(|p| p.id != id);
                if self.pins.len() == before {
                    return Err("wb_err_gone");
                }
            }
            Op::Lock => {
                if self.task.goal.is_empty() {
                    return Err("wb_err_need_goal");
                }
                self.task.locked = true;
            }
            Op::Unlock => self.task.locked = false,
            Op::New => self.task = Task::default(),
            Op::SetProfile { profile } => self.profile = profile,
            Op::SetPaused { paused } => self.paused = paused,
        }
        self.rev += 1;
        self.updated = crate::now_ms();
        Ok(())
    }

    fn add_pin(&mut self, kind: PinKind) -> Result<(), &'static str> {
        if self.pins.len() >= MAX_PINS {
            return Err("wb_err_many");
        }
        let id = self.id();
        self.pins.push(Pin { id, kind });
        Ok(())
    }

    /// A plain text summary (what `/swb status` prints).
    pub fn status_text(&self) -> String {
        let t = &self.task;
        let mut out = format!("Goal: {}{}\n", if t.goal.is_empty() { "—" } else { &t.goal }, if t.locked { " (locked)" } else { " (unlocked)" });
        for c in t.constraints.iter().filter(|c| c.active) {
            out.push_str(&format!("[{}] {}\n", if c.hard { "H" } else { "S" }, c.text));
        }
        for n in &t.non_goals {
            out.push_str(&format!("Non-goal: {n}\n"));
        }
        for d in &t.done {
            out.push_str(&format!("{} {}{}\n", if d.met { "✓" } else { "○" }, d.text, d.link.as_ref().map(|l| format!(" [{l}]")).unwrap_or_default()));
        }
        out.push_str(&format!("Pins: {}", self.pins.len()));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_contract_is_edited_locked_and_refuses_edits_while_locked() {
        let mut w = Workbench::default();
        assert_eq!(w.apply(Op::Lock), Err("wb_err_need_goal"));
        w.apply(Op::SetGoal { text: "  ship it\u{7} ".into() }).unwrap();
        assert_eq!(w.task.goal, "ship it");
        w.apply(Op::AddConstraint { text: "no new deps".into(), hard: true }).unwrap();
        w.apply(Op::AddNonGoal { text: "refactoring".into() }).unwrap();
        w.apply(Op::AddDone { text: "tests pass".into(), link: Some("test".into()) }).unwrap();
        assert_eq!(w.apply(Op::AddDone { text: "x".into(), link: Some("deploy".into()) }), Err("wb_err_link"));
        assert_eq!(w.apply(Op::SetGoal { text: "   ".into() }), Err("wb_err_blank"));
        w.apply(Op::Lock).unwrap();
        assert_eq!(w.apply(Op::SetGoal { text: "other".into() }), Err("wb_err_locked"));
        assert_eq!(w.apply(Op::AddNonGoal { text: "y".into() }), Err("wb_err_locked"));
        // Pins and the guard's settings are not part of the contract: they change while it is locked.
        w.apply(Op::PinNote { text: "remember".into() }).unwrap();
        w.apply(Op::SetProfile { profile: Profile::Strict }).unwrap();
        w.apply(Op::Unlock).unwrap();
        let id = w.task.done[0].id;
        w.apply(Op::MarkDone { id, met: true }).unwrap();
        assert!(w.status_text().contains("✓ tests pass [test]") && w.status_text().contains("[H] no new deps") && w.status_text().contains("Pins: 1"));
        w.apply(Op::New).unwrap();
        assert!(w.task.goal.is_empty() && w.task.done.is_empty() && w.pins.len() == 1);
        assert!(w.rev >= 10);
    }

    #[test]
    fn limits_hold_and_unknown_ids_are_refused() {
        let mut w = Workbench::default();
        for n in 0..MAX_ITEMS {
            w.apply(Op::AddNonGoal { text: format!("n{n}") }).unwrap();
        }
        assert_eq!(w.apply(Op::AddNonGoal { text: "one more".into() }), Err("wb_err_many"));
        assert_eq!(w.apply(Op::RemoveConstraint { id: 99 }), Err("wb_err_gone"));
        assert_eq!(w.apply(Op::Unpin { id: 99 }), Err("wb_err_gone"));
        w.apply(Op::SetGoal { text: "g".repeat(900) }).unwrap();
        assert_eq!(w.task.goal.chars().count(), MAX_TEXT);
        assert_eq!(Profile::parse("low"), Some(Profile::Permissive));
        assert_eq!(Profile::parse("loud"), None);
    }
}

/// The command Claude Code runs as the PreToolUse hook of a chat: `"<exe>" guard --data "<folder>" --project "<folder>" --sid <chat> || exit 2`.
/// The shell reads it, so a path holding a character the quotes cannot hold (or that is not UTF-8) is refused: the chat then does not start,
/// rather than run without its guard. The `|| exit 2` turns every way the hook can fail (no program, a crash, a signal) into a block: Claude
/// Code runs a call whose hook fails any other way. On Windows the paths are written with forward slashes and without the `\\?\` prefix.
pub fn hook_command(exe: &std::path::Path, data: &std::path::Path, project: &std::path::Path, sid: &str) -> Result<String, String> {
    fn shell_path(path: &std::path::Path) -> Result<String, String> {
        let text = path.to_str().ok_or("the app's path is not plain text")?.to_owned();
        let text = if cfg!(windows) { text.strip_prefix(r"\\?\").unwrap_or(&text).replace('\\', "/") } else { text };
        if text.chars().any(|c| matches!(c, '"' | '$' | '`' | '\\' | '\n' | '\r' | '%' | '!') || c.is_control()) {
            return Err("the app's path holds a character the guard hook's command cannot carry".into());
        }
        Ok(text)
    }
    if sid.is_empty() || !sid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("a chat id that is not plain".into());
    }
    Ok(format!("\"{}\" guard --data \"{}\" --project \"{}\" --sid {sid} || exit 2", shell_path(exe)?, shell_path(data)?, shell_path(project)?))
}

#[cfg(test)]
mod hook_command_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_hook_command_quotes_paths_and_refuses_what_it_cannot_carry() {
        let c = hook_command(Path::new("/opt/tenrec app/tenrec"), Path::new("/home/me/.local/share/tenrec"), Path::new("/work/p"), "ab12").unwrap();
        assert_eq!(c, "\"/opt/tenrec app/tenrec\" guard --data \"/home/me/.local/share/tenrec\" --project \"/work/p\" --sid ab12 || exit 2");
        assert!(hook_command(Path::new("/opt/te\"nrec"), Path::new("/d"), Path::new("/p"), "ab").is_err());
        assert!(hook_command(Path::new("/opt/t$HOME"), Path::new("/d"), Path::new("/p"), "ab").is_err());
        assert!(hook_command(Path::new("/opt/t"), Path::new("/d"), Path::new("/p"), "a b").is_err());
    }
}
