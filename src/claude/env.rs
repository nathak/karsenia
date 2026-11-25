//! The environment a `claude` process of a provider chat gets: a pure plan, so it can be tested without starting anything.
//!
//! Measured against the real CLI (see the README): with only `ANTHROPIC_BASE_URL` changed, the logged-in account's OAuth token is
//! sent to that address. So a provider chat always sets a token of its own (a placeholder when the endpoint wants none), runs in a
//! config folder of its own (the account's credentials file is never in reach of the CLI), and loses every inherited variable that
//! could carry or redirect a credential. A normal chat gets the empty plan: its environment is whatever it was.
use std::path::Path;

use crate::providers::{Auth, Provider};

/// Used when the endpoint wants no key: the CLI needs some token, or it falls back to the account's own.
pub const PLACEHOLDER_TOKEN: &str = "tenrec-no-key";

#[derive(Clone, Default)]
pub struct EnvPlan {
    pub set: Vec<(String, String)>,
    pub remove: Vec<String>,
}

impl EnvPlan {
    /// What a person may be shown of what this plan sets, for a debug view: each name with its value, except that a value that carries a
    /// credential is only said to be set (and how long it is), and the lines of `ANTHROPIC_CUSTOM_HEADERS` show their names only.
    pub fn describe(&self) -> Vec<(String, String)> {
        let secret = |name: &str| {
            let upper = name.to_ascii_uppercase();
            ["TOKEN", "KEY", "SECRET", "PASSWORD", "CREDENTIAL"].iter().any(|word| upper.contains(word))
        };
        self.set
            .iter()
            .map(|(name, value)| {
                let shown = if name == "ANTHROPIC_CUSTOM_HEADERS" {
                    let names: Vec<String> = value.lines().filter_map(|line| line.split_once(':')).map(|(header, _)| format!("{}: ***", header.trim())).collect();
                    format!("(set: {})", names.join(", "))
                } else if secret(name) {
                    format!("(set, {} characters)", value.chars().count())
                } else {
                    value.clone()
                };
                (name.clone(), shown)
            })
            .collect()
    }
}

/// Values are never printed (the token is one of them): only the names.
impl std::fmt::Debug for EnvPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvPlan").field("set", &self.set.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>()).field("remove", &self.remove).finish()
    }
}

/// Variables that carry a credential, choose where requests go, or pick another cloud: prefixes, so a variable added by a later
/// CLI version is covered too.
pub fn is_credential_var(name: &str) -> bool {
    ["ANTHROPIC_", "CLAUDE_CODE_USE_", "CLAUDE_CODE_SKIP_", "CLAUDE_CODE_OAUTH", "CLAUDE_CODE_API_KEY", "CLAUDE_CONFIG_DIR", "AWS_BEARER_TOKEN_BEDROCK", "TENREC_MCP_", "SWB_MCP_"].iter().any(|prefix| name.starts_with(prefix))
}

