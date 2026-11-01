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
pub fn allow(request_id: &str, input: &Value) -> String {
    one_line(&json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": {"behavior": "allow", "updatedInput": input}}}))
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
        let v: Value = serde_json::from_str(&allow("r1", &json!({"command": "ls"}))).unwrap();
        assert_eq!(v["response"]["response"]["behavior"], "allow");
        assert_eq!(v["response"]["response"]["updatedInput"]["command"], "ls");
        let v: Value = serde_json::from_str(&deny("r1", "no")).unwrap();
        assert_eq!(v["response"]["response"]["message"], "no");
        let v: Value = serde_json::from_str(&interrupt("c1")).unwrap();
        assert_eq!(v["request"]["subtype"], "interrupt");
        assert!(!allow("r", &json!({})).contains('\n'));
    }
}
