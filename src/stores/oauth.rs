use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail, ensure};
use base64::{
    Engine,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{now, read_json, text, timestamp, write_json};
use crate::App;

const KEYRING_SERVICE: &str = "suunto-mcp";
const KEYRING_ACCOUNT: &str = "tokens";
const MAX_OAUTH_RESPONSE: usize = 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct TokenRecord {
    pub account_id: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub username: Option<String>,
    pub label: Option<String>,
    pub expires_at: Option<f64>,
    pub scope: Option<String>,
    #[serde(default = "bearer_type")]
    pub token_type: String,
    #[serde(default = "timestamp")]
    pub created_at: String,
    #[serde(default = "timestamp")]
    pub updated_at: String,
}

fn bearer_type() -> String {
    "bearer".into()
}

impl TokenRecord {
    fn safe(&self) -> Value {
        json!({
            "account_id": self.account_id, "username": self.username,
            "label": self.label, "expires_at": self.expires_at, "scope": self.scope,
            "token_type": self.token_type,
            "created_at": self.created_at.replace('Z', "+00:00"),
            "updated_at": self.updated_at.replace('Z', "+00:00"),
            "has_access_token": !self.access_token.is_empty(),
            "has_refresh_token": self.refresh_token.as_ref().is_some_and(|token| !token.is_empty()),
        })
    }

    fn refresh_token(&self) -> Option<&str> {
        self.refresh_token
            .as_deref()
            .filter(|token| !token.is_empty())
    }
}

#[derive(Default, Deserialize, Serialize)]
struct Accounts {
    accounts: BTreeMap<String, TokenRecord>,
}

enum Backend {
    Memory(Accounts),
    File(PathBuf),
    Sqlite(PathBuf),
    Keyring,
}

impl Backend {
    fn load(&self) -> Result<Accounts> {
        match self {
            Backend::Memory(_) => unreachable!("memory accounts are accessed in place"),
            Backend::File(path) => read_json(path),
            Backend::Sqlite(path) => load_sqlite(&open_sqlite(path)?),
            Backend::Keyring => match read_keyring()? {
                Some(value) => {
                    serde_json::from_str(&value).context("Invalid keyring token payload")
                }
                None => Ok(Accounts::default()),
            },
        }
    }

    fn store(&self, accounts: &Accounts) -> Result<()> {
        match self {
            Backend::Memory(_) => unreachable!("memory accounts are accessed in place"),
            Backend::File(path) => write_json(path, accounts),
            Backend::Sqlite(_) => unreachable!("SQLite updates use a transaction"),
            Backend::Keyring => write_keyring(&serde_json::to_string(accounts)?),
        }
    }
}

pub struct Tokens {
    backend: Arc<Mutex<Backend>>,
    mutation: tokio::sync::Mutex<()>,
}

impl Tokens {
    pub fn new(backend: &str, path: PathBuf) -> Result<Self> {
        let backend = match backend {
            "file" => Backend::File(path),
            "sqlite" => Backend::Sqlite(path),
            "memory" => Backend::Memory(Accounts::default()),
            "keyring" => Backend::Keyring,
            _ => bail!("Unsupported token store: {backend}"),
        };
        Ok(Self {
            backend: Arc::new(Mutex::new(backend)),
            mutation: tokio::sync::Mutex::new(()),
        })
    }

    async fn with_accounts<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&Accounts) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || {
            let backend = backend.lock().expect("tokens lock poisoned");
            match &*backend {
                Backend::Memory(accounts) => operation(accounts),
                backend => operation(&backend.load()?),
            }
        })
        .await?
    }

    async fn change_accounts<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Accounts) -> Result<(T, bool)> + Send + 'static,
    ) -> Result<T> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || {
            let mut backend = backend.lock().expect("tokens lock poisoned");
            match &mut *backend {
                Backend::Memory(accounts) => Ok(operation(accounts)?.0),
                Backend::Sqlite(path) => {
                    let mut db = open_sqlite(path)?;
                    let txn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let mut accounts = load_sqlite(&txn)?;
                    let (result, changed) = operation(&mut accounts)?;
                    if changed {
                        txn.execute(
                            "INSERT OR REPLACE INTO token_store (id, accounts_json) VALUES (1, ?1)",
                            [serde_json::to_string(&accounts)?],
                        )?;
                    }
                    txn.commit()?;
                    Ok(result)
                }
                backend => {
                    let mut accounts = backend.load()?;
                    let (result, changed) = operation(&mut accounts)?;
                    if changed {
                        backend.store(&accounts)?;
                    }
                    Ok(result)
                }
            }
        })
        .await?
    }

    async fn all(&self) -> Result<Vec<TokenRecord>> {
        self.with_accounts(|accounts| Ok(accounts.accounts.values().cloned().collect()))
            .await
    }

    async fn resolve(&self, id: Option<&str>, default: &str) -> Result<TokenRecord> {
        let id = id.map(str::to_owned);
        let default = default.to_owned();
        self.with_accounts(move |accounts| {
            if let Some(id) = id.filter(|id| !id.is_empty()) {
                return accounts
                    .accounts
                    .get(&id)
                    .cloned()
                    .with_context(|| format!("No Suunto account stored for account_id={id:?}."));
            }
            if !default.is_empty() {
                return accounts
                    .accounts
                    .get(&default)
                    .cloned()
                    .with_context(|| format!("Default account_id={default:?} is not stored."));
            }
            match accounts.accounts.len() {
                0 => bail!("No Suunto accounts are stored. Authorize or import tokens first."),
                1 => Ok(accounts.accounts.values().next().unwrap().clone()),
                _ => bail!(
                    "Multiple accounts are stored ({}); pass account_id explicitly.",
                    accounts
                        .accounts
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }
        })
        .await
    }

    async fn save_response(
        &self,
        data: Value,
        id: Option<&str>,
        label: Option<&str>,
        expiry_override: Option<f64>,
    ) -> Result<TokenRecord> {
        let access_token =
            text(&data, "access_token").context("OAuth response did not contain access_token.")?;
        let jwt = decode_jwt(access_token);
        let account_id = id
            .filter(|id| !id.is_empty())
            .or_else(|| {
                ["user", "username", "sub"]
                    .iter()
                    .find_map(|key| text(&jwt, key))
            })
            .context("Could not infer account_id from token; provide account_id explicitly.")?;
        let expires_at = expiry_override
            .or_else(|| data["expires_in"].as_f64().map(|seconds| now() + seconds))
            .or_else(|| jwt["exp"].as_f64());
        let mut record = TokenRecord {
            account_id: account_id.into(),
            access_token: access_token.into(),
            refresh_token: text(&data, "refresh_token").map(str::to_owned),
            username: text(&jwt, "user")
                .or_else(|| text(&jwt, "username"))
                .map(str::to_owned),
            label: label.map(str::to_owned),
            expires_at,
            scope: data["scope"].as_str().map(str::to_owned),
            token_type: text(&data, "token_type").map_or_else(bearer_type, str::to_owned),
            created_at: timestamp(),
            updated_at: timestamp(),
        };
        self.change_accounts(move |accounts| {
            if let Some(existing) = accounts.accounts.get(&record.account_id) {
                record.created_at.clone_from(&existing.created_at);
                if record.label.is_none() {
                    record.label.clone_from(&existing.label);
                }
                if record.refresh_token.is_none() {
                    record.refresh_token.clone_from(&existing.refresh_token);
                }
            }
            accounts
                .accounts
                .insert(record.account_id.clone(), record.clone());
            Ok((record, true))
        })
        .await
    }

    async fn delete(&self, id: &str) -> Result<bool> {
        let id = id.to_owned();
        self.change_accounts(move |accounts| {
            let removed = accounts.accounts.remove(&id).is_some();
            Ok((removed, removed))
        })
        .await
    }
}