/// `inherited` are the names of this process's variables.
pub fn plan(inherited: &[String], provider: Option<&Provider>, config_dir: &Path) -> EnvPlan {
    let Some(provider) = provider else { return EnvPlan::default() };
    let mut set: Vec<(String, String)> = vec![("ANTHROPIC_BASE_URL".into(), provider.base_url.clone())];
    match provider.auth {
        Auth::Bearer => set.push(("ANTHROPIC_AUTH_TOKEN".into(), provider.token.clone().unwrap_or_else(|| PLACEHOLDER_TOKEN.into()))),
        Auth::XApiKey => set.push(("ANTHROPIC_API_KEY".into(), provider.token.clone().unwrap_or_else(|| PLACEHOLDER_TOKEN.into()))),
        // The key goes in the header lines only; the CLI still needs some credential of its own, so it gets the placeholder.
        Auth::Headers => set.push(("ANTHROPIC_AUTH_TOKEN".into(), PLACEHOLDER_TOKEN.into())),
        // An account's own login is the CLI's credential: nothing of the provider's is set, so no variable outranks it.
        Auth::AccountLogin => {}
    }
    // A gateway that wants its key in a header of its own: the lines of the provider, with the key where it says `{key}`.
    if let Some(headers) = provider.custom_headers() {
        set.push(("ANTHROPIC_CUSTOM_HEADERS".into(), headers));
    }
    set.push(("CLAUDE_CONFIG_DIR".into(), config_dir.display().to_string()));
    set.push(("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(), "1".into()));
    for (name, value) in [("ANTHROPIC_DEFAULT_OPUS_MODEL", &provider.models.opus), ("ANTHROPIC_DEFAULT_SONNET_MODEL", &provider.models.sonnet), ("ANTHROPIC_DEFAULT_HAIKU_MODEL", &provider.models.haiku)] {
        if let Some(value) = value {
            set.push((name.into(), value.clone()));
        }
    }
    let mut remove: Vec<String> = inherited.iter().filter(|name| is_credential_var(name) && !set.iter().any(|(k, _)| k == *name)).cloned().collect();
    remove.sort();
    remove.dedup();
    EnvPlan { set, remove }
}

/// The environment of a provider chat that uses an account's login (`Auth::AccountLogin`): the provider's address and header lines, and the
/// folder of that account. `account_dir` is None for the default account: its folder is wherever the CLI looks by itself (so
/// `CLAUDE_CONFIG_DIR` is neither set nor taken away: a person who moved it keeps the login where it is). Any other credential of
/// this app's own environment is taken away, as for every provider chat.
pub fn login_plan(inherited: &[String], provider: &Provider, account_dir: Option<&Path>) -> EnvPlan {
    let mut plan = plan(inherited, Some(provider), account_dir.unwrap_or_else(|| Path::new("")));
    if account_dir.is_none() {
        plan.set.retain(|(name, _)| name != "CLAUDE_CONFIG_DIR");
        plan.remove.retain(|name| name != "CLAUDE_CONFIG_DIR");
    }
    plan
}

/// The environment of a `claude` that runs for one of the person's extra accounts: its own settings folder, and none of the
/// credential variables the app inherited, so the default account's token or address cannot be used in its place. Everything else
/// stays (proxy, certificate, locale, `PATH`: a work network needs them). Unlike a provider chat nothing is pinned: the person's
/// own project settings still apply.
pub fn account_plan(inherited: &[String], config_dir: &Path) -> EnvPlan {
    let mut remove: Vec<String> = inherited.iter().filter(|name| is_credential_var(name) && name.as_str() != "CLAUDE_CONFIG_DIR").cloned().collect();
    remove.sort();
    remove.dedup();
    EnvPlan { set: vec![("CLAUDE_CONFIG_DIR".into(), config_dir.display().to_string())], remove }
}

/// What the app calls itself to a provider.
pub const APP_UA: &str = concat!("Karsenia/", env!("CARGO_PKG_VERSION"));

/// Appends the app's name to the program's own User-Agent for a provider chat. The program replaces its whole User-Agent with one given in
/// `ANTHROPIC_CUSTOM_HEADERS` (measured with 2.1.326: `claude-cli/2.1.326 (external, sdk-cli)`), so the program's own part is written
/// back first and left as it was. With no provider, no (clean) program version yet, or a `User-Agent` line the person wrote in the
/// provider's header lines, the plan is returned as it is. The line is added after the `{key}` of the header lines was filled in, so it
/// never goes through that, and it holds nothing but a clean version and a constant.
pub fn with_user_agent(mut plan: EnvPlan, provider: Option<&Provider>, cli_version: Option<&str>) -> EnvPlan {
    let Some(provider) = provider else { return plan };
    let own = provider.headers.as_deref().unwrap_or_default().lines().any(|line| line.split_once(':').is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("user-agent")));
    let Some(version) = cli_version.and_then(super::cli_version::clean) else { return plan };
    if own {
        return plan;
    }
    let line = format!("User-Agent: claude-cli/{version} (external, sdk-cli) {APP_UA}");
    match plan.set.iter_mut().find(|(name, _)| name == "ANTHROPIC_CUSTOM_HEADERS") {
        Some((_, value)) => {
            value.push('\n');
            value.push_str(&line);
        }
        None => {
            plan.set.push(("ANTHROPIC_CUSTOM_HEADERS".into(), line));
            plan.remove.retain(|name| name != "ANTHROPIC_CUSTOM_HEADERS");
        }
    }
    plan
}

/// The names of this process's variables, for `plan` and `account_plan`.
pub fn inherited_names() -> Vec<String> {
    std::env::vars_os().filter_map(|(name, _)| name.into_string().ok()).collect()
}

/// The settings given on the command line: they pin the address, because a project's `.claude/settings.json` may carry an `env`
/// that would otherwise replace it (measured), and the token would follow the address. Only the address, which is not a secret.
pub fn pin_settings(base_url: &str) -> String {
    serde_json::json!({"env": {"ANTHROPIC_BASE_URL": base_url}}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Aliases;

    fn provider(auth: Auth, token: Option<&str>) -> Provider {
        Provider { id: "p1".into(), name: "P".into(), base_url: "https://gw.example.com/x".into(), auth, token: token.map(str::to_owned), login_account: None, model: None, models: Aliases::default(), headers: None, created: 0, updated: 0 }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn value<'a>(plan: &'a EnvPlan, name: &str) -> Option<&'a str> {
        plan.set.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    #[test]
    fn an_account_chat_runs_in_its_own_folder_and_loses_every_inherited_credential_but_nothing_else() {
        let inherited = names(&[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_MODEL",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CONFIG_DIR",
            "AWS_BEARER_TOKEN_BEDROCK",
            "TENREC_MCP_SECRET",
            "HOME",
            "PATH",
            "LANG",
            "TZ",
            "HTTPS_PROXY",
            "NO_PROXY",
            "NODE_EXTRA_CA_CERTS",
            "SSL_CERT_FILE",
            "XDG_RUNTIME_DIR",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
        ]);
        let plan = account_plan(&inherited, Path::new("/data/accounts/ab"));
        assert_eq!(plan.set, vec![("CLAUDE_CONFIG_DIR".to_owned(), "/data/accounts/ab".to_owned())], "only the folder is set: no token, no address");
        for gone in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL", "ANTHROPIC_MODEL", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CODE_USE_BEDROCK", "AWS_BEARER_TOKEN_BEDROCK", "TENREC_MCP_SECRET"] {
            assert!(plan.remove.iter().any(|n| n == gone), "{gone} must be removed");
        }
        for kept in ["HOME", "PATH", "LANG", "TZ", "HTTPS_PROXY", "NO_PROXY", "NODE_EXTRA_CA_CERTS", "SSL_CERT_FILE", "XDG_RUNTIME_DIR", "CLAUDE_CODE_MAX_OUTPUT_TOKENS", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"] {
            assert!(!plan.remove.iter().any(|n| n == kept), "{kept} stays");
        }
        assert!(!plan.remove.iter().any(|n| n == "CLAUDE_CONFIG_DIR"), "what is set is not also removed");
        assert!(account_plan(&[], Path::new("/c")).remove.is_empty());
    }

    #[test]
    fn a_normal_chat_changes_nothing() {
        let plan = plan(&names(&["ANTHROPIC_API_KEY", "HOME"]), None, Path::new("/cfg"));
        assert!(plan.set.is_empty() && plan.remove.is_empty());
    }

    #[test]
    fn a_provider_chat_sets_its_own_token_and_strips_everything_that_could_carry_or_redirect_a_credential() {
        let inherited = names(&[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_CUSTOM_HEADERS",
            "ANTHROPIC_FOUNDRY_API_KEY",
            "ANTHROPIC_SOMETHING_NEW",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_SKIP_BEDROCK_AUTH",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CODE_API_KEY_HELPER_TTL_MS",
            "CLAUDE_CONFIG_DIR",
            "AWS_BEARER_TOKEN_BEDROCK",
            "TENREC_MCP_SECRET",
            "SWB_MCP_SECRET",
            "HOME",
            "PATH",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
        ]);
        let plan = plan(&inherited, Some(&provider(Auth::Bearer, Some("tok-123"))), Path::new("/cfg"));
        for gone in ["ANTHROPIC_API_KEY", "ANTHROPIC_CUSTOM_HEADERS", "ANTHROPIC_FOUNDRY_API_KEY", "ANTHROPIC_SOMETHING_NEW", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_SKIP_BEDROCK_AUTH", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CODE_API_KEY_HELPER_TTL_MS", "AWS_BEARER_TOKEN_BEDROCK", "TENREC_MCP_SECRET", "SWB_MCP_SECRET"] {
            assert!(plan.remove.iter().any(|n| n == gone), "{gone} must be removed");
        }
        assert!(!plan.remove.iter().any(|n| n == "HOME" || n == "PATH" || n == "CLAUDE_CODE_MAX_OUTPUT_TOKENS"), "harmless variables stay");
        // What the plan sets is not also removed (the set wins), and each of those is set once.
        for (name, _) in &plan.set {
            assert!(!plan.remove.contains(name), "{name} is both set and removed");
            assert_eq!(plan.set.iter().filter(|(k, _)| k == name).count(), 1);
        }
        assert_eq!(value(&plan, "ANTHROPIC_BASE_URL"), Some("https://gw.example.com/x"));
        assert_eq!(value(&plan, "ANTHROPIC_AUTH_TOKEN"), Some("tok-123"));
        assert_eq!(value(&plan, "CLAUDE_CONFIG_DIR"), Some("/cfg"));
        assert_eq!(value(&plan, "ANTHROPIC_API_KEY"), None, "bearer and x-api-key are never both set");
    }

    #[test]
    fn a_token_variable_is_always_set_exactly_once_a_placeholder_when_the_endpoint_wants_none() {
        let keyless = plan(&[], Some(&provider(Auth::Bearer, None)), Path::new("/cfg"));
        assert_eq!(value(&keyless, "ANTHROPIC_AUTH_TOKEN"), Some(PLACEHOLDER_TOKEN));
        let keyed = plan(&[], Some(&provider(Auth::XApiKey, Some("sk-abc"))), Path::new("/cfg"));
        assert_eq!(value(&keyed, "ANTHROPIC_API_KEY"), Some("sk-abc"));
        assert_eq!(value(&keyed, "ANTHROPIC_AUTH_TOKEN"), None);
    }

    #[test]
    fn model_names_become_variables_and_debug_shows_names_only() {
        let mut p = provider(Auth::Bearer, Some("tok-secret-xyz"));
        p.models = Aliases { opus: Some("big".into()), sonnet: None, haiku: Some("small".into()) };
        let plan = plan(&[], Some(&p), Path::new("/cfg"));
        assert_eq!(value(&plan, "ANTHROPIC_DEFAULT_OPUS_MODEL"), Some("big"));
        assert_eq!(value(&plan, "ANTHROPIC_DEFAULT_SONNET_MODEL"), None);
        assert_eq!(value(&plan, "ANTHROPIC_DEFAULT_HAIKU_MODEL"), Some("small"));
        let shown = format!("{plan:?} {p:?}");
        assert!(!shown.contains("tok-secret-xyz"), "{shown}");
    }

    #[test]
    fn the_pinned_settings_hold_the_address_and_nothing_else() {
        let text = pin_settings("https://gw.example.com/x");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v, serde_json::json!({"env": {"ANTHROPIC_BASE_URL": "https://gw.example.com/x"}}));
    }

    #[test]
    fn a_providers_header_lines_get_its_key_where_they_say_so_and_only_when_it_has_headers() {
        let mut p = provider(Auth::Bearer, Some("airkey-123456"));
        let plain = plan(&[], Some(&p), Path::new("/cfg"));
        assert!(!plain.set.iter().any(|(k, _)| k == "ANTHROPIC_CUSTOM_HEADERS"));
        p.headers = Some("AIR-Api-Key: {key}\nAIR-Coding-Agent-Mode: subscription".into());
        let with = plan(&names(&["ANTHROPIC_CUSTOM_HEADERS"]), Some(&p), Path::new("/cfg"));
        assert_eq!(value(&with, "ANTHROPIC_CUSTOM_HEADERS"), Some("AIR-Api-Key: airkey-123456\nAIR-Coding-Agent-Mode: subscription"));
        assert!(!with.remove.contains(&"ANTHROPIC_CUSTOM_HEADERS".to_owned()), "set by the plan, so not also removed");
        // An inherited value of the variable is removed when the provider has none: a chat never gets another one's headers.
        let none = provider(Auth::Bearer, Some("airkey-123456"));
        let stripped = plan(&names(&["ANTHROPIC_CUSTOM_HEADERS"]), Some(&none), Path::new("/cfg"));
        assert!(stripped.remove.contains(&"ANTHROPIC_CUSTOM_HEADERS".to_owned()));
    }

    #[test]
    fn a_plan_is_described_without_a_credential_or_a_header_value() {
        let mut p = provider(Auth::Bearer, Some("airkey-123456"));
        p.headers = Some("AIR-Api-Key: {key}\nAIR-Coding-Agent-Mode: subscription".into());
        let shown = plan(&[], Some(&p), Path::new("/cfg")).describe();
        let all = format!("{shown:?}");
        assert!(!all.contains("airkey-123456") && !all.contains("subscription"), "{all}");
        let get = |name: &str| shown.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
        assert_eq!(get("ANTHROPIC_BASE_URL"), Some("https://gw.example.com/x"), "an address is shown");
        assert_eq!(get("ANTHROPIC_AUTH_TOKEN"), Some("(set, 13 characters)"));
        assert_eq!(get("ANTHROPIC_CUSTOM_HEADERS"), Some("(set: AIR-Api-Key: ***, AIR-Coding-Agent-Mode: ***)"));
        assert_eq!(get("CLAUDE_CONFIG_DIR"), Some("/cfg"));
    }

    #[test]
    fn in_headers_only_mode_the_key_is_in_the_header_lines_and_authorization_gets_the_placeholder() {
        let mut p = provider(Auth::Headers, Some("airkey-123456"));
        p.headers = Some("AIR-Api-Key: {key}".into());
        let plan = plan(&[], Some(&p), Path::new("/cfg"));
        assert_eq!(value(&plan, "ANTHROPIC_CUSTOM_HEADERS"), Some("AIR-Api-Key: airkey-123456"));
        assert_eq!(value(&plan, "ANTHROPIC_AUTH_TOKEN"), Some(PLACEHOLDER_TOKEN), "not the key");
        assert_eq!(value(&plan, "ANTHROPIC_API_KEY"), None);
        let all = format!("{:?}", plan.set.iter().filter(|(k, _)| k != "ANTHROPIC_CUSTOM_HEADERS").collect::<Vec<_>>());
        assert!(!all.contains("airkey-123456"), "the key is in no other variable: {all}");
    }

    #[test]
    fn an_account_login_provider_sets_no_credential_and_leaves_the_default_accounts_folder_alone() {
        let mut p = provider(Auth::AccountLogin, Some("airkey-123456"));
        p.headers = Some("AIR-Api-Key: {key}".into());
        let inherited = names(&["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CONFIG_DIR"]);
        // The default account: its folder is wherever the CLI looks, so the variable is neither set nor removed.
        let plan = login_plan(&inherited, &p, None);
        for name in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CONFIG_DIR"] {
            assert_eq!(value(&plan, name), None, "{name} is not set");
        }
        assert_eq!(value(&plan, "ANTHROPIC_BASE_URL"), Some("https://gw.example.com/x"));
        assert_eq!(value(&plan, "ANTHROPIC_CUSTOM_HEADERS"), Some("AIR-Api-Key: airkey-123456"));
        assert!(!plan.remove.contains(&"CLAUDE_CONFIG_DIR".to_owned()), "a moved default folder keeps working");
        for name in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"] {
            assert!(plan.remove.contains(&name.to_owned()), "an inherited {name} is taken away");
        }
        // An extra account: its folder.
        let extra = login_plan(&inherited, &p, Some(Path::new("/data/accounts/0123456789abcdef")));
        assert_eq!(value(&extra, "CLAUDE_CONFIG_DIR"), Some("/data/accounts/0123456789abcdef"));
        assert_eq!(value(&extra, "ANTHROPIC_AUTH_TOKEN"), None);
    }

    fn ua(plan: &EnvPlan) -> Option<&str> {
        value(plan, "ANTHROPIC_CUSTOM_HEADERS")
    }

    #[test]
    fn a_provider_chat_gets_the_apps_name_after_the_programs_own_user_agent() {
        let mut p = provider(Auth::Bearer, Some("airkey-123456"));
        p.headers = Some("AIR-Api-Key: {key}".into());
        let with = with_user_agent(plan(&[], Some(&p), Path::new("/cfg")), Some(&p), Some("2.1.326"));
        let expected = format!("AIR-Api-Key: airkey-123456\nUser-Agent: claude-cli/2.1.326 (external, sdk-cli) Karsenia/{}", env!("CARGO_PKG_VERSION"));
        assert_eq!(ua(&with), Some(expected.as_str()));
        // Without header lines the line stands alone, and an inherited value of the variable is not also removed.
        let none = provider(Auth::Bearer, Some("airkey-123456"));
        let alone = with_user_agent(plan(&names(&["ANTHROPIC_CUSTOM_HEADERS"]), Some(&none), Path::new("/cfg")), Some(&none), Some("2.1.326"));
        assert_eq!(ua(&alone), Some(format!("User-Agent: claude-cli/2.1.326 (external, sdk-cli) Karsenia/{}", env!("CARGO_PKG_VERSION")).as_str()));
        assert!(!alone.remove.contains(&"ANTHROPIC_CUSTOM_HEADERS".to_owned()));
        // The key is only where the header lines put it, never in the User-Agent line.
        let line = ua(&alone).unwrap();
        assert!(!line.contains("airkey-123456"));
    }

    #[test]
    fn a_chat_without_a_provider_and_one_without_a_known_version_are_left_as_they_were() {
        assert!(with_user_agent(plan(&[], None, Path::new("/cfg")), None, Some("2.1.326")).set.is_empty());
        let p = provider(Auth::Bearer, Some("airkey-123456"));
        let base = plan(&[], Some(&p), Path::new("/cfg"));
        assert!(ua(&with_user_agent(base.clone(), Some(&p), None)).is_none());
        for bad in ["2.1.326\r\nX-Evil: 1", "2.1.326 (x)", "", &"1".repeat(33), "{key}", "２.１"] {
            assert!(ua(&with_user_agent(base.clone(), Some(&p), Some(bad))).is_none(), "{bad:?}");
        }
        assert!(ua(&with_user_agent(base, Some(&p), Some("2.1.326-beta+3"))).is_some());
    }

    #[test]
    fn a_user_agent_the_person_wrote_wins_whatever_its_case() {
        let mut p = provider(Auth::Bearer, Some("airkey-123456"));
        p.headers = Some("user-agent: Mine/1".into());
        let plan = with_user_agent(plan(&[], Some(&p), Path::new("/cfg")), Some(&p), Some("2.1.326"));
        assert_eq!(ua(&plan), Some("user-agent: Mine/1"));
    }

    #[test]
    fn an_account_login_provider_gets_it_too() {
        let p = provider(Auth::AccountLogin, None);
        for dir in [None, Some(Path::new("/data/accounts/0123456789abcdef"))] {
            let plan = with_user_agent(login_plan(&[], &p, dir), Some(&p), Some("2.1.326"));
            assert!(ua(&plan).is_some_and(|v| v.starts_with("User-Agent: claude-cli/2.1.326 (external, sdk-cli) Karsenia/")));
        }
    }
}
