//! The feed of one chat, made from its events. It keeps what it must to join a result to its call and an answer to its question, does no
//! I/O, and gives the same items for the same events (a replay and a live follow agree). Every text that goes into an item is masked here.
use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::feed::{Body, Call, Context, GuardMark, Item, ModelUse, Prompts, Status, TurnEnd, Usage, IO, NOTICE, PROMPT_FULL, PROMPT_SHORT, SUMMARY};
use super::mask::{mask, mask_text};
use crate::session::Envelope;
use crate::watch::{Alert, Watch};
use crate::workbench::risk::{classify, summarize, Category, Risk};

/// What the guard would say about a call (the server gives the chat's own settings; a test gives a fixed answer).
pub type GuardFn = Box<dyn Fn(&str, &Value) -> Option<GuardMark> + Send>;

/// Tools whose input and result are prompts to a sub-agent: left out when prompts are cut short.
const PROMPT_TOOLS: [&str; 3] = ["Agent", "Task", "Workflow"];
const GUARD_PREFIX: &str = "SmartWorkbench guard";

struct Open {
    ts: u64,
    background: bool,
    /// Its input and result are not shown (a secret file, or a prompt in the short mode).
    hide: bool,
}

pub struct Deriver {
    prompts: Prompts,
    guard: Option<GuardFn>,
    turn: u32,
    calls: HashMap<String, Open>,
    asks: HashMap<String, Option<String>>,
    declined: HashSet<String>,
    ctx_used: Option<u64>,
    watch: Watch,
}

impl Deriver {
    pub fn new(prompts: Prompts, guard: Option<GuardFn>) -> Self {
        Self { prompts, guard, turn: 0, calls: HashMap::new(), asks: HashMap::new(), declined: HashSet::new(), ctx_used: None, watch: Watch::new() }
    }

    /// The questions that are still open (asked and not yet answered).
    pub fn asks_open(&self) -> u32 {
        self.asks.len() as u32
    }

    pub fn push(&mut self, e: &Envelope) -> Vec<Item> {
        let bodies = match e.kind.as_str() {
            "user" => self.user(&e.data),
            "claude" => self.claude(&e.data, e.ts),
            "permission" => self.permission(&e.data),
            "permission_resolved" => self.resolved(&e.data),
            "state" => e.data["state"].as_str().map(|s| vec![Body::State { state: s.to_owned() }]).unwrap_or_default(),
            "error" => vec![Body::Notice { text: mask_text(e.data["message"].as_str().unwrap_or_default(), NOTICE) }],
            "stderr" => {
                let text = e.data["text"].as_str().unwrap_or_default().trim();
                let tail: String = text.chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect();
                vec![Body::Notice { text: mask_text(&tail, NOTICE) }]
            }
            _ => Vec::new(),
        };
        bodies.into_iter().map(|body| Item { sid: e.sid.clone(), seq: e.seq, ts: e.ts, turn: self.turn, body }).collect()
    }

    fn user(&mut self, d: &Value) -> Vec<Body> {
        self.turn += 1;
        let limit = if self.prompts == Prompts::Full { PROMPT_FULL } else { PROMPT_SHORT };
        let (text, truncated) = mask(d["text"].as_str().unwrap_or_default(), limit);
        vec![Body::Prompt { text, truncated }]
    }

    fn claude(&mut self, d: &Value, ts: u64) -> Vec<Body> {
        match d["type"].as_str().unwrap_or_default() {
            "assistant" => self.assistant(d, ts),
            "user" => self.tool_results(d, ts),
            "result" => self.result(d),
            _ => Vec::new(),
        }
    }

