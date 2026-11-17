//! The terminal is not built on Windows: a shell on a pseudo-terminal needs the Unix pty calls, and the Windows counterpart (ConPTY)
//! has not been checked here. The page is told the terminal is off (`hello` carries `terminal: false`), and a `term_*` request is
//! refused with a reason.
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::{plain, AppState};

pub const FRAME_OUTPUT: u8 = 1;
pub const FRAME_EXIT: u8 = 2;

#[derive(Default)]
pub struct Terms;

pub fn kill_all() {}

pub async fn handle(_state: &AppState, _name: &str, op: &Value, out: &mpsc::UnboundedSender<String>, _bin: &mpsc::Sender<Vec<u8>>, _terms: &mut Terms) {
    let _ = out.send(plain("term_error", None, json!({"req": op["req"].clone(), "tid": op["tid"].as_u64(), "message": "the terminal is not available on Windows"})));
}
