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
    /// Why Claude Code asks, when a hook (the app's guard) or a rule is the reason (a text; measured with a PreToolUse `ask` hook).
    #[serde(default)]
    pub decision_reason: Value,
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
    /// A request of the program that this app has no answer for (a permission request it cannot show, or another kind such as a hook
    /// callback or an elicitation): the program waits for an answer, so it is refused with `control_error`.
    UnhandledControl { request_id: Option<String>, subtype: String, raw: Value },
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
                        decision_reason: request.get("decision_reason").cloned().unwrap_or(Value::Null),
                        server_policy_ask_only: request.get("server_policy_ask_only").and_then(Value::as_bool) == Some(true),
                        tool_name,
                    });
                }
            }
            Event::UnhandledControl { request_id: text(&value, "request_id"), subtype: text(request, "subtype").unwrap_or_default(), raw: value }
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

/// A file the person attached: its media type as the sender said it (not trusted: the kind is read from the bytes, see
/// `attachments::classify`), its base64 data and, when it has one, its name. Most are images (a thumbnail in the transcript), but any
/// file may be attached.
#[derive(Debug, Clone, Deserialize)]
pub struct Image {
    pub media_type: String,
    pub data: String,
    #[serde(default)]
    pub name: Option<String>,
}

pub const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
/// The largest image the host forwards, in decoded bytes.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// What goes into a message for an attached file, once it has been checked (see `attachments`).
#[derive(Debug, Clone)]
pub enum Part {
    /// An image, as an `image` block.
    Image(Image),
    /// A PDF, as a `document` block.
    Pdf { name: String, data: String },
    /// A text file small enough to be put in the message itself.
    Text { name: String, body: String },
    /// Any other file (or a big one): saved in a folder of this app, and the message names where.
    Stored { name: String, path: std::path::PathBuf, size: usize },
}

fn one_line(value: &Value) -> String {
    value.to_string()
}

/// One user turn: the images first, then the text, then `context` (blocks the model is told besides what the person wrote; the
/// transcript shows only the text).
/// The most lines of a patch or of a created file's content that are kept of a tool result.
pub const MAX_RESULT_LINES: usize = 400;
const MAX_RESULT_CONTENT: usize = 32 * 1024;

