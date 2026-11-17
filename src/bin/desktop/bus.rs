//! The window's side of the in-process link (`tenrec::server::local`): it sends the web page's operations and keeps the ones
//! that wait for an answer (a resend with `confirm`, an undo). Nothing sent or received is logged: notes and keys pass here.
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

pub struct Bus {
    tx: UnboundedSender<Value>,
    next: AtomicU64,
    pending: Mutex<HashMap<u64, Value>>,
    /// The account the panels show (None: the default one); an operation that names none is sent for it.
    account: Mutex<Option<String>>,
}

impl Bus {
    pub fn new(tx: UnboundedSender<Value>) -> Self {
        Bus { tx, next: AtomicU64::new(1), pending: Mutex::new(HashMap::new()), account: Mutex::new(None) }
    }

    /// Changes the panels' account; true when it changed.
    pub fn set_account(&self, id: Option<String>) -> bool {
        let mut account = self.account.lock().unwrap_or_else(|e| e.into_inner());
        if *account == id {
            return false;
        }
        *account = id;
        true
    }

    pub fn account(&self) -> Option<String> {
        self.account.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// "account": the panels' account, unless the operation names one.
    fn stamp(&self, op: &mut Value) {
        let account = self.account().map_or(Value::Null, Value::String);
        if let Some(map) = op.as_object_mut() {
            map.entry("account").or_insert(account);
        }
    }

    /// Sends one operation. It gets a number as "rid" and "req" (replies carry one or the other), and the panels' account
    /// unless it names one: an operation without the key is refused rather than answered from the default account.
    pub fn send(&self, mut op: Value) -> u64 {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        self.stamp(&mut op);
        if let Some(map) = op.as_object_mut() {
            map.insert("rid".into(), n.into());
            map.insert("req".into(), n.into());
        }
        let _ = self.tx.send(op);
        n
    }

    /// Sends and keeps the operation until its result comes back (`take`).
    pub fn ask(&self, mut op: Value) -> u64 {
        // Stamped before it is kept: a resend after a question goes to the account it was asked for.
        self.stamp(&mut op);
        let keep = op.clone();
        let n = self.send(op);
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        // A result that never came is not kept forever.
        if pending.len() > 200 {
            pending.clear();
        }
        pending.insert(n, keep);
        n
    }

    /// `ask` when the number is not needed.
    pub fn post(&self, op: Value) {
        self.ask(op);
    }

    /// `send` when the number is not needed.
    pub fn post_quiet(&self, op: Value) {
        self.send(op);
    }

    pub fn take(&self, rid: u64) -> Option<Value> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&rid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn operations_carry_the_panels_account_unless_they_name_one() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let bus = Bus::new(tx);
        bus.send(json!({"op": "memory_list"}));
        assert_eq!(rx.try_recv().expect("sent")["account"], Value::Null);
        assert!(bus.set_account(Some("a1".into())));
        assert!(!bus.set_account(Some("a1".into())));
        bus.send(json!({"op": "memory_list"}));
        assert_eq!(rx.try_recv().expect("sent")["account"], "a1");
        bus.send(json!({"op": "skill_propose", "account": "a2"}));
        assert_eq!(rx.try_recv().expect("sent")["account"], "a2");
        let n = bus.ask(json!({"op": "memory_add"}));
        bus.set_account(None);
        assert_eq!(bus.take(n).expect("kept")["account"], "a1");
    }
}
