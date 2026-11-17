//! The lines `claude -p --input-format stream-json --output-format stream-json` speaks.
//!
//! Only what the host must act on is modelled strictly (init, a permission request, the end of a turn, a reply to a
//! control request); every other event is kept as raw JSON and drawn by the UI, so a new CLI version cannot break parsing.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A permission request from `--permission-prompt-tool stdio`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanUseTool {
    pub request_id: String,
    pub tool_name: String,
    pub display_name: Option<String>,
    pub input: Value,
    pub description: Option<String>,
    pub permission_suggestions: Value,
    pub blocked_path: Option<String>,
    pub tool_use_id: Option<String>,
    /// A field Claude Code 2.1.325 sends on some requests (seen as `true` on a file write) and does not document: kept as it is, and a request
    /// that carries it is not offered a rule to remember (what it promises is not known).
    #[serde(default)]
    pub server_policy_ask_only: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Init { session_id: String, cwd: Option<String>, model: Option<String>, raw: Value },
    CanUseTool(CanUseTool),
    /// A reply to a control request the host sent (an interrupt, for one).
    ControlResponse { request_id: String, raw: Value },
    /// The end of a turn.
    Result { subtype: String, is_error: bool, total_cost_usd: Option<f64>, duration_ms: Option<u64>, raw: Value },
    /// Anything else that parsed as JSON.
    Raw(Value),
    /// A line that was not JSON.
    Unparsed(String),
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// Branches on the field values, not on how the line starts (a `result` line does not begin with `type`). Never panics.
pub fn parse_line(line: &str) -> Event {
    let line = line.trim();
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Event::Unparsed(line.to_owned());
    };
    let kind = value.get("type").and_then(Value::as_str).unwrap_or_default();
    let sub = value.get("subtype").and_then(Value::as_str).unwrap_or_default();
    match (kind, sub) {
        ("system", "init") => match text(&value, "session_id") {
            Some(session_id) => Event::Init { session_id, cwd: text(&value, "cwd"), model: text(&value, "model"), raw: value },
            None => Event::Raw(value),
        },
        ("control_request", _) => {
            let request = &value["request"];
            if request.get("subtype").and_then(Value::as_str) == Some("can_use_tool") {
                if let (Some(request_id), Some(tool_name)) = (text(&value, "request_id"), text(request, "tool_name")) {
                    return Event::CanUseTool(CanUseTool {
                        request_id,
                        display_name: text(request, "display_name"),
                        input: request.get("input").cloned().unwrap_or(Value::Null),
                        description: text(request, "description"),
                        permission_suggestions: request.get("permission_suggestions").cloned().unwrap_or(Value::Null),
                        blocked_path: text(request, "blocked_path"),
                        tool_use_id: text(request, "tool_use_id"),
                        server_policy_ask_only: request.get("server_policy_ask_only").and_then(Value::as_bool) == Some(true),
                        tool_name,
                    });
                }
            }
            Event::Raw(value)
        }
        ("control_response", _) => match value["response"].get("request_id").and_then(Value::as_str) {
            Some(request_id) => Event::ControlResponse { request_id: request_id.to_owned(), raw: value },
            None => Event::Raw(value),
        },
        ("result", _) => Event::Result {
            subtype: sub.to_owned(),
            is_error: value.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            total_cost_usd: value.get("total_cost_usd").and_then(Value::as_f64),
            duration_ms: value.get("duration_ms").and_then(Value::as_u64),
            raw: value,
        },
        _ => Event::Raw(value),
    }
}

/// An image the person attached: the media type and its base64 data.
#[derive(Debug, Clone, Deserialize)]
pub struct Image {
    pub media_type: String,
    pub data: String,
}

pub const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
/// The largest image the host forwards, in decoded bytes.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

fn one_line(value: &Value) -> String {
    value.to_string()
}

