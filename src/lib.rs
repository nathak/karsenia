//! SmartWorkbench App: runs `claude` headless and serves a chat UI for it on this machine only.
pub mod claude;
pub mod private;
pub mod registry;
pub mod server;
pub mod session;

/// `n` random bytes as lower-case hex.
pub fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    getrandom::getrandom(&mut bytes).expect("the system has no random source");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
