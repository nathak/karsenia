//! The team-mode chip: what a chat has shown of the SmartWorkbench plug-in's team mode. The only evidence is the plug-in's own
//! reply to a `/swb` command that ran in the chat (and, weaker, a held edit): nothing is read from the plug-in's files. A reply
//! counts only when it answers a `/swb` message and the turn made no model call, so a sentence a model wrote, or one that came
//! from a file, never changes the state.

/// What one status reply said: team mode on or off, the model and effort of each role, how many Opus reviews this session had.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TeamLine {
    pub on: bool,
    /// (role, model, effort); only the roles found.
    pub roles: Vec<(String, String, String)>,
    pub opus_reviews: Option<u32>,
    /// The first lines of the reply, cleaned and cut: shown as plain text, never read as markup.
    pub raw: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seen {
    /// The plug-in said so.
    Line,
    /// Team mode held an edit in this chat, so it was on then.
    Guard,
}

/// The last state seen and when (the event's time, so a reopened chat restores it).
#[derive(Debug, Clone, PartialEq)]
pub struct TeamSeen {
    pub line: TeamLine,
    pub ts: u64,
    pub source: Seen,
}

const ROLES: [&str; 3] = ["lead", "explorer", "advisor"];
const PREFIX: &str = "smartworkbench: Team mode ";

/// Whether a message is a command for the plug-in (`/swb ...` or `/smartworkbench ...`).
pub fn is_swb_command(text: &str) -> bool {
    let t = text.trim_start();
    for name in ["/swb", "/smartworkbench"] {
        if let Some(rest) = t.strip_prefix(name) {
            if rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == ':') {
                return true;
            }
        }
    }
    false
}

fn clean_head(text: &str) -> String {
    // Only the front is looked at, cut on a character boundary.
    let front: String = text.chars().take(2000).collect();
    front.chars().filter(|c| *c == '\n' || !c.is_control()).collect::<String>().trim_start().to_owned()
}

pub(super) fn is_model_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '[' | ']')
}

/// `lead sonnet (default)` -> ("sonnet", "default"); the role must start a word.
fn role_in(line: &str, role: &str) -> Option<(String, String)> {
    let needle = format!("{role} ");
    let mut from = 0;
    while let Some(at) = line[from..].find(&needle) {
        let start = from + at;
        let before_ok = start == 0 || line[..start].ends_with([' ', ':', '·', ',']);
        if before_ok {
            let rest = &line[start + needle.len()..];
            let model: String = rest.chars().take_while(|c| is_model_char(*c)).collect();
            if !model.is_empty() && model.chars().count() <= 40 {
                let after = rest[model.len()..].trim_start();
                // The plug-in always names a role's effort in parentheses; without them the words are prose
                // ("the explorer and advisor agents stay available").
                let effort = after
                    .strip_prefix('(')
                    .and_then(|r| r.split_once(')'))
                    .map(|(inside, _)| inside.to_owned())
                    .filter(|e| !e.is_empty() && e.len() <= 16 && e.chars().all(|c| c.is_ascii_lowercase()));
                if let Some(effort) = effort {
                    return Some((model, effort));
                }
            }
        }
        from = start + needle.len();
    }
    None
}

/// The SmartWorkbench commands the Team page can send, by the key a button carries: only these documented, fixed texts (README of the
/// plug-in, "Commands"). Goals, pins, constraints and done conditions have no command form and are not here; `guard off` is left out
/// (a click should not switch the guard off).
pub fn swb_command(key: &str) -> Option<&'static str> {
    Some(match key {
        "new" => "/swb new",
        "lock" => "/swb lock",
        "unlock" => "/swb unlock",
        "suggest" => "/swb suggest",
        "suggest_add" => "/swb suggest add",
        "suggest_undo" => "/swb suggest undo",
        "status" => "/swb status",
        "preview" => "/swb preview",
        "guard_high" => "/swb guard high",
        "guard_medium" => "/swb guard medium",
        "guard_low" => "/swb guard low",
        _ => return None,
    })
}