/// What a `user` event keeps of its `tool_use_result` (Claude Code's own structured form of a tool's result) before it is logged,
/// indexed and sent on: only what the transcript draws (the patch of an edit, the numbers of a read, the content of a created file),
/// cut to a size. Everything else is dropped, `originalFile` (a whole file, for every edit) above all. A list, not a blacklist, so a
/// big field the CLI adds later is not carried either.
pub fn slim_tool_result(event: &mut Value) {
    if event.get("type").and_then(Value::as_str) != Some("user") {
        return;
    }
    let Some(map) = event.as_object_mut() else { return };
    for key in ["tool_use_result", "toolUseResult"] {
        let Some(result) = map.get_mut(key) else { continue };
        let Some(old) = result.as_object() else {
            // Not a structure (a string, a list): small ones stay, others go.
            if !matches!(result, Value::String(t) if t.len() <= 2048) {
                *result = Value::Null;
            }
            continue;
        };
        let mut keep = serde_json::Map::new();
        for name in ["type", "filePath", "userModified", "replaceAll", "interrupted"] {
            if let Some(v) = old.get(name).filter(|v| v.is_boolean() || matches!(v, Value::String(t) if t.len() <= 2048)) {
                keep.insert(name.into(), v.clone());
            }
        }
        // The counts of the whole result, made before it is cut (the badge and the "N more lines" of a card must not shrink with the cut).
        let counted = |lines: &mut dyn Iterator<Item = &str>| -> (u64, u64, u64) {
            lines.fold((0, 0, 0), |(a, d, n), l| match l.as_bytes().first() {
                Some(b'+') => (a + 1, d, n + 1),
                Some(b'-') => (a, d + 1, n + 1),
                Some(b'\\') => (a, d, n),
                _ => (a, d, n + 1),
            })
        };
        let mut totals = None;
        if let Some(hunks) = old.get("structuredPatch").and_then(Value::as_array).filter(|h| !h.is_empty()) {
            let mut all = hunks.iter().flat_map(|h| h.get("lines").and_then(Value::as_array).into_iter().flatten()).filter_map(Value::as_str);
            totals = Some(counted(&mut all));
        } else if old.get("type").and_then(Value::as_str) == Some("create") {
            if let Some(content) = old.get("content").and_then(Value::as_str) {
                let n = content.lines().count() as u64;
                totals = Some((n, 0, n));
            }
        }
        if let Some((a, d, n)) = totals {
            keep.insert("addCount".into(), a.into());
            keep.insert("delCount".into(), d.into());
            keep.insert("lineCount".into(), n.into());
        }
        if let Some(hunks) = old.get("structuredPatch").and_then(Value::as_array) {
            let mut left = MAX_RESULT_LINES;
            let mut out = Vec::new();
            for hunk in hunks {
                if left == 0 {
                    break;
                }
                let mut h = serde_json::Map::new();
                for n in ["oldStart", "oldLines", "newStart", "newLines"] {
                    if let Some(v) = hunk.get(n).filter(|v| v.is_number()) {
                        h.insert(n.into(), v.clone());
                    }
                }
                let lines: Vec<Value> = hunk.get("lines").and_then(Value::as_array).into_iter().flatten().filter(|l| l.is_string()).take(left).cloned().collect();
                left -= lines.len();
                h.insert("lines".into(), Value::Array(lines));
                out.push(Value::Object(h));
            }
            keep.insert("structuredPatch".into(), Value::Array(out));
        }
        if let Some(file) = old.get("file").and_then(Value::as_object) {
            let mut f = serde_json::Map::new();
            for n in ["filePath", "numLines", "startLine", "totalLines"] {
                if let Some(v) = file.get(n).filter(|v| v.is_number() || matches!(v, Value::String(t) if t.len() <= 2048)) {
                    f.insert(n.into(), v.clone());
                }
            }
            keep.insert("file".into(), Value::Object(f));
        }
        // The content of a file that was created is what the card shows of it.
        if old.get("type").and_then(Value::as_str) == Some("create") {
            if let Some(content) = old.get("content").and_then(Value::as_str) {
                let mut cut: String = content.lines().take(MAX_RESULT_LINES).collect::<Vec<_>>().join("\n");
                if cut.len() > MAX_RESULT_CONTENT {
                    let mut end = MAX_RESULT_CONTENT;
                    while !cut.is_char_boundary(end) {
                        end -= 1;
                    }
                    cut.truncate(end);
                }
                keep.insert("content".into(), Value::String(cut));
            }
        }
        *result = Value::Object(keep);
    }
}

/// `</attached_file` in a file's text, in any letter case, made harmless; the rest of the file stays as it is.
fn neutralise_closing_tag(body: &str) -> String {
    const TAG: &str = "</attached_file";
    let lower = body.to_ascii_lowercase();
    let (mut out, mut from) = (String::with_capacity(body.len()), 0);
    while let Some(at) = lower[from..].find(TAG) {
        out.push_str(&body[from..from + at]);
        out.push_str("&lt;/");
        out.push_str(&body[from + at + 2..from + at + TAG.len()]);
        from += at + TAG.len();
    }
    out.push_str(&body[from..]);
    out
}