/// One user turn: the images first, then the text.
pub fn user_message(text: &str, images: &[Image]) -> String {
    let mut content: Vec<Value> = images
        .iter()
        .map(|image| json!({"type": "image", "source": {"type": "base64", "media_type": image.media_type, "data": image.data}}))
        .collect();
    content.push(json!({"type": "text", "text": text}));
    one_line(&json!({"type": "user", "message": {"role": "user", "content": content}}))
}

/// The answer to a permission request. The input is the one the request carried, never one the browser sent.
pub fn allow(request_id: &str, input: &Value, updated_permissions: Option<Value>) -> String {
    let mut response = json!({"behavior": "allow", "updatedInput": input});
    if let Some(rules) = updated_permissions {
        response["updatedPermissions"] = rules;
    }
    one_line(&json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": response}}))
}

/// The rules a person may have Claude Code remember for a request, worked out from the request's own `permission_suggestions` and from
/// nothing else. A suggestion is taken only when it is `addRules` with behavior `allow` and 1 to 8 rules that all name the tool that
/// is asked about and carry a plain, non-empty `ruleContent` of at most 500 characters; what is sent back is built from those checked
/// fields (every other key is dropped), with the destination the person chose. `addDirectories`, `setMode` and any other suggestion
/// are never passed on.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Grants {
    /// `updatedPermissions` for "allow for this run of the process" (destination `session`).
    pub session: Option<Value>,
    /// `updatedPermissions` for "always allow in this project" (destination `localSettings`, a file Claude Code writes).
    pub always: Option<Value>,
    /// The rules as text, `Bash(touch probe-file.txt)`, one per line.
    pub rule_text: Option<String>,
    /// True when no rule has a `*`: only exactly what was asked is allowed.
    pub exact: bool,
}

pub fn grants(suggestions: &Value, tool_name: &str) -> Grants {
    let Some(list) = suggestions.as_array() else { return Grants::default() };
    for suggestion in list {
        if suggestion["type"] != "addRules" || suggestion["behavior"] != "allow" {
            continue;
        }
        let Some(rules) = suggestion["rules"].as_array().filter(|r| (1..=8).contains(&r.len())) else { continue };
        let mut clean: Vec<Value> = Vec::new();
        let mut shown: Vec<String> = Vec::new();
        for rule in rules {
            let (Some(name), Some(content)) = (rule["toolName"].as_str(), rule["ruleContent"].as_str()) else { break };
            if name != tool_name || content.is_empty() || content.chars().count() > 500 || content.chars().any(char::is_control) {
                break;
            }
            clean.push(json!({"toolName": name, "ruleContent": content}));
            shown.push(format!("{name}({content})"));
        }
        if clean.len() != rules.len() {
            continue;
        }
        let build = |destination: &str| json!([{"type": "addRules", "rules": clean, "behavior": "allow", "destination": destination}]);
        let exact = clean.iter().all(|r| !r["ruleContent"].as_str().unwrap_or_default().contains('*'));
        return Grants { session: Some(build("session")), always: Some(build("localSettings")), rule_text: Some(shown.join("\n")), exact };
    }
    Grants::default()
}

