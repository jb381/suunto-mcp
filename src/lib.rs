mod cloud;
pub mod config;
pub mod local;
pub mod mcp;
pub mod stores;

use anyhow::Result;
use config::Config;
use std::{sync::Mutex, time::Duration};
use stores::{Quota, Tokens, Webhooks};

pub struct App {
    pub config: Config,
    pub http: reqwest::Client,
    pub tokens: Tokens,
    pub quota: Quota,
    pub webhooks: Webhooks,
    pub imports_lock: Mutex<()>,
    pub api_rate: tokio::sync::Mutex<Option<tokio::time::Instant>>,
}

impl App {
    pub fn new(config: Config) -> Result<Self> {
        let tokens = Tokens::new(&config.token_store, config.token_file.clone())?;
        let quota = Quota::new(config.quota_file.clone(), config.weekly_limit);
        let webhooks = Webhooks::new(&config.webhook_store, config.webhook_file.clone())?;
        let http = reqwest::Client::builder()
            .https_only(!cfg!(test))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            config,
            http,
            tokens,
            quota,
            webhooks,
            imports_lock: Mutex::new(()),
            api_rate: tokio::sync::Mutex::new(None),
        })
    }
}
