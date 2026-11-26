use anyhow::{Context, Result, ensure};
use reqwest::Url;
use serde_json::{Value, json};
use std::{collections::HashMap, env, path::PathBuf, time::Duration};

pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub subscription_key: String,
    api_base: Url,
    oauth_base_url: Url,
    pub rate: f64,
    pub weekly_limit: i64,
    pub quota_file: PathBuf,
    pub token_store: String,
    pub token_file: PathBuf,
    pub default_account: String,
    pub data_dir: PathBuf,
    pub export_dir: PathBuf,
    pub max_base64_bytes: usize,
    pub bearer: String,
    pub webhook_secret: String,
    pub webhook_store: String,
    pub webhook_file: PathBuf,
    pub write_tools: bool,
    pub max_sessions: usize,
    pub session_ttl: Duration,
    display: Value,
}

fn boolean(value: &str, name: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "on" | "t" | "true" | "y" | "yes" => Ok(true),
        "0" | "off" | "f" | "false" | "n" | "no" => Ok(false),
        _ => anyhow::bail!("Invalid SUUNTO_{name}: expected a boolean"),
    }
}

fn redact(value: &str) -> String {
    match value.chars().count() {
        0 => String::new(),
        1..=8 => "***".into(),
        _ => format!("{}...", value.chars().take(4).collect::<String>()),
    }
}

pub fn expand_path(path: &str) -> PathBuf {
    if path == "~" {
        env::home_dir().unwrap_or_else(|| PathBuf::from(path))
    } else if let Some(suffix) = path.strip_prefix("~/") {
        env::home_dir().map_or_else(|| PathBuf::from(path), |home| home.join(suffix))
    } else {
        PathBuf::from(path)
    }
}

fn service_base(raw: &str, setting: &str) -> Result<Url> {
    let mut url = Url::parse(raw).with_context(|| format!("Invalid {setting}"))?;
    // Unit tests exercise the same request paths against local mock servers.
    let test_loopback = cfg!(test)
        && url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "[::1]"));
    ensure!(
        (url.scheme() == "https" || test_loopback)
            && url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "{setting} must be an HTTPS base URL without credentials, a query, or a fragment"
    );
    if !url.path().ends_with('/') {
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Invalid {setting}"))?
            .push("");
    }
    Ok(url)
}

impl Config {
    pub(crate) fn api_endpoint(&self, path: &str) -> Result<Url> {
        Ok(self.api_base.join(path.trim_start_matches('/'))?)
    }

    pub(crate) fn oauth_url(&self, path: &str) -> Result<Url> {
        Ok(self.oauth_base_url.join(path)?)
    }

