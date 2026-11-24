//! A stand-in for `claude` that replays a scenario, so tests run without a model.
//!
//! FAKE_SCENARIO: a file of lines. A line is printed to stdout as it is, unless it is a directive:
//!   {"$wait":"user"|"control_response"|"interrupt"}  read stdin until such a line arrives
//!   {"$reply":"<subtype>","response":{..}}           read stdin until a control_request of that subtype arrives, then answer it with a
//!                                                    success carrying `response` (or, with "error":"text", an error); `$wait` on
//!                                                    "control_request" waits for any control request without answering
//!   {"$sleep":ms}                                    wait
//!   {"$exit":code}                                   end the process
//! FAKE_ENV_LOG: one JSON line per start, `{"argv": [...], "env": {...}}`, with the variables a credential could ride in (test values only).
//! FAKE_AUTH: a file holding what `auth status` prints (`$DIR` stands for CLAUDE_CONFIG_DIR); `auth logout` logs `AUTH_LOGOUT`.
//! FAKE_LOG: every argument line (`ARGV ...`) and every stdin line (`IN ...`) is appended here as it arrives.
use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::Duration;

fn log(path: &Option<String>, text: &str) {
    if let Some(path) = path {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{text}");
        }
    }
}

fn log_env(path: &Option<String>) {
    let Some(path) = path else { return };
    let env: serde_json::Map<String, serde_json::Value> = std::env::vars()
        .filter(|(k, _)| ["ANTHROPIC_", "CLAUDE_", "AWS_BEARER", "TENREC_MCP_", "SWB_MCP_"].iter().any(|p| k.starts_with(p)))
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect();
    let line = serde_json::json!({"argv": std::env::args().skip(1).collect::<Vec<_>>(), "env": env, "cwd": std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default()});
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

fn args_has(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn matches(line: &str, wanted: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return false };
    let kind = v["type"].as_str().unwrap_or_default();
    match wanted {
        "user" => kind == "user",
        "control_response" => kind == "control_response",
        "interrupt" => kind == "control_request" && v["request"]["subtype"] == "interrupt",
        "control_request" => kind == "control_request",
        _ => false,
    }
}

fn main() {
    let log_path = std::env::var("FAKE_LOG").ok();
    log(&log_path, &format!("ARGV {}", std::env::args().skip(1).collect::<Vec<_>>().join(" ")));
    log_env(&std::env::var("FAKE_ENV_LOG").ok());

    // `claude auth status|logout`: status prints FAKE_AUTH's text (`$DIR` is the config folder this run was given) or a logged-in answer.
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("auth") {
        let dir = std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_default();
        match argv.get(1).map(String::as_str) {
            Some("status") => {
                let default = r#"{"loggedIn":true,"email":"fake@example.com","configDirectory":"$DIR"}"#.to_owned();
                let text = std::env::var("FAKE_AUTH").ok().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or(default);
                println!("{}", text.trim().replace("$DIR", &dir));
            }
            Some("logout") => log(&log_path, "AUTH_LOGOUT"),
            _ => {}
        }
        return;
    }
    // `claude -p /usage`: answered with no model, from FAKE_USAGE (a file with the text) or a sample.
    if argv.iter().any(|a| a == "/usage") {
        log(&log_path, "USAGE_ASKED");
        let default = "Current session: 100% used · resets 6:39pm (Asia/Seoul)\nCurrent week (all models): 81% used · resets Nov 24, 4:59pm (Asia/Seoul)";
        let text = std::env::var("FAKE_USAGE").ok().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_else(|| default.to_owned());
        println!("{}", serde_json::json!({"type": "result", "subtype": "success", "is_error": false, "num_turns": 0, "total_cost_usd": 0, "result": text}));
        return;
    }
    // A scheduled run (--max-turns with --output-format json): the prompt on stdin, one JSON result out.
    //   FAKE_SCHED: a file holding the result line to print; FAKE_SCHED_SLEEP_MS: wait first; FAKE_SCHED_EXIT: exit with this code instead.
    if args_has(&std::env::args().collect::<Vec<_>>(), "--max-turns") {
        let mut prompt = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut prompt);
        log(&log_path, &format!("SCHED_IN {}", prompt.replace('\n', "\\n")));
        if let Some(ms) = std::env::var("FAKE_SCHED_SLEEP_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
            std::thread::sleep(Duration::from_millis(ms));
        }
        if let Some(code) = std::env::var("FAKE_SCHED_EXIT").ok().and_then(|v| v.parse::<i32>().ok()) {
            std::process::exit(code);
        }
        let default = r#"{"type":"result","subtype":"success","is_error":false,"result":"fake report","total_cost_usd":0.01,"num_turns":2,"permission_denials":[]}"#;
        let answer = std::env::var("FAKE_SCHED").ok().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_else(|| default.to_owned());
        println!("{}", answer.trim());
        return;
    }
    // The auto-save extractor asks for one JSON answer (--output-format json) and writes its prompt on stdin, whole.
    let args: Vec<String> = std::env::args().collect();
    if args.windows(2).any(|w| w[0] == "--output-format" && w[1] == "json") {
        let mut prompt = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut prompt);
        // A skill proposal asks for a schema with a "body"; auto-save asks for memories.
        let propose = args.windows(2).any(|w| w[0] == "--json-schema" && w[1].contains("\"body\""));
        let (tag, var, default) = if propose {
            ("PROPOSE_IN", "FAKE_PROPOSE", r#"{"type":"result","subtype":"success","is_error":false,"result":"{}","structured_output":{"skip":true,"reason":"nothing"}}"#)
        } else {
            ("EXTRACT_IN", "FAKE_EXTRACT", r#"{"type":"result","subtype":"success","is_error":false,"result":"{}","structured_output":{"memories":[]}}"#)
        };
        log(&log_path, &format!("{tag} {}", prompt.replace('\n', "\\n")));
        let answer = std::env::var(var).ok().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_else(|| default.to_owned());
        println!("{}", answer.trim());
        return;
    }
    let scenario = std::env::var("FAKE_SCENARIO").ok().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();

    let (tx, rx) = mpsc::channel::<String>();
    let reader_log = log_path.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            log(&reader_log, &format!("IN {line}"));
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let stdout = std::io::stdout();
    for line in scenario.lines().filter(|l| !l.trim().is_empty()) {
        let directive = serde_json::from_str::<serde_json::Value>(line).ok();
        if let Some(wanted) = directive.as_ref().and_then(|d| d["$wait"].as_str()) {
            loop {
                match rx.recv_timeout(Duration::from_secs(20)) {
                    Ok(got) if matches(&got, wanted) => break,
                    Ok(_) => continue,
                    Err(_) => std::process::exit(9),
                }
            }
        } else if let Some(subtype) = directive.as_ref().and_then(|d| d["$reply"].as_str()) {
            let d = directive.as_ref().unwrap();
            loop {
                match rx.recv_timeout(Duration::from_secs(20)) {
                    Ok(got) => {
                        let Ok(v) = serde_json::from_str::<serde_json::Value>(&got) else { continue };
                        if v["type"] != "control_request" || v["request"]["subtype"] != subtype {
                            continue;
                        }
                        let id = v["request_id"].as_str().unwrap_or_default();
                        let response = match d["error"].as_str() {
                            Some(error) => serde_json::json!({"subtype": "error", "request_id": id, "error": error}),
                            None => serde_json::json!({"subtype": "success", "request_id": id, "response": d["response"].clone()}),
                        };
                        let mut out = stdout.lock();
                        let _ = writeln!(out, "{}", serde_json::json!({"type": "control_response", "response": response}));
                        let _ = out.flush();
                        break;
                    }
                    Err(_) => std::process::exit(9),
                }
            }
        } else if let Some(ms) = directive.as_ref().and_then(|d| d["$sleep"].as_u64()) {
            std::thread::sleep(Duration::from_millis(ms));
        } else if let Some(code) = directive.as_ref().and_then(|d| d["$exit"].as_i64()) {
            std::process::exit(code as i32);
        } else {
            let mut out = stdout.lock();
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    }
}