    fn assistant(&mut self, d: &Value, ts: u64) -> Vec<Body> {
        let parent = d["parent_tool_use_id"].as_str().filter(|p| !p.is_empty()).map(str::to_owned);
        if parent.is_none() {
            // The last call's prompt plus its answer is how much of the window the chat takes now.
            if let Some(usage) = d.pointer("/message/usage") {
                let n: u64 = ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens", "output_tokens"].iter().map(|k| usage[*k].as_u64().unwrap_or(0)).sum();
                if n > 0 {
                    self.ctx_used = Some(n);
                }
            }
        }
        let mut out = Vec::new();
        for block in d.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
            if block["type"] != "tool_use" {
                continue;
            }
            let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) else { continue };
            let input = block.get("input").cloned().unwrap_or(Value::Null);
            let verdict = classify(name, &input);
            let secret = matches!(verdict.category, Category::SecretFile | Category::SecretOutput);
            let hide = secret || (self.prompts == Prompts::Short && PROMPT_TOOLS.contains(&name));
            let shown = if hide { None } else { Some(mask_text(&serde_json::to_string_pretty(&input).unwrap_or_default(), IO)) };
            let background = input["run_in_background"].as_bool() == Some(true) || name == "Workflow";
            self.calls.insert(id.to_owned(), Open { ts, background, hide });
            out.push(Body::Call(Call {
                id: id.to_owned(),
                parent: parent.clone(),
                tool: name.chars().take(80).collect(),
                summary: mask_text(&summarize(name, &input), SUMMARY),
                input: shown,
                category: verdict.category.name().to_owned(),
                risk: risk_name(verdict.risk),
                guard: self.guard.as_ref().and_then(|g| g(name, &input)),
                background,
            }));
            // Said right after the call that made it worth saying: the same on a replay as when following.
            for alert in self.watch.call(id, name, &input, parent.is_some()) {
                out.push(match alert {
                    Alert::Loop { tool, times } => Body::Alert { kind: "loop", tool: Some(tool), n: times },
                    Alert::Scope { files } => Body::Alert { kind: "scope", tool: None, n: files as u32 },
                });
            }
        }
        out
    }

    fn tool_results(&mut self, d: &Value, ts: u64) -> Vec<Body> {
        let mut out = Vec::new();
        for block in d.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
            if block["type"] != "tool_result" {
                continue;
            }
            let Some(id) = block["tool_use_id"].as_str() else { continue };
            let text = result_text(&block["content"]);
            let is_error = block["is_error"].as_bool() == Some(true);
            let open = self.calls.remove(id);
            let status = if is_error {
                let kind = crate::view::transcript::guard_kind(&text);
                if text.trim_start().starts_with(GUARD_PREFIX) || matches!(kind, Some("team" | "vhdb")) {
                    Status::Blocked
                } else if self.declined.remove(id) || kind == Some("swb_ask") {
                    Status::Declined
                } else {
                    Status::Error
                }
            } else if open.as_ref().is_some_and(|o| o.background) {
                Status::Started
            } else {
                Status::Ok
            };
            let ms = open.as_ref().filter(|o| o.ts > 0 && ts >= o.ts).map(|o| ts - o.ts);
            let hide = open.as_ref().is_some_and(|o| o.hide);
            out.push(Body::CallEnd { id: id.to_owned(), status, ms, output: (!hide).then(|| mask_text(&text, IO)) });
        }
        out
    }

    fn result(&mut self, d: &Value) -> Vec<Body> {
        // A command the plug-in answered itself called no model: nothing to count.
        if d["num_turns"].as_u64() == Some(0) {
            return Vec::new();
        }
        let n = |v: &Value, k: &str| v[k].as_u64().unwrap_or(0);
        let usage = Usage { input: n(&d["usage"], "input_tokens"), cache_read: n(&d["usage"], "cache_read_input_tokens"), cache_create: n(&d["usage"], "cache_creation_input_tokens"), output: n(&d["usage"], "output_tokens") };
        let by_model = d["modelUsage"].as_object();
        let window = by_model.and_then(|m| m.values().filter_map(|u| u["contextWindow"].as_u64()).max());
        let models = by_model.map(|m| m.iter().map(|(name, u)| ModelUse { model: name.chars().take(80).collect(), input: n(u, "inputTokens"), output: n(u, "outputTokens") }).collect()).unwrap_or_default();
        vec![Body::TurnEnd(TurnEnd {
            ok: d["subtype"] == "success" && d["is_error"].as_bool() != Some(true),
            ms: d["duration_ms"].as_u64(),
            cost_usd: d["total_cost_usd"].as_f64(),
            usage,
            context: self.ctx_used.zip(window).map(|(used, window)| Context { used, window }),
            models,
        })]
    }

    fn permission(&mut self, d: &Value) -> Vec<Body> {
        let Some(id) = d["request_id"].as_str() else { return Vec::new() };
        let call = d["tool_use_id"].as_str().map(str::to_owned);
        self.asks.insert(id.to_owned(), call.clone());
        let by_guard = d["decision_reason"].to_string().contains(GUARD_PREFIX);
        vec![Body::Ask { request_id: id.to_owned(), call, tool: d["tool_name"].as_str().unwrap_or_default().chars().take(80).collect(), by_guard }]
    }

    fn resolved(&mut self, d: &Value) -> Vec<Body> {
        let Some(id) = d["request_id"].as_str() else { return Vec::new() };
        let outcome = d["outcome"].as_str().filter(|o| ["allowed", "denied", "cancelled"].contains(o)).unwrap_or("cancelled");
        if let Some(Some(call)) = self.asks.remove(id) {
            if outcome == "denied" {
                self.declined.insert(call);
            }
        }
        vec![Body::AskEnd { request_id: id.to_owned(), outcome: outcome.to_owned() }]
    }
}

