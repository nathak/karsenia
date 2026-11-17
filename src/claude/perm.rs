//! The permission mode a chat starts its `claude` with, chosen when the chat is made and fixed for its life. The names are the ones
//! the CLI accepts for `--permission-mode` (Claude Code 2.1.325: `acceptEdits`, `auto`, `bypassPermissions`, `manual` (the same as
//! `default`), `dontAsk`, `plan`); a value is checked against this list before it is ever passed on, and a mode the list does not know
//! is refused, never turned into another one.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermMode {
    /// No argument is passed at all: what a chat made before modes existed does.
    #[default]
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    #[serde(rename = "plan")]
    Plan,
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "dontAsk")]
    DontAsk,
    /// Everything runs without asking (the same as `--dangerously-skip-permissions`; passed as `--permission-mode bypassPermissions`).
    #[serde(rename = "bypassPermissions")]
    Bypass,
}

impl PermMode {
    /// The modes in the order a picker lists them.
    pub const ALL: [PermMode; 6] = [PermMode::Default, PermMode::AcceptEdits, PermMode::Plan, PermMode::Auto, PermMode::DontAsk, PermMode::Bypass];

    /// The CLI's name for the mode.
    pub fn name(self) -> &'static str {
        match self {
            PermMode::Default => "default",
            PermMode::AcceptEdits => "acceptEdits",
            PermMode::Plan => "plan",
            PermMode::Auto => "auto",
            PermMode::DontAsk => "dontAsk",
            PermMode::Bypass => "bypassPermissions",
        }
    }

    /// Exactly one of the six names (no `manual`, no other case, no blank).
    pub fn parse(text: &str) -> Option<PermMode> {
        PermMode::ALL.into_iter().find(|m| m.name() == text)
    }

    pub fn is_default(&self) -> bool {
        *self == PermMode::Default
    }

    /// The value after `--permission-mode`; none for the default, so the arguments of a chat that never chose stay as they were.
    pub fn cli_arg(self) -> Option<&'static str> {
        (!self.is_default()).then(|| self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_six_names_are_accepted_and_each_round_trips() {
        for m in PermMode::ALL {
            assert_eq!(PermMode::parse(m.name()), Some(m));
            assert_eq!(serde_json::to_string(&m).unwrap(), format!("\"{}\"", m.name()), "the stored name is the CLI's");
            assert_eq!(serde_json::from_str::<PermMode>(&format!("\"{}\"", m.name())).unwrap(), m);
        }
        for bad in ["manual", "", "Default", "bypass", "yolo", "plan ", "acceptedits", "--dangerously-skip-permissions"] {
            assert_eq!(PermMode::parse(bad), None, "{bad:?}");
            assert!(serde_json::from_str::<PermMode>(&format!("\"{bad}\"")).is_err(), "a stored name that is not one of them is an error, not another mode");
        }
    }

    #[test]
    fn the_default_adds_no_argument_and_the_others_name_their_mode() {
        assert_eq!(PermMode::Default.cli_arg(), None);
        assert_eq!(PermMode::Bypass.cli_arg(), Some("bypassPermissions"));
        assert_eq!(PermMode::AcceptEdits.cli_arg(), Some("acceptEdits"));
        assert!(PermMode::default().is_default());
    }
}
