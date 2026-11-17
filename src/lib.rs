//! Tenrec: runs `claude` headless and serves a chat UI for it on this machine only.
pub mod accounts;
pub mod cli_sessions;
pub mod claude;
pub mod extract;
pub mod index;
pub mod mcp;
pub mod view;
pub mod platform;
pub mod plugins;
pub mod proc_group;
pub mod propose;
pub mod realm;
pub mod memory;
pub mod private;
pub mod providers;
pub mod registry;
pub mod sched_store;
pub mod schedule;
pub mod scheduler;
pub mod server;
pub mod session;
pub mod settings;
pub mod skills;
pub mod usage;
pub mod worktree;

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