fn open_sqlite(path: &std::path::Path) -> Result<Connection> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS token_store (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            accounts_json TEXT NOT NULL
        )",
    )?;
    Ok(db)
}

fn load_sqlite(db: &Connection) -> Result<Accounts> {
    let data: Option<String> = db
        .query_row(
            "SELECT accounts_json FROM token_store WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match data {
        Some(value) => Ok(serde_json::from_str(&value)?),
        None => Ok(Accounts::default()),
    }
}

#[cfg(not(windows))]
fn read_keyring() -> Result<Option<String>> {
    read_password(&keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)?)
}

#[cfg(not(windows))]
fn write_keyring(value: &str) -> Result<()> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)?
        .set_password(value)
        .context("Could not write to system keyring")
}

#[cfg(windows)]
fn windows_entry() -> Result<keyring::Entry> {
    let service_entry =
        keyring::Entry::new_with_target(KEYRING_SERVICE, KEYRING_SERVICE, KEYRING_ACCOUNT)?;
    match service_entry.get_attributes() {
        Ok(attributes)
            if attributes
                .get("username")
                .is_some_and(|user| user == KEYRING_ACCOUNT) =>
        {
            Ok(service_entry)
        }
        Err(keyring::Error::NoEntry) => {
            let compound = keyring::Entry::new_with_target(
                &format!("{KEYRING_ACCOUNT}@{KEYRING_SERVICE}"),
                KEYRING_SERVICE,
                KEYRING_ACCOUNT,
            )?;
            match compound.get_attributes() {
                Ok(_) => Ok(compound),
                Err(keyring::Error::NoEntry) => Ok(service_entry),
                Err(error) => Err(error.into()),
            }
        }
        Ok(_) => Ok(keyring::Entry::new_with_target(
            &format!("{KEYRING_ACCOUNT}@{KEYRING_SERVICE}"),
            KEYRING_SERVICE,
            KEYRING_ACCOUNT,
        )?),
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
fn read_keyring() -> Result<Option<String>> {
    read_password(&windows_entry()?)
}

#[cfg(windows)]
fn write_keyring(value: &str) -> Result<()> {
    windows_entry()?
        .set_password(value)
        .context("Could not write to system keyring")
}

fn read_password(entry: &keyring::Entry) -> Result<Option<String>> {
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error).context("Could not read from system keyring"),
    }
}

