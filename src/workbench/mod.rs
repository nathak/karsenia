//! The workbench of a chat: the task contract (goal, rules, non-goals, done conditions), the pinned context and the guard's settings.
//! It is this app's own version of what the SmartWorkbench plug-in does, kept in a file of the chat (see `store`) and added to what the
//! model is told (see `inject`). Nothing here runs a model or reads a secret; the person's words are never logged.
pub mod hook;
pub mod inject;
pub mod risk;
pub mod rules;
pub mod store;
pub mod team;

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
    /// Hold the first stop of a turn while a done condition is unmet (see `stop_decision`). Off unless the person turns it on; a file
    /// written before this existed has no such field and reads as off.
    #[serde(default)]
    pub stop_gate: bool,
    /// The app's own team mode (helper subagents and efforts); old files have none and read as off.
    #[serde(default)]
    pub team: team::Team,
}

impl Default for Workbench {
    fn default() -> Self {
        Self { version: VERSION, rev: 0, updated: 0, next_id: 1, task: Task::default(), pins: Vec::new(), profile: Profile::default(), paused: false, stop_gate: false, team: team::Team::default() }
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
    SetStopGate { on: bool },
    /// Team mode on or off (not an edit of the contract: it applies while locked too).
    SetTeam { on: bool },
    SetLeadEffort { effort: Option<team::Effort> },
    /// A helper's model (None: the lead's) and effort.
    SetRole { role: team::Sub, model: Option<String>, effort: Option<team::Effort> },
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
        let editing = !matches!(op, Op::Lock | Op::Unlock | Op::New | Op::SetProfile { .. } | Op::SetPaused { .. } | Op::SetStopGate { .. } | Op::SetTeam { .. } | Op::SetLeadEffort { .. } | Op::SetRole { .. } | Op::PinFile { .. } | Op::PinNote { .. } | Op::Unpin { .. });
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
            Op::SetStopGate { on } => self.stop_gate = on,
            Op::SetTeam { on } => self.team.on = on,
            Op::SetLeadEffort { effort } => self.team.lead_effort = effort,
            Op::SetRole { role, model, effort } => {
                if model.as_deref().is_some_and(|m| !team::role_model_ok(m)) {
                    return Err("team_err_model");
                }
                *self.team.role_mut(role) = team::RoleSet { model, effort };
            }
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

// ---- done conditions: a check that they can be checked, and the hold on the first stop

/// What is wrong with the way a contract's done conditions are written. Said beside the list, never a reason to refuse one: the
/// person knows their task; these are the usual ways a condition ends up meaning nothing a machine (or the person, later) can test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lint {
    /// The condition (by id) is too short, or names nothing that can be looked at: no command, file, number or outcome.
    Vague(u32),
    /// The condition (by id) is a matter of taste ("clean", "works well"), which no check can settle.
    Subjective(u32),
    /// The condition (by id) has no kind of check linked, while others have.
    NoCheck(u32),
    /// Conditions exist but nothing says what must not be done to meet them (deleting or weakening a test is the usual way).
    NoBoundary,
    /// So many conditions that none of them gets looked at.
    TooMany,
}

impl Lint {
    /// The text key of its message.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Vague(_) => "lint_vague",
            Self::Subjective(_) => "lint_subjective",
            Self::NoCheck(_) => "lint_nocheck",
            Self::NoBoundary => "lint_noboundary",
            Self::TooMany => "lint_toomany",
        }
    }

    /// The condition it is about, when it is about one.
    pub fn about(&self) -> Option<u32> {
        match self {
            Self::Vague(id) | Self::Subjective(id) | Self::NoCheck(id) => Some(*id),
            Self::NoBoundary | Self::TooMany => None,
        }
    }
}

const MIN_DONE_CHARS: usize = 12;
const MAX_DONE_ADVISED: usize = 8;

