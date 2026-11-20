//! The app's own team mode: a chat can have two helper subagents (an explorer that reads and searches, an advisor that designs and
//! checks) with a model and an effort of their own, and the lead's effort. The settings are kept per chat in the workbench file and
//! handed to `claude` as `--agents` (the two definitions) and `--effort` (the lead); the delegation rule is in the agents'
//! descriptions, so a message carries only one short line (`GUIDANCE`).
use serde::{Deserialize, Serialize};

/// How hard a model thinks (Claude Code's `--effort` levels; which ones a model accepts depends on the model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub const ALL: [Effort; 5] = [Self::Low, Self::Medium, Self::High, Self::Xhigh, Self::Max];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == word)
    }
}

/// The two helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sub {
    Explorer,
    Advisor,
}

impl Sub {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explorer => "explorer",
            Self::Advisor => "advisor",
        }
    }
}

/// One helper's model (None: the lead's) and effort (None: the model's own).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RoleSet {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<Effort>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    #[serde(default)]
    pub on: bool,
    #[serde(default)]
    pub lead_effort: Option<Effort>,
    #[serde(default = "explorer_default")]
    pub explorer: RoleSet,
    #[serde(default = "advisor_default")]
    pub advisor: RoleSet,
}

fn explorer_default() -> RoleSet {
    RoleSet { model: Some("haiku".into()), effort: None }
}

fn advisor_default() -> RoleSet {
    RoleSet { model: Some("opus".into()), effort: None }
}

impl Default for Team {
    fn default() -> Self {
        Self { on: false, lead_effort: None, explorer: explorer_default(), advisor: advisor_default() }
    }
}

impl Team {
    pub fn role(&self, which: Sub) -> &RoleSet {
        match which {
            Sub::Explorer => &self.explorer,
            Sub::Advisor => &self.advisor,
        }
    }

    pub fn role_mut(&mut self, which: Sub) -> &mut RoleSet {
        match which {
            Sub::Explorer => &mut self.explorer,
            Sub::Advisor => &mut self.advisor,
        }
    }
}

/// The models a helper can be given by name (the rest of the accepted ones are full ids).
pub const ROLE_ALIASES: [&str; 6] = ["sonnet", "opus", "haiku", "fable", "sonnet[1m]", "opus[1m]"];

/// A helper's model as typed: an alias or a full model id; anything else (spaces, quotes, a path) is refused.
pub fn role_model_ok(name: &str) -> bool {
    ROLE_ALIASES.contains(&name)
        || (name.len() <= 64
            && name.starts_with("claude-")
            && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-' || c == '_'))
}

/// The one line a message carries while team mode is on.
pub const GUIDANCE: &str = "<workbench_team>Team mode: delegate exploration to the explorer subagent; ask the advisor for a DESIGN before non-trivial edits and for a final check before you say you are done.</workbench_team>";

/// The `--agents` value for a team: the explorer (read-only: search, read, fetch) and the advisor (read-only: no web, no edits). A
/// helper without a model takes the lead's (`inherit`). The effort is the helper's own field, left out when not chosen.
pub fn agents_json(team: &Team) -> String {
    let def = |which: Sub, description: &str, prompt: &str, tools: &[&str]| {
        let set = team.role(which);
        let mut d = serde_json::json!({
            "description": description,
            "prompt": prompt,
            "tools": tools,
            "model": set.model.as_deref().unwrap_or("inherit"),
        });
        if let Some(e) = set.effort {
            d["effort"] = e.as_str().into();
        }
        d
    };
    serde_json::json!({
        "explorer": def(
            Sub::Explorer,
            "Use proactively for any search or reading across the project before editing. Fast and read-only; answers with paths, line numbers and facts.",
            "You explore the project and the docs for the lead. You never edit. Answer briefly with file paths, line numbers and the facts asked for.",
            &["Read", "Grep", "Glob", "WebFetch", "WebSearch"],
        ),
        "advisor": def(
            Sub::Advisor,
            "Use before any non-trivial edit to get a DESIGN (approach, files, steps, risks, checks) and before saying the work is done to get a final check (go / go with changes / stop, blockers with path:line). Read-only.",
            "You are a senior designer and reviewer for the lead. You never edit. For a design give the approach, the files, the steps, the risks and how to check. For a final check say go, go with changes or stop, and name each blocker with path:line.",
            &["Read", "Grep", "Glob"],
        ),
    })
    .to_string()
}

/// A line for the transcript when the team changes, e.g. `on · lead high · explorer haiku · advisor opus/high`.
pub fn summary(team: &Team) -> String {
    if !team.on {
        return "off".into();
    }
    let part = |name: &str, set: &RoleSet| format!("{name} {}{}", set.model.as_deref().unwrap_or("lead"), set.effort.map(|e| format!("/{}", e.as_str())).unwrap_or_default());
    let lead = team.lead_effort.map(|e| format!("lead {} · ", e.as_str())).unwrap_or_default();
    format!("on · {lead}{} · {}", part("explorer", &team.explorer), part("advisor", &team.advisor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_are_aliases_or_plain_ids() {
        for ok in ["haiku", "opus[1m]", "claude-opus-5-5", "claude-sonnet-4.5_x"] {
            assert!(role_model_ok(ok), "{ok}");
        }
        for bad in ["", "gpt-4", "claude opus", "claude-\"x", "claude-../x", "Opus", "default"] {
            assert!(!role_model_ok(bad), "{bad}");
        }
    }

    #[test]
    fn the_agents_have_the_chosen_models_and_no_way_to_edit() {
        let mut t = Team::default();
        t.advisor = RoleSet { model: Some("claude-opus-5-5".into()), effort: Some(Effort::High) };
        t.explorer.model = None;
        let v: serde_json::Value = serde_json::from_str(&agents_json(&t)).unwrap();
        assert_eq!(v["explorer"]["model"], "inherit");
        assert!(v["explorer"].get("effort").is_none());
        assert_eq!(v["advisor"]["model"], "claude-opus-5-5");
        assert_eq!(v["advisor"]["effort"], "high");
        for who in ["explorer", "advisor"] {
            let tools = v[who]["tools"].as_array().unwrap();
            for banned in ["Edit", "Write", "Bash", "NotebookEdit"] {
                assert!(!tools.iter().any(|t| t == banned), "{who} {banned}");
            }
        }
        assert!(!v["advisor"]["tools"].as_array().unwrap().iter().any(|t| t == "WebFetch"));
    }

    #[test]
    fn an_old_file_without_a_team_reads_as_the_default_and_the_summary_names_it() {
        let t: Team = serde_json::from_str("{}").unwrap();
        assert_eq!(t, Team::default());
        assert!(!t.on);
        let mut t = t;
        t.on = true;
        t.lead_effort = Some(Effort::Max);
        t.advisor.effort = Some(Effort::High);
        assert_eq!(summary(&t), "on · lead max · explorer haiku · advisor opus/high");
        assert_eq!(Effort::parse("xhigh"), Some(Effort::Xhigh));
        assert_eq!(Effort::parse("none"), None);
    }
}