pub fn deny(request_id: &str, message: &str) -> String {
    one_line(&json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": {"behavior": "deny", "message": message}}}))
}

pub fn interrupt(request_id: &str) -> String {
    one_line(&json!({"type": "control_request", "request_id": request_id, "request": {"subtype": "interrupt"}}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_read_by_field_values() {
        let line = r#"{"type":"system","subtype":"init","session_id":"s1","cwd":"/p","model":"haiku","tools":[]}"#;
        assert!(matches!(parse_line(line), Event::Init { ref session_id, .. } if session_id == "s1"));
    }

    #[test]
    fn a_permission_request_keeps_its_input_and_suggestions() {
        let line = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"touch a"},"permission_suggestions":[{"type":"addRules"}],"blocked_path":"/x"}}"#;
        let Event::CanUseTool(ask) = parse_line(line) else { panic!("not a permission request") };
        assert_eq!(ask.tool_name, "Bash");
        assert_eq!(ask.input["command"], "touch a");
        assert_eq!(ask.blocked_path.as_deref(), Some("/x"));
        assert!(ask.permission_suggestions.is_array());
    }

    #[test]
    fn a_result_line_that_does_not_start_with_type_is_still_a_result() {
        let line = r#"{"subtype":"success","is_error":false,"total_cost_usd":0.5,"duration_ms":12,"type":"result","result":"ok"}"#;
        assert!(matches!(parse_line(line), Event::Result { is_error: false, duration_ms: Some(12), .. }));
        let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#;
        assert!(matches!(parse_line(line), Event::Result { is_error: true, .. }));
    }

    #[test]
    fn a_control_response_names_its_request() {
        let line = r#"{"type":"control_response","response":{"subtype":"success","request_id":"c1","response":{"still_queued":[]}}}"#;
        assert!(matches!(parse_line(line), Event::ControlResponse { ref request_id, .. } if request_id == "c1"));
    }

    #[test]
    fn unknown_broken_and_malformed_lines_never_panic() {
        assert!(matches!(parse_line(r#"{"type":"brand_new","x":1}"#), Event::Raw(_)));
        assert!(matches!(parse_line("not json at all"), Event::Unparsed(_)));
        assert!(matches!(parse_line(""), Event::Unparsed(_)));
        assert!(matches!(parse_line(r#"{"type":"system","subtype":"init"}"#), Event::Raw(_)));
        assert!(matches!(parse_line(r#"{"type":"control_request","request_id":"r","request":{"subtype":"can_use_tool"}}"#), Event::Raw(_)));
        assert!(matches!(parse_line("[1,2]"), Event::Raw(_)));
    }

    #[test]
    fn outgoing_lines_have_the_shape_the_cli_reads() {
        let image = Image { media_type: "image/png".into(), data: "AAAA".into() };
        let v: Value = serde_json::from_str(&user_message("hi", &[image])).unwrap();
        assert_eq!(v["message"]["content"][0]["type"], "image");
        assert_eq!(v["message"]["content"][1]["text"], "hi");
        let v: Value = serde_json::from_str(&allow("r1", &json!({"command": "ls"}), None)).unwrap();
        assert!(v["response"]["response"].get("updatedPermissions").is_none());
        let rules = json!([{"type": "addRules"}]);
        let v2: Value = serde_json::from_str(&allow("r1", &json!({}), Some(rules.clone()))).unwrap();
        assert_eq!(v2["response"]["response"]["updatedPermissions"], rules);
        assert_eq!(v["response"]["response"]["behavior"], "allow");
        assert_eq!(v["response"]["response"]["updatedInput"]["command"], "ls");
        let v: Value = serde_json::from_str(&deny("r1", "no")).unwrap();
        assert_eq!(v["response"]["response"]["message"], "no");
        let v: Value = serde_json::from_str(&interrupt("c1")).unwrap();
        assert_eq!(v["request"]["subtype"], "interrupt");
        assert!(!allow("r", &json!({}), None).contains('\n'));
    }
    // What Claude Code 2.1.325 really sent for `touch probe-file.txt` (measured).
    const BASH_SUGGESTIONS: &str = r#"[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"touch probe-file.txt"}],"behavior":"allow","destination":"localSettings"},{"type":"addDirectories","directories":["/p"],"destination":"session"},{"type":"setMode","mode":"acceptEdits","destination":"session"}]"#;
    const WRITE_SUGGESTIONS: &str = r#"[{"type":"setMode","mode":"acceptEdits","destination":"session"}]"#;

    #[test]
    fn a_rule_to_remember_is_built_from_the_requests_own_suggestion_and_only_for_the_tool_asked_about() {
        let g = grants(&serde_json::from_str(BASH_SUGGESTIONS).unwrap(), "Bash");
        assert_eq!(g.rule_text.as_deref(), Some("Bash(touch probe-file.txt)"));
        assert!(g.exact);
        assert_eq!(g.session, Some(json!([{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "touch probe-file.txt"}], "behavior": "allow", "destination": "session"}])));
        assert_eq!(g.always.as_ref().unwrap()[0]["destination"], "localSettings");
        // Nothing else of the suggestions travels: no directory, no mode.
        let sent = format!("{:?}", g);
        assert!(!sent.contains("addDirectories") && !sent.contains("setMode") && !sent.contains("acceptEdits"));
        // A file write was only offered a mode change: nothing to remember.
        assert_eq!(grants(&serde_json::from_str(WRITE_SUGGESTIONS).unwrap(), "Write"), Grants::default());
        // The rule must be for the tool that asks.
        assert_eq!(grants(&serde_json::from_str(BASH_SUGGESTIONS).unwrap(), "Edit"), Grants::default());
    }

    #[test]
    fn a_suggestion_that_is_not_plain_is_not_taken() {
        let one = |rule: Value, behavior: &str| json!([{"type": "addRules", "rules": [rule], "behavior": behavior, "destination": "localSettings"}]);
        let bash = |c: Value| json!({"toolName": "Bash", "ruleContent": c});
        assert!(grants(&one(bash(json!("ls")), "allow"), "Bash").session.is_some());
        for bad in [one(bash(json!("ls")), "deny"), one(bash(json!("")), "allow"), one(bash(json!("a\nb")), "allow"), one(bash(json!("x".repeat(501))), "allow"), one(bash(json!(3)), "allow"), one(json!({"toolName": "Bash"}), "allow"), one(json!({"toolName": "Read", "ruleContent": "x"}), "allow"), json!([{"type": "addRules", "rules": [], "behavior": "allow"}]), json!("nope"), Value::Null] {
            assert_eq!(grants(&bad, "Bash"), Grants::default(), "{bad}");
        }
        // A rule that has a wildcard is taken as Claude suggested it, and said not to be exact.
        let wild = grants(&one(bash(json!("npm run *")), "allow"), "Bash");
        assert!(wild.session.is_some() && !wild.exact);
        // More rules than eight, or one bad rule among good ones: none.
        let many = json!([{"type": "addRules", "behavior": "allow", "rules": (0..9).map(|i| bash(json!(format!("c{i}")))).collect::<Vec<_>>()}]);
        assert_eq!(grants(&many, "Bash"), Grants::default());
        let mixed = json!([{"type": "addRules", "behavior": "allow", "rules": [bash(json!("ls")), json!({"toolName": "Bash", "ruleContent": ""})]}]);
        assert_eq!(grants(&mixed, "Bash"), Grants::default());
        // Extra keys of the suggestion and of a rule are dropped.
        let extra = json!([{"type": "addRules", "behavior": "allow", "destination": "userSettings", "x": 1, "rules": [{"toolName": "Bash", "ruleContent": "ls", "y": 2}]}]);
        let g = grants(&extra, "Bash");
        assert_eq!(g.always.unwrap(), json!([{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "ls"}], "behavior": "allow", "destination": "localSettings"}]), "the destination is the person's choice, not the suggestion's");
    }

    #[test]
    fn the_undocumented_ask_only_flag_is_read_and_defaults_to_false() {
        let line = |extra: &str| format!(r#"{{"type":"control_request","request_id":"r","request":{{"subtype":"can_use_tool","tool_name":"Write","input":{{}}{extra}}}}}"#);
        let Event::CanUseTool(a) = parse_line(&line(r#","server_policy_ask_only":true"#)) else { panic!() };
        assert!(a.server_policy_ask_only);
        let Event::CanUseTool(b) = parse_line(&line("")) else { panic!() };
        assert!(!b.server_policy_ask_only);
    }
}