fn risk_name(risk: Risk) -> &'static str {
    match risk {
        Risk::Low => "low",
        Risk::Medium => "medium",
        Risk::High => "high",
    }
}

/// A tool result's content as text: a string, or the text blocks of a list.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(list) => list.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn env(seq: u64, ts: u64, kind: &str, data: Value) -> Envelope {
        Envelope { v: 1, sid: "s1".into(), seq, ts, kind: kind.into(), data }
    }

    fn assistant(blocks: Value, parent: Option<&str>) -> Value {
        json!({"type": "assistant", "parent_tool_use_id": parent, "message": {"content": blocks, "usage": {"input_tokens": 10, "cache_read_input_tokens": 90, "output_tokens": 5}}})
    }

    fn tool_use(id: &str, name: &str, input: Value) -> Value {
        json!([{"type": "tool_use", "id": id, "name": name, "input": input}])
    }

    fn tool_result(id: &str, text: &str, is_error: bool) -> Value {
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text, "is_error": is_error}]}})
    }

    fn run(d: &mut Deriver, events: Vec<Envelope>) -> Vec<Item> {
        events.iter().flat_map(|e| d.push(e)).collect()
    }

    fn bodies(items: &[Item]) -> Vec<&Body> {
        items.iter().map(|i| &i.body).collect()
    }

    #[test]
    fn a_prompt_a_call_and_its_result_with_the_time_it_took() {
        let mut d = Deriver::new(Prompts::Full, None);
        let items = run(&mut d, vec![
            env(1, 1000, "user", json!({"text": "list files"})),
            env(2, 1100, "claude", assistant(tool_use("t1", "Bash", json!({"command": "ls -la"})), None)),
            env(3, 1350, "claude", tool_result("t1", "a\nb", false)),
        ]);
        assert!(matches!(&items[0].body, Body::Prompt { text, truncated: false } if text == "list files"));
        let Body::Call(call) = &items[1].body else { panic!("{:?}", items[1]) };
        assert_eq!((call.id.as_str(), call.tool.as_str(), call.summary.as_str(), call.category.as_str(), call.risk), ("t1", "Bash", "ls -la", "shell", "low"));
        assert!(call.input.as_deref().is_some_and(|i| i.contains("ls -la")) && !call.background && call.parent.is_none());
        assert!(matches!(&items[2].body, Body::CallEnd { id, status: Status::Ok, ms: Some(250), output: Some(o) } if id == "t1" && o == "a\nb"), "{:?}", items[2]);
        assert!(items.iter().all(|i| i.turn == 1 && i.sid == "s1"));
    }

    #[test]
    fn an_error_a_guard_block_and_a_declined_question_are_told_apart() {
        let mut d = Deriver::new(Prompts::Full, None);
        let items = run(&mut d, vec![
            env(1, 10, "user", json!({"text": "go"})),
            env(2, 11, "claude", assistant(json!([{"type": "tool_use", "id": "a", "name": "Bash", "input": {"command": "false"}}, {"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": "x"}}, {"type": "tool_use", "id": "c", "name": "Edit", "input": {"file_path": "a.rs"}}]), None)),
            env(3, 12, "claude", tool_result("a", "exit 1", true)),
            env(4, 13, "claude", tool_result("b", "SmartWorkbench guard (destructive-git): a call the guard looks at", true)),
            env(5, 14, "permission", json!({"request_id": "r1", "tool_name": "Edit", "tool_use_id": "c", "decision_reason": "SmartWorkbench guard (workspace-edit): high"})),
            env(6, 15, "permission_resolved", json!({"request_id": "r1", "outcome": "denied"})),
            env(7, 16, "claude", tool_result("c", "The user declined this.", true)),
        ]);
        let ends: Vec<Status> = items.iter().filter_map(|i| if let Body::CallEnd { status, .. } = &i.body { Some(*status) } else { None }).collect();
        assert_eq!(ends, [Status::Error, Status::Blocked, Status::Declined]);
        assert!(items.iter().any(|i| matches!(&i.body, Body::Ask { request_id, call: Some(c), by_guard: true, .. } if request_id == "r1" && c == "c")));
        assert!(items.iter().any(|i| matches!(&i.body, Body::AskEnd { outcome, .. } if outcome == "denied")));
        assert_eq!(d.asks_open(), 0);
    }

    #[test]
    fn a_team_mode_hold_is_a_block_and_a_question_that_could_not_be_asked_is_a_decline() {
        let mut d = Deriver::new(Prompts::Full, None);
        let items = run(&mut d, vec![
            env(1, 1, "claude", assistant(json!([{"type": "tool_use", "id": "a", "name": "Edit", "input": {"file_path": "x"}}, {"type": "tool_use", "id": "b", "name": "Edit", "input": {"file_path": "y"}}]), None)),
            env(2, 2, "claude", tool_result("a", "smartworkbench: Team mode is on: use the advisor first", true)),
            env(3, 3, "claude", tool_result("b", "The user declined this. in SmartWorkbench", true)),
        ]);
        let ends: Vec<Status> = items.iter().filter_map(|i| if let Body::CallEnd { status, .. } = &i.body { Some(*status) } else { None }).collect();
        assert_eq!(ends, [Status::Error, Status::Declined], "the plug-in's hold text is 'SmartWorkbench team mode:' exactly; this one is not it");
        let mut d = Deriver::new(Prompts::Full, None);
        let items = run(&mut d, vec![env(1, 1, "claude", assistant(tool_use("a", "Edit", json!({"file_path": "x"})), None)), env(2, 2, "claude", tool_result("a", "SmartWorkbench team mode: design first", true))]);
        assert!(matches!(&items[1].body, Body::CallEnd { status: Status::Blocked, .. }), "{:?}", items[1]);
    }

    #[test]
    fn an_open_question_is_counted_and_a_cancelled_one_is_not_a_decline() {
        let mut d = Deriver::new(Prompts::Full, None);
        run(&mut d, vec![env(1, 1, "claude", assistant(tool_use("c", "Edit", json!({"file_path": "a"})), None)), env(2, 2, "permission", json!({"request_id": "r", "tool_name": "Edit", "tool_use_id": "c"}))]);
        assert_eq!(d.asks_open(), 1);
        let items = run(&mut d, vec![env(3, 3, "permission_resolved", json!({"request_id": "r", "outcome": "cancelled"})), env(4, 4, "claude", tool_result("c", "interrupted", true))]);
        assert!(matches!(&items[1].body, Body::CallEnd { status: Status::Error, .. }), "{:?}", items[1]);
        assert_eq!(d.asks_open(), 0);
    }

    #[test]
    fn a_sub_agent_call_names_its_parent_and_a_background_call_is_started_not_ok() {
        let mut d = Deriver::new(Prompts::Full, None);
        let items = run(&mut d, vec![
            env(1, 1, "claude", assistant(tool_use("p", "Agent", json!({"description": "look", "prompt": "find it"})), None)),
            env(2, 2, "claude", assistant(tool_use("k", "Read", json!({"file_path": "a.rs"})), Some("p"))),
            env(3, 3, "claude", assistant(tool_use("bg", "Bash", json!({"command": "sleep 9", "run_in_background": true})), None)),
            env(4, 4, "claude", tool_result("bg", "started", false)),
        ]);
        let Body::Call(child) = &items[1].body else { panic!() };
        assert_eq!(child.parent.as_deref(), Some("p"));
        let Body::Call(bg) = &items[2].body else { panic!() };
        assert!(bg.background);
        assert!(matches!(&items[3].body, Body::CallEnd { status: Status::Started, .. }));
    }

    #[test]
    fn the_end_of_a_turn_carries_tokens_cost_context_and_models_and_a_plugin_answer_carries_none() {
        let mut d = Deriver::new(Prompts::Full, None);
        let items = run(&mut d, vec![
            env(1, 1, "claude", assistant(json!([]), None)),
            env(2, 2, "claude", json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 4200, "total_cost_usd": 0.0123, "usage": {"input_tokens": 12, "cache_read_input_tokens": 800, "cache_creation_input_tokens": 30, "output_tokens": 55}, "modelUsage": {"claude-sonnet-4-5": {"inputTokens": 12, "outputTokens": 55, "contextWindow": 200000}}})),
            env(3, 3, "claude", json!({"type": "result", "subtype": "success", "num_turns": 0, "result": "ok"})),
        ]);
        assert_eq!(items.len(), 1, "the zero-turn result gives nothing: {items:?}");
        let Body::TurnEnd(t) = &items[0].body else { panic!() };
        assert!(t.ok && t.ms == Some(4200) && t.cost_usd == Some(0.0123));
        assert_eq!(t.usage, Usage { input: 12, cache_read: 800, cache_create: 30, output: 55 });
        assert_eq!(t.context, Some(Context { used: 105, window: 200000 }));
        assert_eq!(t.models, [ModelUse { model: "claude-sonnet-4-5".into(), input: 12, output: 55 }]);
    }

    #[test]
    fn a_call_repeated_five_times_is_alerted_once_after_the_fifth_and_a_replay_says_the_same() {
        let events = || {
            (0..7)
                .map(|i| env(i + 1, 100 + i, "claude", assistant(tool_use(&format!("t{i}"), "Bash", json!({"command": "cargo test"})), None)))
                .collect::<Vec<_>>()
        };
        let mut live = Deriver::new(Prompts::Full, None);
        let mut items = Vec::new();
        for e in events() {
            items.extend(live.push(&e));
        }
        let alerts: Vec<usize> = items.iter().enumerate().filter(|(_, i)| matches!(i.body, Body::Alert { .. })).map(|(at, _)| at).collect();
        // Seven calls and one alert, which stands right after the fifth call (the sixth item, as every call is one item before it).
        assert_eq!(alerts, [5], "{items:?}");
        assert!(matches!(&items[5].body, Body::Alert { kind: "loop", tool: Some(t), n: 5 } if t == "Bash"));
        assert!(matches!(&items[4].body, Body::Call(c) if c.id == "t4"));
        let mut replay = Deriver::new(Prompts::Full, None);
        let again: Vec<Item> = events().iter().flat_map(|e| replay.push(e)).collect();
        assert_eq!(items, again);
    }

    #[test]
    fn one_message_written_as_several_events_is_not_counted_several_times() {
        // Claude Code writes one line per content block of a message and repeats the message's usage on each: the size of the context
        // is the last call's own number (it replaces), and the cost is the result's single figure, so nothing adds up per line.
        let mut d = Deriver::new(Prompts::Full, None);
        let items = run(&mut d, vec![
            env(1, 1, "claude", assistant(json!([]), None)),
            env(2, 2, "claude", assistant(json!([]), None)),
            env(3, 3, "claude", assistant(json!([]), None)),
            env(4, 4, "claude", json!({"type": "result", "subtype": "success", "is_error": false, "total_cost_usd": 0.5, "modelUsage": {"claude-sonnet-4-5": {"contextWindow": 200000}}})),
        ]);
        let Body::TurnEnd(t) = &items[0].body else { panic!("{items:?}") };
        assert_eq!(t.context, Some(Context { used: 105, window: 200000 }), "not 315");
        assert_eq!(t.cost_usd, Some(0.5));
    }

    #[test]
    fn secrets_are_masked_a_secret_file_shows_no_input_or_result_and_the_short_mode_hides_prompts() {
        let key = format!("sk-ant-{}", "z9y8x7w6v5".repeat(6));
        let mut d = Deriver::new(Prompts::Full, None);
        let items = run(&mut d, vec![
            env(1, 1, "user", json!({"text": format!("my key is {key}")})),
            env(2, 2, "claude", assistant(tool_use("e", "Read", json!({"file_path": ".env"})), None)),
            env(3, 3, "claude", tool_result("e", "API_KEY=supersecretvalue123", false)),
            env(4, 4, "claude", assistant(tool_use("b", "Bash", json!({"command": format!("curl -H 'Authorization: Bearer {key}' x")})), None)),
        ]);
        let all = serde_json::to_string(&items).unwrap();
        assert!(!all.contains("z9y8x7w6v5") && !all.contains("supersecretvalue123"), "{all}");
        let Body::Call(env_call) = &items[1].body else { panic!() };
        assert!(matches!(env_call.category.as_str(), "secret-file" | "secret-output") && env_call.input.is_none(), "{env_call:?}");
        assert!(matches!(&items[2].body, Body::CallEnd { output: None, .. }));

        let mut short = Deriver::new(Prompts::Short, None);
        let items = run(&mut short, vec![
            env(1, 1, "user", json!({"text": "x".repeat(500)})),
            env(2, 2, "claude", assistant(tool_use("p", "Agent", json!({"prompt": "the brief"})), None)),
            env(3, 3, "claude", tool_result("p", "what it found", false)),
        ]);
        assert!(matches!(&items[0].body, Body::Prompt { text, truncated: true } if text.chars().count() == 120));
        let Body::Call(c) = &items[1].body else { panic!() };
        assert!(c.input.is_none());
        assert!(matches!(&items[2].body, Body::CallEnd { output: None, .. }));
    }

    #[test]
    fn the_guard_mark_is_what_the_server_gave_and_the_same_events_give_the_same_items() {
        let guard: GuardFn = Box::new(|tool, _| (tool == "Bash").then_some(GuardMark { profile: "balanced", policy: "ask", rule: None, derived: true }));
        let events = || vec![env(1, 1, "user", json!({"text": "go"})), env(2, 2, "claude", assistant(tool_use("t", "Bash", json!({"command": "git push"})), None)), env(3, 3, "claude", tool_result("t", "ok", false)), env(4, 4, "state", json!({"state": "idle"})), env(5, 5, "stderr", json!({"text": "warn"}))];
        let mut a = Deriver::new(Prompts::Full, Some(guard));
        let first = run(&mut a, events());
        let Body::Call(c) = &first[1].body else { panic!() };
        assert_eq!(c.guard, Some(GuardMark { profile: "balanced", policy: "ask", rule: None, derived: true }));
        assert!(matches!(&first[3].body, Body::State { state } if state == "idle") && matches!(&first[4].body, Body::Notice { .. }));
        let mut b = Deriver::new(Prompts::Full, Some(Box::new(|tool, _| (tool == "Bash").then_some(GuardMark { profile: "balanced", policy: "ask", rule: None, derived: true }))));
        assert_eq!(bodies(&first), bodies(&run(&mut b, events())));
    }

    #[test]
    fn events_that_are_not_in_the_expected_shape_give_nothing_and_do_not_panic() {
        let mut d = Deriver::new(Prompts::Full, None);
        let items = run(&mut d, vec![
            env(1, 1, "claude", json!({"type": "assistant"})),
            env(2, 2, "claude", json!("text")),
            env(3, 3, "claude", assistant(json!([{"type": "tool_use"}, {"type": "tool_use", "id": "x"}, 5]), None)),
            env(4, 4, "claude", tool_result("never-opened", "x", false)),
            env(5, 5, "permission", json!({})),
            env(6, 6, "permission_resolved", json!({"request_id": "nope", "outcome": "weird"})),
            env(7, 7, "memory_used", json!({"ids": [1]})),
        ]);
        assert_eq!(items.len(), 2, "{items:?}");
        assert!(matches!(&items[1].body, Body::AskEnd { outcome, .. } if outcome == "cancelled"));
    }
}