// Something to look at in a condition: a digit, a quoted name, a path or file name, a tool people run, or the outcome of a run.
static CHECKABLE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)[0-9`/]|\w\.\w{1,5}\b|\b(?:cargo|npm|pnpm|yarn|pytest|make|tsc|clippy|eslint|ruff|go test|build|test|tests|lint)\b|\b(?:passes|pass|fails|returns|exits|prints|compiles)\b|통과|실패|반환|출력|오류 없|에러 없|컴파일")
        .expect("a lint pattern compiles")
});
static SUBJECTIVE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(?:good|clean|nice|better|works? well|properly|robust|elegant|intuitive)\b|깔끔|적절히|적절하게|자연스럽|원활|잘 ?(?:동작|작동|되)")
        .expect("a lint pattern compiles")
});

/// How the done conditions of a task could be better written. Empty when there are none or they read well.
pub fn lint_done(task: &Task) -> Vec<Lint> {
    let mut out = Vec::new();
    if task.done.is_empty() {
        return out;
    }
    let any_link = task.done.iter().any(|d| d.link.is_some());
    for d in &task.done {
        let vague = d.text.chars().count() < MIN_DONE_CHARS || (d.link.is_none() && !CHECKABLE.is_match(&d.text));
        if vague {
            out.push(Lint::Vague(d.id));
        } else if any_link && d.link.is_none() {
            out.push(Lint::NoCheck(d.id));
        }
        if SUBJECTIVE.is_match(&d.text) {
            out.push(Lint::Subjective(d.id));
        }
    }
    if !task.constraints.iter().any(|c| c.active) && task.non_goals.is_empty() {
        out.push(Lint::NoBoundary);
    }
    if task.done.len() > MAX_DONE_ADVISED {
        out.push(Lint::TooMany);
    }
    out
}

/// What a Stop hook says when a chat is about to end its turn: `Some(reason)` holds it once (the reason goes to the model, which then
/// carries on), `None` lets it end. It holds only when the person turned the gate on, the contract is locked, a condition is unmet, and
/// this stop is not already the one that follows a hold (`stop_hook_active`: Claude Code says so), so a chat is held at most once per
/// stop and a condition that cannot be met never traps it. A plan-mode turn is not held: it ends with a plan, not with work.
pub fn stop_decision(input: &serde_json::Value, wb: &Workbench) -> Option<String> {
    if input["stop_hook_active"].as_bool() == Some(true) || input["permission_mode"].as_str() == Some("plan") {
        return None;
    }
    if !wb.stop_gate || !wb.task.locked {
        return None;
    }
    let unmet: Vec<&Done> = wb.task.done.iter().filter(|d| !d.met).collect();
    if unmet.is_empty() {
        return None;
    }
    let mut reason = String::from("Some done conditions of this task are not marked as met:\n");
    for d in unmet.iter().take(MAX_ITEMS) {
        reason.push_str(&format!("- {}\n", d.text));
    }
    reason.push_str("For each one, check it now and say what you saw, or say plainly that it is not met yet and what is left. You cannot mark a condition as met yourself: the person does.");
    Some(reason)
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
    check_sid(sid)?;
    Ok(format!("\"{}\" guard --data \"{}\" --project \"{}\" --sid {sid} || exit 2", shell_path(exe)?, shell_path(data)?, shell_path(project)?))
}

/// The command of the Stop hook: it always ends `exit 0`, so that a failure of the hook can never stop a chat from ending its turn (on
/// a Stop hook, exit code 2 is the one that holds the stop; never use it here).
pub fn stop_command(exe: &std::path::Path, data: &std::path::Path, sid: &str) -> Result<String, String> {
    check_sid(sid)?;
    Ok(format!("\"{}\" stop-gate --data \"{}\" --sid {sid} || exit 0", shell_path(exe)?, shell_path(data)?))
}

fn check_sid(sid: &str) -> Result<(), String> {
    if sid.is_empty() || !sid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("a chat id that is not plain".into());
    }
    Ok(())
}

fn shell_path(path: &std::path::Path) -> Result<String, String> {
    let text = path.to_str().ok_or("the app's path is not plain text")?.to_owned();
    let text = if cfg!(windows) { text.strip_prefix(r"\\?\").unwrap_or(&text).replace('\\', "/") } else { text };
    if text.chars().any(|c| matches!(c, '"' | '$' | '`' | '\\' | '\n' | '\r' | '%' | '!') || c.is_control()) {
        return Err("the app's path holds a character the guard hook's command cannot carry".into());
    }
    Ok(text)
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

#[cfg(test)]
mod lint_and_stop_tests {
    use serde_json::json;

    use super::*;

    fn task(done: &[(&str, Option<&str>)], boundary: bool) -> Task {
        let mut wb = Workbench::default();
        wb.task.goal = "ship it".into();
        for (text, link) in done {
            wb.apply(Op::AddDone { text: (*text).into(), link: link.map(str::to_owned) }).unwrap();
        }
        if boundary {
            wb.apply(Op::AddNonGoal { text: "do not delete or weaken a test".into() }).unwrap();
        }
        wb.task
    }

    #[test]
    fn a_vague_or_subjective_condition_is_told_and_a_checkable_one_is_not() {
        let t = task(&[("done", None), ("the code is clean and works well", None), ("cargo test passes with no failures", Some("test")), ("`src/a.rs` has a function parse_line", None)], true);
        let lint = lint_done(&t);
        assert!(lint.contains(&Lint::Vague(1)), "{lint:?}");
        assert!(lint.contains(&Lint::Subjective(2)), "{lint:?}");
        assert!(!lint.iter().any(|l| l.about() == Some(3)), "a condition with a command, a link and an outcome is fine: {lint:?}");
        assert!(lint.contains(&Lint::NoCheck(4)) && !lint.contains(&Lint::Vague(4)), "{lint:?}");
        assert!(!lint.contains(&Lint::NoBoundary));
        assert_eq!(Lint::Vague(1).key(), "lint_vague");
    }

    #[test]
    fn korean_wording_is_read_and_a_word_that_only_looks_like_one_is_not() {
        let t = task(&[("코드가 깔끔하고 잘 동작함", None), ("cargo test 가 통과한다", Some("test")), ("파일을 잘라서 두 개로 나눈다 src/a.rs", Some("lint"))], true);
        let lint = lint_done(&t);
        assert!(lint.contains(&Lint::Subjective(1)));
        assert!(!lint.iter().any(|l| matches!(l, Lint::Subjective(id) if *id != 1)), "'잘라서' is not 'works well': {lint:?}");
    }

