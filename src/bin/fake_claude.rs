//! A stand-in for `claude` that replays a scenario, so tests run without a model.
//!
//! FAKE_SCENARIO: a file of lines. A line is printed to stdout as it is, unless it is a directive:
//!   {"$wait":"user"|"control_response"|"interrupt"}  read stdin until such a line arrives
//!   {"$sleep":ms}                                    wait
//!   {"$exit":code}                                   end the process
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

fn matches(line: &str, wanted: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return false };
    let kind = v["type"].as_str().unwrap_or_default();
    match wanted {
        "user" => kind == "user",
        "control_response" => kind == "control_response",
        "interrupt" => kind == "control_request" && v["request"]["subtype"] == "interrupt",
        _ => false,
    }
}

fn main() {
    let log_path = std::env::var("FAKE_LOG").ok();
    log(&log_path, &format!("ARGV {}", std::env::args().skip(1).collect::<Vec<_>>().join(" ")));

    // The auto-save extractor asks for one JSON answer (--output-format json) and writes its prompt on stdin, whole.
    let args: Vec<String> = std::env::args().collect();
    if args.windows(2).any(|w| w[0] == "--output-format" && w[1] == "json") {
        let mut prompt = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut prompt);
        log(&log_path, &format!("EXTRACT_IN {}", prompt.replace('\n', "\\n")));
        let answer = std::env::var("FAKE_EXTRACT").ok().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_else(|| r#"{"type":"result","subtype":"success","is_error":false,"result":"{}","structured_output":{"memories":[]}}"#.to_owned());
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
