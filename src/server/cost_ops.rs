//! The cost ledger and the budgets, for the pages: `cost_ledger` answers the sums (by day and key) and the budgets; `cost_budget_set`
//! sets the two budgets (a number above 0 and up to 100000, or null for none).
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::{plain, AppState};

/// A budget as asked: null is none; a number must be above 0 and at most 100000 (Err for anything else, and nothing changes).
fn budget(value: &Value) -> Result<Option<f64>, ()> {
    match value {
        Value::Null => Ok(None),
        v => v.as_f64().filter(|x| x.is_finite() && *x > 0.0 && *x <= 100_000.0).map(Some).ok_or(()),
    }
}

pub fn handle(state: &AppState, name: &str, op: &Value, out: &UnboundedSender<String>) {
    match name {
        "cost_ledger" => {
            let settings = state.registry.settings().get();
            let _ = out.send(plain("cost_ledger", None, json!({"days": state.registry.costs().snapshot(), "daily": settings.budget_daily_usd, "monthly": settings.budget_monthly_usd})));
        }
        "cost_budget_set" => {
            let rid = op["rid"].clone();
            let (Ok(daily), Ok(monthly)) = (budget(&op["daily"]), budget(&op["monthly"])) else {
                let _ = out.send(plain("cost_result", None, json!({"ok": false, "rid": rid})));
                return;
            };
            state.registry.settings().update(|s| {
                s.budget_daily_usd = daily;
                s.budget_monthly_usd = monthly;
            });
            let _ = state.registry.events.send(json!({"v": 1, "kind": "cost_changed", "data": {}}));
            let _ = out.send(plain("cost_result", None, json!({"ok": true, "rid": rid})));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_is_none_or_a_number_in_range() {
        assert_eq!(budget(&Value::Null), Ok(None));
        assert_eq!(budget(&json!(12.5)), Ok(Some(12.5)));
        for bad in [json!(0), json!(-1), json!(100001), json!("5"), json!(true), json!([1])] {
            assert_eq!(budget(&bad), Err(()), "{bad}");
        }
    }
}