fn decode_jwt(token: &str) -> Value {
    token
        .split('.')
        .nth(1)
        .and_then(|encoded| {
            URL_SAFE_NO_PAD
                .decode(encoded)
                .or_else(|_| URL_SAFE.decode(encoded))
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

async fn token_request(app: &App, form: &[(&str, &str)]) -> Result<Value> {
    let config = &app.config;
    ensure!(
        !config.client_id.is_empty() && !config.client_secret.is_empty(),
        "SUUNTO_CLIENT_ID and SUUNTO_CLIENT_SECRET are required."
    );
    let mut response = app
        .http
        .post(config.oauth_url("oauth/token")?)
        .basic_auth(&config.client_id, Some(&config.client_secret))
        .form(form)
        .send()
        .await
        .map_err(reqwest::Error::without_url)?;
    ensure!(
        response.status().is_success(),
        "Suunto OAuth API returned HTTP {}.",
        response.status().as_u16()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(reqwest::Error::without_url)?
    {
        ensure!(
            bytes.len() + chunk.len() <= MAX_OAUTH_RESPONSE,
            "Suunto OAuth response is too large."
        );
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).context("Could not decode Suunto OAuth response")
}

async fn refresh(app: &App, record: &TokenRecord) -> Result<TokenRecord> {
    let refresh_token = record
        .refresh_token()
        .context("Stored access token has no refresh token.")?;
    let response = token_request(
        app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await?;
    app.tokens
        .save_response(response, Some(&record.account_id), None, None)
        .await
}

fn needs_refresh(record: &TokenRecord, rejected: Option<&str>) -> bool {
    record
        .expires_at
        .is_some_and(|expiry| expiry <= now() + 60.0)
        || rejected == Some(record.access_token.as_str())
}

impl App {
    pub async fn record(
        &self,
        account: Option<&str>,
        rejected: Option<&str>,
    ) -> Result<TokenRecord> {
        let stored = self
            .tokens
            .resolve(account, &self.config.default_account)
            .await?;
        if !needs_refresh(&stored, rejected) {
            return Ok(stored);
        }

        // Serialize refreshes and re-read the account so concurrent requests use the rotated token.
        let _guard = self.tokens.mutation.lock().await;
        let current = self.tokens.resolve(Some(&stored.account_id), "").await?;
        if !needs_refresh(&current, rejected) {
            return Ok(current);
        }

        refresh(self, &current).await
    }
}

fn authorization_url(app: &App, state: Option<&str>) -> Result<Value> {
    let config = &app.config;
    ensure!(
        !config.client_id.is_empty(),
        "SUUNTO_CLIENT_ID is required."
    );
    ensure!(
        !config.redirect_uri.is_empty(),
        "SUUNTO_REDIRECT_URI is required."
    );
    let mut url = config.oauth_url("oauth/authorize")?;
    let generated_state = uuid::Uuid::new_v4().simple().to_string();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", &config.redirect_uri)
        .append_pair("state", state.unwrap_or(&generated_state));
    Ok(json!({
        "authorization_url": url.as_str(), "redirect_uri": config.redirect_uri,
        "next_step": "Open authorization_url, authorize with a Suunto App account, then exchange the returned code."
    }))
}

pub(super) async fn call(app: &App, name: &str, args: &Value) -> Result<Value> {
    let id = text(args, "account_id");
    let label = args["label"].as_str();
    let default = &app.config.default_account;
    let tokens = &app.tokens;
    match name {
        "suunto_get_authorization_url" => authorization_url(app, text(args, "state")),
        "suunto_exchange_authorization_code" => {
            ensure!(
                !app.config.redirect_uri.is_empty(),
                "SUUNTO_REDIRECT_URI is required."
            );
            let code = args["code"].as_str().context("code is required.")?;
            let response = token_request(
                app,
                &[
                    ("grant_type", "authorization_code"),
                    ("redirect_uri", &app.config.redirect_uri),
                    ("code", code),
                ],
            )
            .await?;
            let _guard = tokens.mutation.lock().await;
            let record = tokens.save_response(response, id, label, None).await?;
            Ok(json!({"stored": true, "account": record.safe()}))
        }
        "suunto_import_token_set" => {
            let _guard = tokens.mutation.lock().await;
            let record = tokens
                .save_response(args.clone(), id, label, args["expires_at"].as_f64())
                .await?;
            Ok(json!({"stored": true, "account": record.safe()}))
        }
        "suunto_import_token_file" => {
            let path =
                crate::config::expand_path(args["path"].as_str().context("path is required.")?);
            let response: Value = tokio::task::spawn_blocking(move || {
                let file = std::fs::File::open(path)?;
                let data: Value = serde_json::from_reader(std::io::BufReader::new(file))?;
                ensure!(data.is_object(), "Token file must contain a JSON object.");
                Ok::<_, anyhow::Error>(data)
            })
            .await??;
            let _guard = tokens.mutation.lock().await;
            let record = tokens.save_response(response, id, label, None).await?;
            Ok(json!({"stored": true, "account": record.safe()}))
        }
        "suunto_refresh_token" => {
            let _guard = tokens.mutation.lock().await;
            let records = if id.is_some() {
                vec![tokens.resolve(id, default).await?]
            } else {
                tokens.all().await?
            };
            let mut refreshed = Vec::new();
            let mut skipped = Vec::new();
            for record in records {
                if record.refresh_token().is_some() {
                    refreshed.push(refresh(app, &record).await?.safe());
                } else {
                    skipped.push(record.account_id);
                }
            }
            Ok(json!({"refreshed": refreshed, "skipped_without_refresh_token": skipped}))
        }
        "suunto_list_accounts" => {
            let accounts: Vec<_> = tokens.all().await?.iter().map(TokenRecord::safe).collect();
            Ok(json!({"accounts": accounts}))
        }
        "suunto_auth_status" => {
            let records = if id.is_some() {
                vec![tokens.resolve(id, default).await?]
            } else {
                tokens.all().await?
            };
            let now = now();
            let accounts: Vec<_> = records
                .iter()
                .map(|record| {
                    let mut safe = record.safe();
                    safe["is_expired"] =
                        json!(record.expires_at.is_some_and(|expiry| expiry <= now));
                    safe["seconds_until_expiry"] =
                        json!(record.expires_at.map(|expiry| (expiry - now) as i64));
                    safe
                })
                .collect();
            Ok(json!({"accounts": accounts}))
        }
        "suunto_deauthorize" => {
            let _guard = tokens.mutation.lock().await;
            let record = tokens.resolve(id, default).await?;
            let status = app
                .http
                .get(app.config.oauth_url("oauth/deauthorize")?)
                .bearer_auth(&record.access_token)
                .send()
                .await
                .map_err(reqwest::Error::without_url)?
                .status();
            if status.is_client_error() || status.is_server_error() {
                return Ok(json!({
                    "deauthorized": false, "status_code": status.as_u16(),
                    "note": "Suunto API returned an error during deauthorization.", "local_removed": false,
                }));
            }
            let removed = args["remove_local"].as_bool().unwrap_or(true)
                && tokens.delete(&record.account_id).await?;
            Ok(
                json!({"deauthorized": true, "status_code": status.as_u16(), "local_removed": removed}),
            )
        }
        _ => bail!("Unknown OAuth tool: {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use axum::{Form, Router, extract::State, http::HeaderMap, response::Json, routing::post};
    use std::{
        collections::HashMap,
        sync::atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn python_token_file_round_trips_and_is_private() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("tokens.json");
        write_json(
            &path,
            &json!({"accounts": {"runner": {
                "account_id": "runner", "username": "runner", "label": "Original label",
                "access_token": "old-access-secret", "refresh_token": "old-refresh-secret",
                "expires_at": 123456.0, "scope": null, "token_type": "Bearer",
                "created_at": "2026-06-03T12:00:00Z", "updated_at": "2026-06-03T12:00:00Z"
            }}}),
        )?;

        let store = Tokens::new("file", path.clone())?;
        let saved = store
            .save_response(
                json!({"access_token": "new-access-secret"}),
                Some("runner"),
                None,
                None,
            )
            .await?;
        assert_eq!(saved.refresh_token(), Some("old-refresh-secret"));
        assert_eq!(saved.created_at, "2026-06-03T12:00:00Z");
        assert_eq!(saved.safe()["label"], "Original label");
        assert!(!saved.safe().to_string().contains("-secret"));

        let reopened = Tokens::new("file", path.clone())?;
        let stored = reopened.resolve(None, "").await?;
        assert_eq!(stored.access_token, "new-access-secret");
        assert_eq!(stored.refresh_token(), Some("old-refresh-secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path)?.permissions().mode() & 0o777,
                0o600
            );
        }

        assert!(reopened.delete("runner").await?);
        assert!(!reopened.delete("runner").await?);
        assert!(store.all().await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn sqlite_persists_accounts_and_many_accounts_require_selection() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("tokens.sqlite3");
        let left = Tokens::new("sqlite", path.clone())?;
        let right = Tokens::new("sqlite", path.clone())?;
        let (first, second) = tokio::join!(
            left.save_response(
                json!({"access_token": "secret-one"}),
                Some("one"),
                None,
                None
            ),
            right.save_response(
                json!({"access_token": "secret-two"}),
                Some("two"),
                None,
                None
            ),
        );
        first?;
        second?;

        let reopened = Tokens::new("sqlite", path)?;
        assert!(reopened.resolve(None, "").await.is_err());
        assert_eq!(
            reopened.resolve(None, "two").await?.access_token,
            "secret-two"
        );
        assert_eq!(
            reopened.resolve(Some("one"), "two").await?.access_token,
            "secret-one"
        );
        Ok(())
    }

    #[tokio::test]
    async fn mock_oauth_exchanges_and_refreshes_once_after_concurrent_401s() -> Result<()> {
        async fn token_endpoint(
            State(refreshes): State<Arc<AtomicUsize>>,
            headers: HeaderMap,
            Form(form): Form<HashMap<String, String>>,
        ) -> Json<Value> {
            let credentials =
                base64::engine::general_purpose::STANDARD.encode("known-id:known-secret");
            assert_eq!(
                headers["authorization"],
                format!("Basic {credentials}").as_str()
            );
            if form["grant_type"] == "authorization_code" {
                assert_eq!(form["code"], "known-code");
                assert_eq!(form["redirect_uri"], "http://localhost:8001/callback");
                let jwt = URL_SAFE_NO_PAD.encode(br#"{"user":"runner42","exp":12345}"#);
                Json(
                    json!({"access_token": format!("header.{jwt}."), "refresh_token": "refresh-secret", "expires_in": 3600}),
                )
            } else {
                assert_eq!(form["grant_type"], "refresh_token");
                assert_eq!(form["refresh_token"], "refresh-secret");
                refreshes.fetch_add(1, Ordering::SeqCst);
                Json(json!({"access_token": "new-secret", "expires_in": 3600}))
            }
        }

        let refreshes = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route("/oauth/token", post(token_endpoint))
            .with_state(refreshes.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let app = App::new(Config::from_lookup(|key| {
            Some(match key {
                "SUUNTO_TOKEN_STORE" => "memory".into(),
                "SUUNTO_WEBHOOK_STORE" => "memory".into(),
                "SUUNTO_OAUTH_BASE" => base.clone(),
                "SUUNTO_CLIENT_ID" => "known-id".into(),
                "SUUNTO_CLIENT_SECRET" => "known-secret".into(),
                "SUUNTO_REDIRECT_URI" => "http://localhost:8001/callback".into(),
                _ => return None,
            })
        })?)?;

        let auth = call(
            &app,
            "suunto_get_authorization_url",
            &json!({"state": "state with space"}),
        )
        .await?;
        let url = reqwest::Url::parse(auth["authorization_url"].as_str().unwrap())?;
        let query: HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(query["state"], "state with space");

        let result = call(
            &app,
            "suunto_exchange_authorization_code",
            &json!({"code": "known-code"}),
        )
        .await?;
        assert_eq!(result["account"]["account_id"], "runner42");
        assert!(result["account"]["expires_at"].as_f64().unwrap() > now());
        assert!(!result.to_string().contains("refresh-secret"));

        let original = app.record(None, None).await?;
        let (first, second) = tokio::join!(
            app.record(None, Some(&original.access_token)),
            app.record(None, Some(&original.access_token)),
        );
        assert_eq!(first?.access_token, "new-secret");
        assert_eq!(second?.access_token, "new-secret");
        assert_eq!(refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(
            app.record(None, None).await?.refresh_token(),
            Some("refresh-secret")
        );
        server.abort();
        Ok(())
    }
}