    pub fn load() -> Result<Self> {
        let file = match dotenvy::from_path_iter(".env") {
            Ok(iter) => iter
                .collect::<Result<HashMap<_, _>, _>>()
                .context("Invalid .env file")?,
            Err(dotenvy::Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
                HashMap::new()
            }
            Err(err) => return Err(err).context("Read .env file"),
        };
        Self::from_lookup(|name| env::var(name).ok().or_else(|| file.get(name).cloned()))
    }

    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |name, fallback: &str| {
            lookup(&format!("SUUNTO_{name}")).unwrap_or_else(|| fallback.into())
        };
        let client_id = get("CLIENT_ID", "");
        let client_secret = get("CLIENT_SECRET", "");
        let redirect_uri = get("REDIRECT_URI", "");
        let subscription_key = get("SUBSCRIPTION_KEY", "");
        let api_base = get("API_BASE", "https://cloudapi.suunto.com");
        let oauth_base_url = get("OAUTH_BASE", "https://cloudapi-oauth.suunto.com");
        let api_url = service_base(&api_base, "SUUNTO_API_BASE")?;
        let parsed_oauth_url = service_base(&oauth_base_url, "SUUNTO_OAUTH_BASE")?;
        let bearer = get("MCP_API_TOKEN", "");
        let webhook_secret = get("WEBHOOK_SECRET", "");
        let token_store = get("TOKEN_STORE", "keyring");
        let webhook_store = get("WEBHOOK_STORE", "jsonl");
        let default_account = get("DEFAULT_ACCOUNT_ID", "");
        let home = env::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let local_default = home.join(".suunto-mcp/data");
        let data = get("LOCAL_DATA_DIR", &local_default.to_string_lossy());
        let data_dir = expand_path(&data);
        let parent = data_dir
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let resolve_path = |raw: &str, fallback: PathBuf| {
            if raw.is_empty() {
                fallback
            } else {
                expand_path(raw)
            }
        };
        let quota = get("API_QUOTA_FILE", "");
        let tokens = get("TOKEN_FILE", "");
        let exports = get("EXPORT_DIR", "");
        let webhooks = get("WEBHOOK_STORE_PATH", "");
        let quota_file = resolve_path(&quota, parent.join("api-quota.json"));
        let token_file = resolve_path(
            &tokens,
            parent.join(if token_store == "sqlite" {
                "tokens.sqlite3"
            } else {
                "tokens.json"
            }),
        );
        let export_dir = resolve_path(&exports, PathBuf::new());
        let webhook_file = resolve_path(
            &webhooks,
            parent.join(if webhook_store == "sqlite" {
                "webhooks.sqlite3"
            } else {
                "webhooks.jsonl"
            }),
        );
        let rate: f64 = get("API_CALLS_PER_MINUTE", "10")
            .parse()
            .context("Invalid SUUNTO_API_CALLS_PER_MINUTE")?;
        ensure!(
            rate.is_finite(),
            "SUUNTO_API_CALLS_PER_MINUTE must be finite"
        );
        let weekly_limit = get("API_WEEKLY_CALL_LIMIT", "200")
            .parse()
            .context("Invalid SUUNTO_API_WEEKLY_CALL_LIMIT")?;
        let max_base64_bytes = get("MAX_BASE64_BYTES", "5000000")
            .parse()
            .context("Invalid SUUNTO_MAX_BASE64_BYTES")?;
        let write_tools = boolean(&get("ENABLE_WRITE_TOOLS", "false"), "ENABLE_WRITE_TOOLS")?;
        let live_tests = boolean(&get("LIVE_TESTS", "false"), "LIVE_TESTS")?;
        let max_sessions = get("MCP_MAX_SESSIONS", "128")
            .parse()
            .context("Invalid SUUNTO_MCP_MAX_SESSIONS")?;
        let ttl = get("MCP_SESSION_IDLE_SECONDS", "900")
            .parse()
            .context("Invalid SUUNTO_MCP_SESSION_IDLE_SECONDS")?;
        ensure!(
            (1..=1024).contains(&max_sessions),
            "SUUNTO_MCP_MAX_SESSIONS must be between 1 and 1024"
        );
        ensure!(
            (1..=86400).contains(&ttl),
            "SUUNTO_MCP_SESSION_IDLE_SECONDS must be between 1 and 86400"
        );
        let display = json!({
            "CLIENT_ID": redact(&client_id), "CLIENT_SECRET": redact(&client_secret),
            "REDIRECT_URI": redact(&redirect_uri), "SUBSCRIPTION_KEY": redact(&subscription_key),
            "MCP_API_TOKEN": redact(&bearer), "WEBHOOK_SECRET": redact(&webhook_secret),
            "API_BASE": api_base, "OAUTH_BASE": oauth_base_url,
            "API_CALLS_PER_MINUTE": rate, "API_WEEKLY_CALL_LIMIT": weekly_limit, "API_QUOTA_FILE": quota,
            "TOKEN_STORE": token_store, "TOKEN_FILE": tokens, "DEFAULT_ACCOUNT_ID": default_account,
            "LOCAL_DATA_DIR": data, "EXPORT_DIR": exports, "MAX_BASE64_BYTES": max_base64_bytes,
            "WEBHOOK_STORE": webhook_store, "WEBHOOK_STORE_PATH": webhooks,
            "ENABLE_WRITE_TOOLS": write_tools, "LIVE_TESTS": live_tests,
        });
        Ok(Self {
            client_id,
            client_secret,
            redirect_uri,
            subscription_key,
            api_base: api_url,
            oauth_base_url: parsed_oauth_url,
            rate,
            weekly_limit,
            quota_file,
            token_store,
            token_file,
            default_account,
            data_dir,
            export_dir,
            max_base64_bytes,
            bearer,
            webhook_secret,
            webhook_store,
            webhook_file,
            write_tools,
            max_sessions,
            session_ttl: Duration::from_secs(ttl),
            display,
        })
    }

    pub fn redacted(&self) -> &Value {
        &self.display
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricts_suunto_endpoints_to_https_and_preserves_base_path() {
        for setting in ["SUUNTO_OAUTH_BASE", "SUUNTO_API_BASE"] {
            for url in [
                "http://example.net",
                "https://name:password@example.net",
                "https://example.net?key=value",
                "https://example.net#fragment",
            ] {
                assert!(Config::from_lookup(|key| (key == setting).then(|| url.into())).is_err());
            }
        }
        let config = Config::from_lookup(|key| {
            matches!(key, "SUUNTO_OAUTH_BASE" | "SUUNTO_API_BASE")
                .then(|| "https://example.net/suunto".into())
        })
        .unwrap();
        assert_eq!(
            config.oauth_url("oauth/token").unwrap().as_str(),
            "https://example.net/suunto/oauth/token"
        );
        assert_eq!(
            config.api_endpoint("/v2/route").unwrap().as_str(),
            "https://example.net/suunto/v2/route"
        );
    }
}
