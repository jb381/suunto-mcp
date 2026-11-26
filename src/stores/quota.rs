use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{now, read_json, write_json};

const WINDOW: i64 = 7 * 24 * 60 * 60;

#[derive(Serialize, Deserialize)]
struct Ledger {
    #[serde(default = "window")]
    window_seconds: i64,
    calls: Vec<Value>,
}

fn window() -> i64 {
    WINDOW
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            window_seconds: WINDOW,
            calls: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct Quota {
    path: PathBuf,
    limit: i64,
    lock: Arc<Mutex<()>>,
}

impl Quota {
    pub fn new(path: PathBuf, limit: i64) -> Self {
        Self {
            path,
            limit,
            lock: Arc::new(Mutex::new(())),
        }
    }

    fn recent(&self, now: f64) -> Result<Ledger> {
        let mut ledger: Ledger = read_json(&self.path)?;
        ledger.window_seconds = WINDOW;
        ledger
            .calls
            .retain(|call| call_timestamp(call) >= now - WINDOW as f64);
        Ok(ledger)
    }

    pub fn record(&self, method: &str, path: &str, account: Option<&str>) -> Result<()> {
        if self.limit <= 0 {
            return Ok(());
        }

        let _guard = self.lock.lock().expect("quota lock poisoned");
        let now = now();
        let mut ledger = self.recent(now)?;
        if ledger.calls.len() >= self.limit as usize {
            let used = ledger.calls.len();
            let resets_at = reset_at(&ledger.calls).expect("quota includes at least one call");
            bail!(
                "Suunto API weekly call limit would be exceeded ({used}/{}). Next call is available after {resets_at}.",
                self.limit
            );
        }
        ledger
            .calls
            .push(json!({"timestamp": now, "method": method, "path": path, "account_id": account}));
        write_json(&self.path, &ledger)
    }

    pub fn status(&self) -> Result<Value> {
        if self.limit <= 0 {
            return Ok(json!({
                "enabled": false, "limit": self.limit, "used": null,
                "remaining": null, "path": self.path,
            }));
        }

        let _guard = self.lock.lock().expect("quota lock poisoned");
        let ledger = self.recent(now())?;
        let used = ledger.calls.len() as i64;
        Ok(json!({
            "enabled": true, "limit": self.limit, "used": used,
            "remaining": (self.limit - used).max(0), "reset_at": reset_at(&ledger.calls),
            "path": self.path,
        }))
    }
}

fn call_timestamp(call: &Value) -> f64 {
    let value = &call["timestamp"];
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .unwrap_or(0.0)
}

fn reset_at(calls: &[Value]) -> Option<i64> {
    calls
        .iter()
        .map(call_timestamp)
        .reduce(f64::min)
        .map(|oldest| (oldest + WINDOW as f64) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::Arc};

    #[test]
    fn python_ledger_is_preserved_and_expired_calls_are_pruned() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("api-quota.json");
        write_json(
            &path,
            &json!({"window_seconds": WINDOW, "calls": [
                {"timestamp": 1, "method": "GET", "path": "/expired", "account_id": null},
                {"timestamp": now().to_string(), "method": "GET", "path": "/legacy", "account_id": "runner"}
            ]}),
        )?;
        let quota = Quota::new(path.clone(), 2);
        assert_eq!(quota.status()?["remaining"], 1);
        quota.record("GET", "/v3/workouts", Some("runner"))?;
        assert!(quota.record("GET", "/v3/workouts", Some("runner")).is_err());
        let ledger: Value = serde_json::from_slice(&fs::read(path)?)?;
        assert_eq!(ledger["calls"][0]["path"], "/legacy");
        assert_eq!(ledger["calls"][1]["path"], "/v3/workouts");
        assert_eq!(quota.status()?["remaining"], 0);
        Ok(())
    }

    #[test]
    fn concurrent_requests_share_quota() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let quota = Arc::new(Quota::new(directory.path().join("api-quota.json"), 4));
        let tasks: Vec<_> = (0..16)
            .map(|_| {
                let quota = quota.clone();
                std::thread::spawn(move || quota.record("GET", "/v3/workouts", None).is_ok())
            })
            .collect();
        let successes: usize = tasks
            .into_iter()
            .map(|task| usize::from(task.join().unwrap()))
            .sum();
        assert_eq!(successes, 4);
        assert_eq!(quota.status()?["used"], 4);
        Ok(())
    }

    #[test]
    fn disabled_quota_leaves_disk_untouched() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("quota.json");
        let quota = Quota::new(path.clone(), 0);
        quota.record("GET", "/v3/workouts", None)?;
        assert_eq!(quota.status()?["enabled"], false);
        assert_eq!(quota.status()?["remaining"], Value::Null);
        assert!(!path.exists());
        Ok(())
    }
}
