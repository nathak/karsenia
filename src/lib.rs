//! SmartWorkbench App: runs `claude` headless and serves a chat UI for it on this machine only.
pub mod claude;
pub mod extract;
pub mod index;
pub mod mcp;
pub mod propose;
pub mod memory;
pub mod private;
pub mod registry;
pub mod sched_store;
pub mod schedule;
pub mod scheduler;
pub mod server;
pub mod session;
pub mod settings;
pub mod skills;

/// `n` random bytes as lower-case hex.
pub fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    getrandom::getrandom(&mut bytes).expect("the system has no random source");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Milliseconds since the epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