/// The command that changes one role's model (and effort), as the plug-in documents it: `/swb team <lead|explorer|advisor> <model> [effort]`.
/// The model is an alias or a full model id of 1 to 40 plain characters; the effort is one of Claude Code's levels (`low`, `medium`, `high`,
/// `xhigh`, `max`) or, for the lead only, `default`; blank leaves it out. The plug-in decides what a model that refuses an effort does.
pub fn role_command(role: &str, model: &str, effort: Option<&str>) -> Result<String, &'static str> {
    if !ROLES.contains(&role) {
        return Err("unknown role");
    }
    let model = model.trim();
    if model.is_empty() || model.chars().count() > 40 || !model.chars().all(is_model_char) {
        return Err("not a model name");
    }
    let effort = effort.map(str::trim).filter(|e| !e.is_empty());
    match effort {
        None => Ok(format!("/swb team {role} {model}")),
        Some(e) if ["low", "medium", "high", "xhigh", "max"].contains(&e) || (e == "default" && role == "lead") => Ok(format!("/swb team {role} {model} {e}")),
        Some(_) => Err("not an effort level"),
    }
}

/// The plug-in's reply to `/swb team ...`: `smartworkbench: Team mode on|off` at the very start, then (in the status form and
/// the "on" form) the roles, and in the status form the number of Opus reviews. None for anything else.
pub fn parse_team_line(text: &str) -> Option<TeamLine> {
    let t = clean_head(text);
    let rest = t.strip_prefix(PREFIX)?;
    let word: String = rest.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let on = match word.as_str() {
        "on" => true,
        "off" => false,
        _ => return None,
    };
    if !rest[word.len()..].starts_with([':', '.', ' ', '\n']) && !rest[word.len()..].is_empty() {
        return None;
    }
    let first = t.lines().next().unwrap_or_default();
    let roles = ROLES.iter().filter_map(|role| role_in(first, role).map(|(model, effort)| ((*role).to_owned(), model, effort))).collect();
    let opus_reviews = t.find("Opus reviews this session: ").and_then(|at| {
        let digits: String = t[at + "Opus reviews this session: ".len()..].chars().take_while(char::is_ascii_digit).take(6).collect();
        digits.parse().ok()
    });
    let raw = t.lines().take(3).map(|l| l.chars().take(200).collect::<String>()).collect::<Vec<_>>().join("\n");
    Some(TeamLine { on, roles, opus_reviews, raw: raw.chars().take(600).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: &str = "smartworkbench: Team mode on: lead sonnet (default) builds, explorer haiku (low) explores, advisor opus (high) reviews.\nThe session model is set (team off puts the previous model back).";
    const OFF: &str = "smartworkbench: Team mode off: no automatic Opus reviews; the model is back to sonnet.";
    const STATUS_ON: &str = "smartworkbench: Team mode on: lead sonnet (default) · explorer haiku (low) · advisor opus (high). Opus reviews this session: 3.\n/swb team on|off · /swb team <lead|explorer|advisor> <model> [effort]";

    fn roles(l: &TeamLine) -> Vec<String> {
        l.roles.iter().map(|(r, m, e)| format!("{r}={m}/{e}")).collect()
    }

    #[test]
    fn the_three_replies_the_plugin_gives_are_read() {
        let on = parse_team_line(ON).expect("on");
        assert!(on.on && on.opus_reviews.is_none());
        assert_eq!(roles(&on), vec!["lead=sonnet/default", "explorer=haiku/low", "advisor=opus/high"]);
        let off = parse_team_line(OFF).expect("off");
        assert!(!off.on && off.roles.is_empty());
        let status = parse_team_line(STATUS_ON).expect("status");
        assert!(status.on && status.opus_reviews == Some(3));
        assert_eq!(roles(&status), roles(&on));
        let off_status = parse_team_line(&STATUS_ON.replace("Team mode on", "Team mode off")).expect("status off");
        assert!(!off_status.on && off_status.roles.len() == 3);
    }

    #[test]
    fn spaces_and_control_characters_do_not_hide_it_and_nothing_else_counts() {
        assert!(parse_team_line(&format!("  \r\x1b{OFF}")).is_some());
        for bad in ["Team mode maybe", "xsmartworkbench: Team mode on", "hello smartworkbench: Team mode on", "smartworkbench: Team mode onward", "smartworkbench: Team mode ", ""] {
            assert!(parse_team_line(bad).is_none(), "{bad}");
        }
        // A word that only contains a role is not that role.
        let l = parse_team_line("smartworkbench: Team mode on: misleading x (high) · advisor opus (high)").expect("line");
        assert_eq!(roles(&l), vec!["advisor=opus/high"]);
        // The plug-in's other "off" reply is prose that names two roles without a model: no roles.
        let prose = parse_team_line("smartworkbench: Team mode off: no automatic Opus reviews; the explorer and advisor agents stay available.").expect("off");
        assert!(!prose.on && prose.roles.is_empty());
        // A role change answers with something else and is not a team state.
        assert!(parse_team_line("smartworkbench: lead: opus (default), applied when team mode is turned on.").is_none());
        // A model name too long to be one is left out.
        let long = format!("smartworkbench: Team mode on: lead {} (high)", "m".repeat(41));
        assert!(parse_team_line(&long).expect("line").roles.is_empty());
    }

    #[test]
    fn a_huge_reply_is_cut_and_markup_stays_text() {
        let big = format!("smartworkbench: Team mode on: <b>**x**</b>\n{}", "y".repeat(1_000_000));
        let l = parse_team_line(&big).expect("big");
        assert!(l.raw.chars().count() <= 600);
        assert!(l.raw.contains("<b>**x**</b>"), "{}", l.raw);
    }

    #[test]
    fn which_messages_are_plugin_commands() {
        for yes in ["/swb", "/swb team on", "  /swb team", "/smartworkbench:x", "/swb:team"] {
            assert!(is_swb_command(yes), "{yes}");
        }
        for no in ["/swbx", "hello /swb", "swb team", "/swab", ""] {
            assert!(!is_swb_command(no), "{no}");
        }
    }
    #[test]
    fn a_role_command_is_the_documented_one_and_only_for_plain_names() {
        assert_eq!(role_command("advisor", "opus", Some("max")).unwrap(), "/swb team advisor opus max");
        assert_eq!(role_command("explorer", "sonnet", Some("low")).unwrap(), "/swb team explorer sonnet low");
        assert_eq!(role_command("lead", "opus[1m]", Some("high")).unwrap(), "/swb team lead opus[1m] high");
        assert_eq!(role_command("lead", " fable ", None).unwrap(), "/swb team lead fable");
        assert_eq!(role_command("lead", "sonnet", Some("")).unwrap(), "/swb team lead sonnet");
        assert_eq!(role_command("lead", "sonnet", Some("default")).unwrap(), "/swb team lead sonnet default");
        assert!(role_command("advisor", "opus", Some("default")).is_err(), "default is the lead's alone");
        for bad in ["", "a b", "x;y", "opus\nmax", &"m".repeat(41), "/swb", "a:b"] {
            assert!(role_command("lead", bad, None).is_err(), "{bad:?}");
        }
        for role in ["", "Lead", "team", "lead opus"] {
            assert!(role_command(role, "opus", None).is_err(), "{role:?}");
        }
        assert!(role_command("lead", "opus", Some("ultra")).is_err() && role_command("lead", "opus", Some("high x")).is_err());
    }

    #[test]
    fn only_the_documented_fixed_commands_can_be_sent_from_a_button() {
        for key in ["new", "lock", "unlock", "suggest", "suggest_add", "suggest_undo", "status", "preview", "guard_high", "guard_medium", "guard_low"] {
            assert!(swb_command(key).is_some_and(|c| c.starts_with("/swb ") && is_swb_command(c)), "{key}");
        }
        for bad in ["", "guard_off", "off", "goal", "pin", "clear", "/swb clear", "new; x", "guard"] {
            assert!(swb_command(bad).is_none(), "{bad}");
        }
    }
}
