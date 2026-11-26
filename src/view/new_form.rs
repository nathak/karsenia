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

/// How long after the form changed fields by itself a click on "start" is held back: the person has not had time to see what was filled in (a
/// press on the button takes the focus from the folder field, which fills the form, and the click follows within a moment).
pub const UNSEEN_MS: u128 = 700;

/// Whether `fill` would change the form: some field it names is not already what `current` holds.
pub fn differs(fill: &Fill, current: &Fill) -> bool {
    let changes = |new: Option<i32>, now: Option<i32>| new.is_some() && new != now;
    changes(fill.model, current.model)
        || changes(fill.account, current.account)
        || changes(fill.permission, current.permission)
        || fill.worktree.is_some() && fill.worktree != current.worktree
}

/// Whether what the form filled in `elapsed_ms` ago may still be unseen.
pub fn unseen(elapsed_ms: u128) -> bool {
    elapsed_ms < UNSEEN_MS
}

/// What goes back to the form's starting values (`start`): a field `before` filled in (for the folder it was settled on earlier) that `now`
/// does not fill in again and the person has not set by hand since, so one project's settings do not stay on a form for another.
pub fn revert(before: &Fill, now: &Fill, touched: Touched, start: &Fill) -> Fill {
    let back = |was: bool, now: bool, hand: bool| was && !now && !hand;
    Fill {
        model: start.model.filter(|_| back(before.model.is_some(), now.model.is_some(), touched.model)),
        account: start.account.filter(|_| back(before.account.is_some(), now.account.is_some(), touched.account)),
        permission: start.permission.filter(|_| back(before.permission.is_some(), now.permission.is_some(), touched.permission)),
        worktree: start.worktree.filter(|_| back(before.worktree.is_some(), now.worktree.is_some(), touched.worktree)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_fill_that_changes_nothing_on_screen_is_not_a_change_and_a_fresh_one_is_unseen() {
        let now = Fill { model: Some(1), account: Some(0), permission: Some(2), worktree: Some(false) };
        assert!(!differs(&Fill { model: Some(1), permission: Some(2), ..Fill::default() }, &now), "the same values again");
        assert!(!differs(&Fill::default(), &now));
        assert!(differs(&Fill { worktree: Some(true), ..Fill::default() }, &now));
        assert!(differs(&Fill { model: Some(3), ..Fill::default() }, &now));
        assert!(unseen(0) && unseen(UNSEEN_MS - 1) && !unseen(UNSEEN_MS), "a click right after is held back, one a moment later is not");
    }

    #[test]
    fn what_was_filled_in_for_one_folder_goes_back_to_the_start_when_the_next_has_nothing_for_it() {
        let start = Fill { model: Some(0), account: Some(1), permission: Some(0), worktree: Some(false) };
        let before = Fill { model: Some(2), account: Some(2), permission: Some(2), worktree: Some(true) };
        let now = Fill { model: Some(3), ..Fill::default() };
        // The model is filled again; the rest goes back to where the form starts.
        assert_eq!(revert(&before, &now, Touched::default(), &start), Fill { model: None, account: Some(1), permission: Some(0), worktree: Some(false) });
        // What the person set by hand is not taken back; what was never filled in is not touched.
        let touched = Touched { permission: true, ..Touched::default() };
        assert_eq!(revert(&before, &Fill::default(), touched, &start).permission, None);
        assert_eq!(revert(&Fill::default(), &Fill::default(), Touched::default(), &start), Fill::default());
    }

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
