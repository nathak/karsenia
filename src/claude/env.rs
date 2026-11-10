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
        Provider { id: "p1".into(), name: "P".into(), base_url: "https://gw.example.com/x".into(), auth, token: token.map(str::to_owned), model: None, models: Aliases::default(), created: 0, updated: 0 }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn value<'a>(plan: &'a EnvPlan, name: &str) -> Option<&'a str> {
        plan.set.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
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
}
