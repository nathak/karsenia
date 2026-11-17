//! Installing a plugin of the default account into an extra account: the marketplace is added to the account first when it lacks it, then the
//! plugin is installed, each run as the account (its own settings folder); a plugin the default account does not have is refused.
use tenrec::claude::env::EnvPlan;

#[tokio::test]
async fn a_plugin_of_the_default_account_is_installed_into_the_account_with_its_marketplace() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let log = root.join("calls.log");
    let bin = root.join("claude");
    std::fs::write(&bin, format!("#!/bin/sh\necho \"$CLAUDE_CONFIG_DIR|$*\" >> '{}'\ncase \"$*\" in\n  \"plugin list --json\") if [ -n \"$CLAUDE_CONFIG_DIR\" ]; then echo '[]'; else echo '[{{\"id\":\"sw@smart-workbench\",\"version\":\"1\",\"scope\":\"user\"}}]'; fi ;;\n  \"plugin marketplace list --json\") if [ -n \"$CLAUDE_CONFIG_DIR\" ]; then echo '[]'; else echo '[{{\"name\":\"smart-workbench\",\"source\":\"github\",\"repo\":\"nathak/smart-workbench\"}}]'; fi ;;\nesac\n", log.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let account = root.join("acct");
    std::fs::create_dir_all(&account).unwrap();
    let env = EnvPlan { set: vec![("CLAUDE_CONFIG_DIR".into(), account.display().to_string())], remove: vec![] };
    let bin = bin.display().to_string();

    tenrec::plugins::install(&bin, &root, &account, &env, "sw@smart-workbench").await.unwrap();
    let calls = std::fs::read_to_string(&log).unwrap();
    let acct = account.display().to_string();
    assert!(calls.contains(&format!("{acct}|plugin marketplace add nathak/smart-workbench")), "{calls}");
    assert!(calls.contains(&format!("{acct}|plugin install sw@smart-workbench --scope user")), "{calls}");
    // Only the account's folder is written to: every add and install ran with its folder.
    assert!(calls.lines().filter(|l| l.contains("plugin install") || l.contains("marketplace add")).all(|l| l.starts_with(&acct)), "{calls}");

    // A plugin the default account does not have, and an id that is not plain, are refused before anything is installed.
    assert!(tenrec::plugins::install(&bin, &root, &account, &env, "other@smart-workbench").await.is_err());
    assert!(tenrec::plugins::install(&bin, &root, &account, &env, "x;touch-y@z").await.is_err());
    let after = std::fs::read_to_string(&log).unwrap();
    assert!(!after.contains("other@smart-workbench --scope") && !after.contains("touch-y"), "{after}");
}