    #[test]
    fn a_missing_boundary_and_too_many_conditions_are_told_and_nothing_is_said_without_conditions() {
        assert!(lint_done(&Task::default()).is_empty());
        let t = task(&[("cargo test passes", Some("test"))], false);
        assert_eq!(lint_done(&t), [Lint::NoBoundary]);
        let many: Vec<(String, Option<&str>)> = (0..9).map(|i| (format!("cargo test case {i} passes"), Some("test"))).collect();
        let refs: Vec<(&str, Option<&str>)> = many.iter().map(|(a, b)| (a.as_str(), *b)).collect();
        assert!(lint_done(&task(&refs, true)).contains(&Lint::TooMany));
    }

    fn gated(met: &[bool]) -> Workbench {
        let mut wb = Workbench::default();
        wb.task.goal = "ship it".into();
        for (i, m) in met.iter().enumerate() {
            wb.apply(Op::AddDone { text: format!("condition number {i}"), link: None }).unwrap();
            if *m {
                wb.apply(Op::MarkDone { id: i as u32 + 1, met: true }).unwrap();
            }
        }
        wb.apply(Op::Lock).unwrap();
        wb.apply(Op::SetStopGate { on: true }).unwrap();
        wb
    }

    #[test]
    fn the_stop_is_held_once_only_when_the_gate_is_on_the_contract_locked_and_a_condition_unmet() {
        let wb = gated(&[true, false]);
        let reason = stop_decision(&json!({"hook_event_name": "Stop"}), &wb).expect("held");
        assert!(reason.contains("condition number 1") && !reason.contains("condition number 0"), "{reason}");
        assert!(reason.contains("cannot mark"), "{reason}");
        assert!(stop_decision(&json!({"stop_hook_active": true}), &wb).is_none(), "the stop that follows a hold is let through");
        assert!(stop_decision(&json!({"permission_mode": "plan"}), &wb).is_none());
        assert!(stop_decision(&json!({}), &gated(&[true, true])).is_none(), "everything met");
        let mut off = gated(&[false]);
        off.apply(Op::SetStopGate { on: false }).unwrap();
        assert!(stop_decision(&json!({}), &off).is_none(), "gate off");
        let mut open = gated(&[false]);
        open.apply(Op::Unlock).unwrap();
        assert!(stop_decision(&json!({}), &open).is_none(), "an unlocked contract holds nothing");
    }

    #[test]
    fn the_gate_can_be_switched_while_locked_and_an_old_file_reads_as_off() {
        let mut wb = gated(&[false]);
        assert!(wb.apply(Op::SetStopGate { on: false }).is_ok(), "not an edit of the contract");
        assert!(!wb.stop_gate);
        let mut value = serde_json::to_value(Workbench::default()).unwrap();
        value.as_object_mut().unwrap().remove("stop_gate");
        let old: Workbench = serde_json::from_value(value).unwrap();
        assert!(!old.stop_gate);
    }

    #[test]
    fn team_changes_apply_while_locked_reject_a_bad_model_and_old_files_read_as_off() {
        use team::{Effort, Sub};
        let mut wb = Workbench::default();
        wb.apply(Op::SetGoal { text: "g".into() }).unwrap();
        wb.apply(Op::Lock).unwrap();
        let rev = wb.rev;
        wb.apply(Op::SetTeam { on: true }).unwrap();
        wb.apply(Op::SetLeadEffort { effort: Some(Effort::High) }).unwrap();
        wb.apply(Op::SetRole { role: Sub::Advisor, model: Some("sonnet".into()), effort: Some(Effort::Low) }).unwrap();
        assert!(wb.team.on && wb.team.lead_effort == Some(Effort::High));
        assert_eq!(wb.team.advisor.model.as_deref(), Some("sonnet"));
        assert_eq!(wb.rev, rev + 3);
        let before = wb.clone();
        assert_eq!(wb.apply(Op::SetRole { role: Sub::Explorer, model: Some("gpt 4".into()), effort: None }), Err("team_err_model"));
        assert_eq!(wb, before, "a refused change leaves everything as it was");
        let mut old = serde_json::to_value(Workbench::default()).unwrap();
        old.as_object_mut().unwrap().remove("team");
        let read: Workbench = serde_json::from_value(old).unwrap();
        assert!(!read.team.on);
    }

    #[test]
    fn the_stop_command_never_ends_with_the_exit_that_holds_a_stop() {
        let command = stop_command(std::path::Path::new("/opt/tenrec/tenrec"), std::path::Path::new("/data"), "abc-123").unwrap();
        assert!(command.contains(" stop-gate --data \"/data\" --sid abc-123") && command.ends_with("|| exit 0"), "{command}");
        assert!(!command.contains("exit 2"));
        assert!(stop_command(std::path::Path::new("/a"), std::path::Path::new("/d"), "a b").is_err());
        assert!(stop_command(std::path::Path::new("/a\"b"), std::path::Path::new("/d"), "x").is_err());
    }
}
