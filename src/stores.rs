mod oauth;
mod quota;
mod webhooks;

use std::{
    fs::{self, File},
    io::{BufReader, Write},
    path::Path,
};

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tempfile::NamedTempFile;

use crate::App;
pub use oauth::{TokenRecord, Tokens};
pub use quota::Quota;
pub use webhooks::{WEBHOOK_ENDPOINTS, Webhooks, receive_webhook};

pub async fn call(app: &App, name: &str, args: &Value) -> Result<Value> {
    match name {
        "suunto_api_quota_status" => {
            let quota = app.quota.clone();
            tokio::task::spawn_blocking(move || quota.status()).await?
        }
        "suunto_verify_webhook_signature"
        | "suunto_store_webhook_event"
        | "suunto_list_webhook_events"
        | "suunto_get_webhook_event"
        | "suunto_delete_webhook_event"
        | "suunto_suggest_webhook_followups" => webhooks::call(app, name, args).await,
        _ => oauth::call(app, name, args).await,
    }
}

pub(super) fn read_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match File::open(path) {
        Ok(file) => serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("Could not parse {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error).with_context(|| format!("Could not open {}", path.display())),
    }
}

// NamedTempFile creates mode 0600 on Unix. Persist only after the complete file is written.
pub(super) fn atomic_write(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut file = NamedTempFile::new_in(parent)?;
    write(file.as_file_mut())?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path)
        .with_context(|| format!("Could not replace {}", path.display()))?;
    Ok(())
}

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    atomic_write(path, |file| {
        serde_json::to_writer_pretty(&mut *file, value)?;
        Ok(())
    })
}

pub(super) fn text<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args[name].as_str().filter(|s| !s.is_empty())
}

pub(super) fn now() -> f64 {
    chrono::Utc::now().timestamp_micros() as f64 / 1_000_000.0
}

pub(super) fn timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
}