pub fn user_message(text: &str, parts: &[Part], context: &[String]) -> String {
    use crate::workbench::inject::esc;
    let mut content: Vec<Value> = Vec::new();
    for part in parts {
        match part {
            Part::Image(image) => content.push(json!({"type": "image", "source": {"type": "base64", "media_type": image.media_type, "data": image.data}})),
            Part::Pdf { name, data } => content.push(json!({"type": "document", "title": name, "source": {"type": "base64", "media_type": "application/pdf", "data": data}})),
            _ => {}
        }
    }
    content.push(json!({"type": "text", "text": text}));
    for part in parts {
        match part {
            Part::Text { name, body } => {
                // Only the closing tag is neutralised: the file's own code stays as it is.
                let body = neutralise_closing_tag(body);
                content.push(json!({"type": "text", "text": format!("<attached_file name=\"{}\">\n{body}\n</attached_file>", esc(name))}));
            }
            Part::Stored { name, path, size } => content.push(json!({"type": "text", "text": format!(
                "<attached_file name=\"{}\" size=\"{size}\" path=\"{}\">The file is saved at this path; read it with the Read tool if it is needed.</attached_file>",
                esc(name),
                esc(&path.display().to_string())
            )})),
            _ => {}
        }
    }
    content.extend(context.iter().map(|block| json!({"type": "text", "text": block})));
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

/// The refusal of a control request of the program that the app does not handle (the program stops waiting for it).
pub fn control_error(request_id: &str, error: &str) -> String {
    one_line(&json!({"type": "control_response", "response": {"subtype": "error", "request_id": request_id, "error": error}}))
}

/// A control request the host sends on stdin (checked live against Claude Code 2.1.x: `set_model`, `set_permission_mode`,
/// `stop_task`, `get_context_usage`); the answer is a `control_response` carrying the same `request_id`.
pub fn control(request_id: &str, request: Value) -> String {
    one_line(&json!({"type": "control_request", "request_id": request_id, "request": request}))
}

pub fn set_model(request_id: &str, model: &str) -> String {
    control(request_id, json!({"subtype": "set_model", "model": model}))
}

pub fn set_permission_mode(request_id: &str, mode: &str) -> String {
    control(request_id, json!({"subtype": "set_permission_mode", "mode": mode}))
}

pub fn stop_task(request_id: &str, task_id: &str) -> String {
    control(request_id, json!({"subtype": "stop_task", "task_id": task_id}))
}

pub fn get_context_usage(request_id: &str) -> String {
    control(request_id, json!({"subtype": "get_context_usage"}))
}

/// What a `control_response` says: its payload on success, its error text otherwise. Both shapes of an error are read
/// (`response.error` as text, or a missing success), since only the success shape was seen live.
pub fn control_outcome(raw: &Value) -> Result<Value, String> {
    let response = &raw["response"];
    if response["subtype"] == "success" {
        return Ok(response.get("response").cloned().unwrap_or(Value::Null));
    }
    let text = response.get("error").and_then(|e| e.as_str().map(str::to_owned).or_else(|| e["message"].as_str().map(str::to_owned)));
    Err(text.unwrap_or_else(|| "the program refused the request".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_blocks_follow_the_text_as_extra_text_parts() {
        let v: Value = serde_json::from_str(&user_message("hi", &[], &["<workbench_contract/>".to_owned()])).unwrap();
        let parts = v["message"]["content"].as_array().unwrap();
        assert_eq!((parts.len(), parts[0]["text"].as_str(), parts[1]["text"].as_str()), (2, Some("hi"), Some("<workbench_contract/>")));
    }

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
    fn control_lines_carry_the_subtype_and_the_answer_is_read_either_way() {
        let v: Value = serde_json::from_str(&set_model("c1", "haiku")).unwrap();
        assert_eq!((v["type"].as_str(), v["request_id"].as_str(), v["request"]["subtype"].as_str(), v["request"]["model"].as_str()), (Some("control_request"), Some("c1"), Some("set_model"), Some("haiku")));
        let v: Value = serde_json::from_str(&set_permission_mode("c2", "plan")).unwrap();
        assert_eq!((v["request"]["subtype"].as_str(), v["request"]["mode"].as_str()), (Some("set_permission_mode"), Some("plan")));
        let v: Value = serde_json::from_str(&stop_task("c3", "t9")).unwrap();
        assert_eq!((v["request"]["subtype"].as_str(), v["request"]["task_id"].as_str()), (Some("stop_task"), Some("t9")));
        assert!(!set_model("c1", "haiku").contains('\n'));
        let ok = json!({"type": "control_response", "response": {"subtype": "success", "request_id": "c1", "response": {"model": "claude-haiku-5-5"}}});
        assert_eq!(control_outcome(&ok).unwrap()["model"], "claude-haiku-5-5");
        let bad = json!({"type": "control_response", "response": {"subtype": "error", "request_id": "c1", "error": "no such model"}});
        assert_eq!(control_outcome(&bad), Err("no such model".into()));
        let odd = json!({"type": "control_response", "response": {"subtype": "weird", "request_id": "c1"}});
        assert!(control_outcome(&odd).is_err());
    }

    #[test]
    fn a_control_request_the_host_does_not_handle_is_named_not_raw() {
        let unhandled = |line: &str| match parse_line(line) {
            Event::UnhandledControl { request_id, subtype, .. } => (request_id, subtype),
            other => panic!("not an unhandled control request: {other:?}"),
        };
        for subtype in ["hook_callback", "elicitation", "mcp_message"] {
            let line = format!(r#"{{"type":"control_request","request_id":"r9","request":{{"subtype":"{subtype}"}}}}"#);
            assert_eq!(unhandled(&line), (Some("r9".to_owned()), subtype.to_owned()));
        }
        // A permission request without a tool is not one the app can show: it is refused like the others.
        assert_eq!(unhandled(r#"{"type":"control_request","request_id":"r","request":{"subtype":"can_use_tool"}}"#), (Some("r".to_owned()), "can_use_tool".to_owned()));
        // Without an id nothing can be answered: the request is still named.
        assert_eq!(unhandled(r#"{"type":"control_request","request":{"subtype":"elicitation"}}"#), (None, "elicitation".to_owned()));
    }

    #[test]
    fn control_error_answers_with_the_same_id() {
        let v: Value = serde_json::from_str(&control_error("r7", "not handled by this app")).unwrap();
        assert_eq!((v["type"].as_str(), v["response"]["subtype"].as_str(), v["response"]["request_id"].as_str(), v["response"]["error"].as_str()), (Some("control_response"), Some("error"), Some("r7"), Some("not handled by this app")));
    }

    #[test]
    fn unknown_broken_and_malformed_lines_never_panic() {
        assert!(matches!(parse_line(r#"{"type":"brand_new","x":1}"#), Event::Raw(_)));
        assert!(matches!(parse_line("not json at all"), Event::Unparsed(_)));
        assert!(matches!(parse_line(""), Event::Unparsed(_)));
        assert!(matches!(parse_line(r#"{"type":"system","subtype":"init"}"#), Event::Raw(_)));
        assert!(matches!(parse_line(r#"{"type":"control_request","request_id":"r","request":{"subtype":"can_use_tool"}}"#), Event::UnhandledControl { .. }));
        assert!(matches!(parse_line("[1,2]"), Event::Raw(_)));
    }

    #[test]
    fn a_tool_result_keeps_what_the_card_draws_and_drops_the_files() {
        let mut e = json!({"type": "user", "message": {"content": []}, "tool_use_result": {
            "filePath": "/p/a.rs", "oldString": "o", "newString": "n", "originalFile": "WHOLE FILE", "userModified": false, "replaceAll": false,
            "gitDiff": {"patch": "big"}, "structuredPatch": [{"oldStart": 3, "oldLines": 1, "newStart": 3, "newLines": 1, "lines": [" a", "-b", "+c"]}]}});
        slim_tool_result(&mut e);
        let r = &e["tool_use_result"];
        assert_eq!(r["filePath"], "/p/a.rs");
        assert_eq!(r["structuredPatch"][0]["lines"][1], "-b");
        assert_eq!(r["structuredPatch"][0]["newStart"], 3);
        for gone in ["originalFile", "oldString", "newString", "gitDiff"] {
            assert!(r.get(gone).is_none(), "{gone}");
        }
        assert!(!e.to_string().contains("WHOLE FILE"));
        // A read keeps its numbers, not its content; a created file keeps (cut) content; a Bash result keeps nothing of its output.
        let mut e = json!({"type": "user", "toolUseResult": {"type": "text", "file": {"filePath": "/p/b", "content": "SECRET BODY", "numLines": 50, "startLine": 10, "totalLines": 99}}});
        slim_tool_result(&mut e);
        assert_eq!(e["toolUseResult"]["file"]["numLines"], 50);
        assert!(!e.to_string().contains("SECRET BODY"));
        let mut e = json!({"type": "user", "tool_use_result": {"type": "create", "filePath": "/p/n", "content": (0..500).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n"), "originalFile": null}});
        slim_tool_result(&mut e);
        assert_eq!(e["tool_use_result"]["content"].as_str().unwrap().lines().count(), MAX_RESULT_LINES);
        let mut e = json!({"type": "user", "tool_use_result": {"stdout": "out", "stderr": "", "interrupted": false}});
        slim_tool_result(&mut e);
        assert!(e["tool_use_result"].get("stdout").is_none() && e["tool_use_result"]["interrupted"] == false);
        // A long patch is cut to the limit across hunks; a long string value goes; other events are not touched.
        let hunk = json!({"oldStart": 1, "oldLines": 300, "newStart": 1, "newLines": 300, "lines": vec!["+x"; 300]});
        let mut e = json!({"type": "user", "tool_use_result": {"structuredPatch": [hunk.clone(), hunk.clone()], "filePath": "x".repeat(5000)}});
        slim_tool_result(&mut e);
        let total: usize = e["tool_use_result"]["structuredPatch"].as_array().unwrap().iter().map(|h| h["lines"].as_array().unwrap().len()).sum();
        assert_eq!(total, MAX_RESULT_LINES);
        // The counts of the whole patch are kept, made before the cut.
        assert_eq!((e["tool_use_result"]["addCount"].as_u64(), e["tool_use_result"]["lineCount"].as_u64()), (Some(600), Some(600)));
        assert!(e["tool_use_result"].get("filePath").is_none());
        let mut other = json!({"type": "assistant", "tool_use_result": {"originalFile": "x"}});
        slim_tool_result(&mut other);
        assert_eq!(other["tool_use_result"]["originalFile"], "x");
    }

    #[test]
    fn outgoing_lines_have_the_shape_the_cli_reads() {
        let image = Image { media_type: "image/png".into(), data: "AAAA".into(), name: None };
        let v: Value = serde_json::from_str(&user_message("hi", &[Part::Image(image)], &[])).unwrap();
        assert_eq!(v["message"]["content"][0]["type"], "image");
        assert_eq!(v["message"]["content"][1]["text"], "hi");
        // Files: a PDF is a document block before the words; a text file and a saved file are text blocks after them, then the context.
        let parts = [
            Part::Pdf { name: "a.pdf".into(), data: "JVBERg==".into() },
            Part::Text { name: "n<1>.txt".into(), body: "x </attached_file> y </ATTACHED_FILE>".into() },
            Part::Stored { name: "z.bin".into(), path: "/d/attachments/s/z.bin".into(), size: 7 },
        ];
        let v: Value = serde_json::from_str(&user_message("see", &parts, &["<ctx/>".into()])).unwrap();
        let c = v["message"]["content"].as_array().unwrap();
        assert_eq!(c[0]["type"], "document");
        assert_eq!(c[0]["source"]["media_type"], "application/pdf");
        assert_eq!(c[1]["text"], "see");
        let text = c[2]["text"].as_str().unwrap();
        assert!(text.starts_with("<attached_file name=\"n&lt;1&gt;.txt\">") && text.contains("x &lt;/attached_file> y &lt;/ATTACHED_FILE>") && text.matches("</attached_file>").count() == 1, "{text}");
        assert!(c[3]["text"].as_str().unwrap().contains("path=\"/d/attachments/s/z.bin\"") && c[3]["text"].as_str().unwrap().contains("size=\"7\""));
        assert_eq!(c[4]["text"], "<ctx/>");
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
