//! The items of the feed: what the page is sent, already masked and cut. A browser never gets a text that was not through `mask`.
use serde::Serialize;

/// How much of a prompt (and of what a sub-agent brought back) leaves the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompts {
    /// Up to 2000 characters of a prompt.
    Full,
    /// A short head of a prompt, and no input or result of a call that carries a prompt (a sub-agent's brief and answer).
    Short,
}

impl Prompts {
    pub fn name(self) -> &'static str {
        match self {
            Prompts::Full => "full",
            Prompts::Short => "short",
        }
    }
}

pub const PROMPT_FULL: usize = 2000;
pub const PROMPT_SHORT: usize = 120;
pub const SUMMARY: usize = 120;
pub const IO: usize = 2000;
pub const NOTICE: usize = 500;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    pub sid: String,
    pub seq: u64,
    pub ts: u64,
    pub turn: u32,
    #[serde(flatten)]
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Body {
    Prompt { text: String, truncated: bool },
    Call(Call),
    CallEnd { id: String, status: Status, ms: Option<u64>, output: Option<String> },
    Ask { request_id: String, call: Option<String>, tool: String, by_guard: bool },
    AskEnd { request_id: String, outcome: String },
    TurnEnd(TurnEnd),
    State { state: String },
    Notice { text: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Call {
    pub id: String,
    /// The call that started the sub-agent this one runs in (an exact link: the stream names it).
    pub parent: Option<String>,
    pub tool: String,
    pub summary: String,
    pub input: Option<String>,
    pub category: String,
    pub risk: &'static str,
    /// What the guard says about this call by today's settings (a recomputation, not a record: `derived`).
    pub guard: Option<GuardMark>,
    pub background: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    Ok,
    Error,
    Blocked,
    Declined,
    Started,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GuardMark {
    pub profile: &'static str,
    pub policy: &'static str,
    /// The policy of the project rule that matched, when one did.
    pub rule: Option<&'static str>,
    pub derived: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_create: u64,
    pub output: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Context {
    pub used: u64,
    pub window: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelUse {
    pub model: String,
    pub input: u64,
    pub output: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TurnEnd {
    pub ok: bool,
    pub ms: Option<u64>,
    pub cost_usd: Option<f64>,
    pub usage: Usage,
    pub context: Option<Context>,
    pub models: Vec<ModelUse>,
}

/// One chat as a tab of the page.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tab {
    pub sid: String,
    pub title: String,
    /// The last part of the project folder's path only.
    pub project: String,
    pub state: String,
    pub last_ts: u64,
    pub asks_open: u32,
}
