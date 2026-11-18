//! A Codex account: made, asked about its login through the `codex` program, kept out of everything that is Claude's (chats, notes, the
//! picker), and removed with its folder. The program is a fake script (the real one needs a login); it sets the variable of the default
//! login folder, so this is the only test of its file.
use std::os::unix::fs::PermissionsExt;

use tenrec::accounts::AccountKind;
use tenrec::server::{start, Config};

#[tokio::test]
async fn a_codex_account_is_managed_through_the_program_and_has_no_chats_or_realm() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let log = root.join("codex.log");
    let fake = root.join("codex");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\n{{ echo \"ARGS $*\"; echo \"HOME $CODEX_HOME\"; echo \"CWD $(pwd)\"; echo \"OPENAI ${{OPENAI_API_KEY-unset}}\"; }} >> '{}'\nif [ \"$1\" = login ] && [ \"$2\" = status ]; then\n  echo 'WARNING: a warning first' >&2\n  if [ -f \"$CODEX_HOME/auth.json\" ]; then echo 'Logged in using ChatGPT' >&2; exit 0; fi\n  echo 'Not logged in' >&2; exit 1\nfi\nexit 0\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: the only test in this file, and nothing else runs yet.
    unsafe {
        std::env::set_var("TENREC_CODEX", &fake);
        std::env::set_var("OPENAI_API_KEY", "sk-must-not-reach-the-program");
    }
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let config = || Config { port: 0, data_dir: root.join("data"), bin: None, mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() };
    let server = start(config()).await.unwrap();
    let registry = &server.registry;

    let work = registry.add_account("Work", AccountKind::Claude).unwrap();
    let account = registry.add_account("Codex Work", AccountKind::Codex).unwrap();
    assert!(registry.add_account("Work", AccountKind::Codex).is_err(), "a name is used once across kinds");
    assert_eq!(account.kind, AccountKind::Codex);
    let data = root.join("data");
    assert!(data.join("accounts").join(&account.id).is_dir());
    assert!(!data.join("stores").join(&account.id).exists(), "no notes, skills or history");

    // It cannot take a chat, a Claude login, a realm or a plug-in; a Claude account cannot be asked as a Codex one.
    let chat = registry.create_in(project.to_str().unwrap(), None, None, false, Some(&account.id));
    assert!(chat.err().unwrap_or_default().contains("Codex"), "the reason is told");
    assert!(registry.account_login(&account.id).is_err());
    assert!(registry.realm(Some(&account.id)).is_err());
    assert_eq!(registry.all_realms().len(), 2, "the default realm and the Claude account's");
    assert!(registry.codex_home(&work.id).is_err());

    // Its login is asked of the program, in the account's own folder, with the credential variables of this process taken away.
    let home = registry.codex_home(&account.id).unwrap();
    let path_var = std::env::var_os("PATH");
    let bin = tenrec::codex::bin(path_var.as_deref()).expect("TENREC_CODEX names the program");
    let plan = tenrec::codex::plan(&tenrec::claude::env::inherited_names(), &home);
    let out = tenrec::codex::status(bin.to_str().unwrap(), &home, &plan).await.unwrap();
    assert_eq!((out.logged_in, out.known, out.file), (false, true, false), "not logged in, and no login file");
    std::fs::write(home.join("auth.json"), "{}").unwrap();
    let out = tenrec::codex::status(bin.to_str().unwrap(), &home, &plan).await.unwrap();
    assert_eq!((out.logged_in, out.method, out.file), (true, Some("chatgpt"), true));
    let seen = std::fs::read_to_string(&log).unwrap();
    assert!(seen.contains("ARGS login status") && seen.contains(&format!("HOME {}", home.display())) && seen.contains(&format!("CWD {}", home.display())), "{seen}");
    assert!(seen.contains("OPENAI unset") && !seen.contains("sk-must-not"), "no key of this process reaches the program: {seen}");

    // Another start of the app finds the account without a notice (it has no realm to open).
    drop(server);
    let server = start(config()).await.unwrap();
    assert!(server.registry.accounts_notice.is_none(), "{:?}", server.registry.accounts_notice);
    assert_eq!(server.registry.accounts().get(&account.id).unwrap().kind, AccountKind::Codex);

    // Removing it asks the program to log out, then takes the folder; nothing is left over.
    let warnings = server.registry.delete_account(&account.id).await.unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(!data.join("accounts").join(&account.id).exists());
    assert!(server.registry.accounts().get(&account.id).is_none());
    let seen = std::fs::read_to_string(&log).unwrap();
    assert!(seen.contains("ARGS logout"), "{seen}");
}
