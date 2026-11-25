//! What the desktop new-chat form takes from what the core remembers for a folder (the `project_defaults` answer, see
//! `project_defaults::form_json`): which fields it fills in and with what, never a field the person already touched in this form, never a
//! late answer for another folder, never the mode that skips every check.
use crate::claude::perm::PermMode;
use serde_json::Value;

/// The model choices of the form, in the order of its box (the first is the program's own default).
pub const MODELS: [&str; 4] = ["default", "haiku", "sonnet", "opus"];

/// The fields the person has set by hand in this form: they stay as they are.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Touched {
    pub model: bool,
    pub account: bool,
    pub permission: bool,
    pub worktree: bool,
}

/// What to put into the form; None is a field to leave alone.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Fill {
    pub model: Option<i32>,
    pub account: Option<i32>,
    /// An index into `PermMode::ALL`, never the one that skips every check.
    pub permission: Option<i32>,
    pub worktree: Option<bool>,
}

impl Fill {
    /// Whether anything is filled in (the form then says so).
    pub fn any(&self) -> bool {
        *self != Fill::default()
    }
}

/// `answer` is what the core said for `asked` (the folder the form holds now); `account_ids` are the ids behind the form's account list (None:
/// the default account).
pub fn prefill(answer: &Value, asked: &str, touched: Touched, account_ids: &[Option<String>]) -> Fill {
    if answer["found"] != true || answer["cwd"].as_str() != Some(asked) {
        return Fill::default();
    }
    let mut fill = Fill::default();
    if !touched.model {
        fill.model = match &answer["model"] {
            Value::Null => Some(0),
            Value::String(name) => MODELS.iter().position(|m| m == name).map(|i| i as i32),
            _ => None,
        };
    }
    if !touched.account {
        // A missing key is an account that is not known any more: the form keeps its own.
        fill.account = match answer.get("account") {
            Some(Value::Null) => account_ids.iter().position(Option::is_none).map(|i| i as i32),
            Some(Value::String(id)) => account_ids.iter().position(|one| one.as_deref() == Some(id.as_str())).map(|i| i as i32),
            _ => None,
        };
    }
    if !touched.permission {
        fill.permission = answer["permission"].as_str().and_then(PermMode::parse).filter(|m| *m != PermMode::Bypass).and_then(|m| PermMode::ALL.iter().position(|one| *one == m)).map(|i| i as i32);
    }
    if !touched.worktree {
        fill.worktree = answer["worktree"].as_bool();
    }
    fill
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ids() -> Vec<Option<String>> {
        vec![None, Some("a1".into()), Some("a2".into())]
    }

    fn answer() -> Value {
        json!({"cwd": "/p", "found": true, "model": "haiku", "worktree": true, "account": "a2", "permission": "plan"})
    }

    #[test]
    fn everything_remembered_is_filled_in_as_the_forms_own_indexes() {
        let fill = prefill(&answer(), "/p", Touched::default(), &ids());
        assert_eq!(fill, Fill { model: Some(1), account: Some(2), permission: Some(2), worktree: Some(true) });
        assert!(fill.any());
    }

    #[test]
    fn a_field_the_person_touched_is_left_alone() {
        let touched = Touched { model: true, account: true, permission: true, worktree: true };
        assert_eq!(prefill(&answer(), "/p", touched, &ids()), Fill::default());
        let only_model = prefill(&answer(), "/p", Touched { model: true, ..Default::default() }, &ids());
        assert_eq!((only_model.model, only_model.permission), (None, Some(2)));
    }

    #[test]
    fn an_answer_for_another_folder_or_for_none_is_dropped() {
        assert_eq!(prefill(&answer(), "/other", Touched::default(), &ids()), Fill::default(), "a late answer");
        assert_eq!(prefill(&json!({"cwd": "/p", "found": false}), "/p", Touched::default(), &ids()), Fill::default());
        assert!(!Fill::default().any());
    }

    #[test]
    fn the_mode_that_skips_every_check_is_never_filled_in_even_if_the_answer_names_it() {
        let a = json!({"cwd": "/p", "found": true, "model": null, "worktree": false, "permission": "bypassPermissions"});
        assert_eq!(prefill(&a, "/p", Touched::default(), &ids()).permission, None);
        for unknown in [json!("yolo"), json!(7), Value::Null] {
            let a = json!({"cwd": "/p", "found": true, "model": null, "worktree": false, "permission": unknown});
            assert_eq!(prefill(&a, "/p", Touched::default(), &ids()).permission, None);
        }
    }

    #[test]
    fn an_account_or_model_the_form_does_not_have_is_left_alone() {
        let a = json!({"cwd": "/p", "found": true, "model": "sonnet[1m]", "worktree": false, "account": "gone"});
        let fill = prefill(&a, "/p", Touched::default(), &ids());
        assert_eq!((fill.model, fill.account), (None, None));
        let no_account_key = json!({"cwd": "/p", "found": true, "model": null, "worktree": false});
        assert_eq!(prefill(&no_account_key, "/p", Touched::default(), &ids()).account, None, "no key: the account is not known any more");
    }

    #[test]
    fn the_default_account_and_the_default_model_are_the_first_entries() {
        let a = json!({"cwd": "/p", "found": true, "model": null, "worktree": false, "account": null});
        let fill = prefill(&a, "/p", Touched::default(), &ids());
        assert_eq!((fill.model, fill.account, fill.worktree), (Some(0), Some(0), Some(false)));
        assert_eq!(prefill(&a, "/p", Touched::default(), &[]).account, None, "before the account list came");
    }
}
