use std::{
    collections::{HashMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail, ensure};
use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::Sha256;
use uuid::Uuid;

use super::{atomic_write, text, timestamp};
use crate::App;

pub const WEBHOOK_ENDPOINTS: [(&str, &str); 7] = [
    ("/webhooks/suunto", "suunto-json"),
    ("/webhooks/suunto/workout", "suunto-workout"),
    ("/webhooks/suunto/route", "suunto-route"),
    ("/webhooks/suunto/247/activity", "suunto-247-activity"),
    ("/webhooks/suunto/247/sleep", "suunto-247-sleep"),
    ("/webhooks/suunto/247/recovery", "suunto-247-recovery"),
    ("/webhooks/suunto/legacy-workout", "suunto-legacy-workout"),
];

const MAX_MEMORY_EVENTS: usize = 10_000;
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_BUFFER_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct Webhooks {
    backend: Arc<Mutex<Backend>>,
}

enum Backend {
    Memory(BoundedEvents),
    Jsonl(PathBuf),
    Sqlite(PathBuf),
}

#[derive(Default)]
struct BoundedEvents {
    encoded: VecDeque<String>,
    encoded_bytes: usize,
}

impl BoundedEvents {
    fn push(&mut self, event: String, limit: usize) {
        while !self.encoded.is_empty()
            && (self.encoded.len() >= limit || self.encoded_bytes + event.len() > MAX_BUFFER_BYTES)
        {
            self.encoded_bytes -= self.encoded.pop_front().unwrap().len();
        }
        self.encoded_bytes += event.len();
        self.encoded.push_back(event);
    }

    fn newest(&self, limit: usize) -> Result<Vec<Value>> {
        self.encoded
            .iter()
            .rev()
            .take(limit)
            .map(|encoded| decode_event(encoded))
            .collect()
    }
}

impl Webhooks {
    pub fn new(backend: &str, path: PathBuf) -> Result<Self> {
        let backend = match backend {
            "memory" => Backend::Memory(BoundedEvents::default()),
            "jsonl" => Backend::Jsonl(path),
            "sqlite" => Backend::Sqlite(path),
            _ => bail!("Unsupported webhook store: {backend}"),
        };
        Ok(Self {
            backend: Arc::new(Mutex::new(backend)),
        })
    }

    fn append(&self, mut event: Value) -> Result<Value> {
        let fields = event
            .as_object_mut()
            .context("Webhook event must be an object.")?;
        fields
            .entry("id")
            .or_insert_with(|| json!(Uuid::new_v4().to_string()));
        fields
            .entry("received_at")
            .or_insert_with(|| json!(timestamp()));
        let id = text(&event, "id").context("Webhook event id must be a nonempty string.")?;
        let received_at =
            text(&event, "received_at").context("Webhook event received_at must be a string.")?;
        let encoded = serde_json::to_string(&event)?;
        ensure!(
            encoded.len() <= MAX_EVENT_BYTES,
            "Webhook event is too large."
        );

        let mut backend = self.backend.lock().expect("webhook store lock poisoned");
        match &mut *backend {
            Backend::Memory(events) => events.push(encoded, MAX_MEMORY_EVENTS),
            Backend::Jsonl(path) => {
                create_parent(path)?;
                let mut line = encoded;
                line.push('\n');
                let mut options = OpenOptions::new();
                options.create(true).append(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                options.open(path)?.write_all(line.as_bytes())?;
            }
            Backend::Sqlite(path) => {
                let username = event["payload"]["username"]
                    .as_str()
                    .or_else(|| event["username"].as_str());
                connect(path)?.execute(
                    "INSERT OR REPLACE INTO webhook_events (id, received_at, kind, username, payload_json)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![id, received_at, event["kind"].as_str(), username, encoded],
                )?;
            }
        }
        Ok(event)
    }

    fn list(&self, limit: i64) -> Result<Vec<Value>> {
        let backend = self.backend.lock().expect("webhook store lock poisoned");
        match &*backend {
            Backend::Memory(events) => {
                let limit = tail_limit(limit, events.encoded.len());
                events.newest(limit)
            }
            Backend::Jsonl(path) => {
                let mut events = BoundedEvents::default();
                let Some(lines) = lines(path)? else {
                    return Ok(vec![]);
                };
                let skip = if limit < 0 {
                    limit.unsigned_abs() as usize
                } else {
                    0
                };
                let limit = if limit > 0 {
                    limit as usize
                } else {
                    usize::MAX
                };
                let mut count: usize = 0;
                for line in lines.skip(skip) {
                    let line = line?;
                    if !line.trim().is_empty() {
                        events.push(line, limit);
                        count += 1;
                    }
                }
                let requested = count.min(limit);
                ensure!(
                    events.encoded.len() == requested,
                    "Webhook event list is too large. Request a smaller limit."
                );
                events.newest(requested)
            }
            Backend::Sqlite(path) => {
                let db = connect(path)?;
                let mut statement = db.prepare(
                    "SELECT payload_json FROM webhook_events ORDER BY received_at DESC, rowid DESC LIMIT ?1",
                )?;
                let mut rows = statement.query([limit])?;
                let mut events = Vec::new();
                let mut encoded_bytes = 0;
                while let Some(row) = rows.next()? {
                    let encoded: &str = row.get_ref(0)?.as_str()?;
                    encoded_bytes += encoded.len();
                    ensure!(
                        encoded_bytes <= MAX_BUFFER_BYTES,
                        "Webhook event list is too large. Request a smaller limit."
                    );
                    events.push(decode_event(encoded)?);
                }
                Ok(events)
            }
        }
    }

    fn get(&self, event_id: &str) -> Result<Option<Value>> {
        let backend = self.backend.lock().expect("webhook store lock poisoned");
        match &*backend {
            Backend::Memory(events) => {
                for encoded in &events.encoded {
                    let event = decode_event(encoded)?;
                    if event["id"].as_str() == Some(event_id) {
                        return Ok(Some(event));
                    }
                }
                Ok(None)
            }
            Backend::Jsonl(path) => {
                let Some(lines) = lines(path)? else {
                    return Ok(None);
                };
                for line in lines {
                    let line = line?;
                    if line.trim().is_empty() {
                        continue;
                    }
                    let event = decode_event(&line)?;
                    if event["id"].as_str() == Some(event_id) {
                        return Ok(Some(event));
                    }
                }
                Ok(None)
            }
            Backend::Sqlite(path) => {
                let db = connect(path)?;
                let mut statement =
                    db.prepare("SELECT payload_json FROM webhook_events WHERE id = ?1")?;
                let mut rows = statement.query([event_id])?;
                let Some(row) = rows.next()? else {
                    return Ok(None);
                };
                Ok(Some(decode_event(row.get_ref(0)?.as_str()?)?))
            }
        }
    }

    fn delete(&self, event_id: &str) -> Result<bool> {
        let mut backend = self.backend.lock().expect("webhook store lock poisoned");
        match &mut *backend {
            Backend::Memory(events) => {
                for (position, encoded) in events.encoded.iter().enumerate() {
                    if decode_event(encoded)?["id"].as_str() == Some(event_id) {
                        events.encoded_bytes -= events.encoded.remove(position).unwrap().len();
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Backend::Sqlite(path) => Ok(connect(path)?
                .execute("DELETE FROM webhook_events WHERE id = ?1", [event_id])?
                > 0),
            Backend::Jsonl(path) => {
                let Some(lines) = lines(path)? else {
                    return Ok(false);
                };
                let mut removed = false;
                atomic_write(path, |file| {
                    let mut file = BufWriter::new(file);
                    for line in lines {
                        let line = line?;
                        if line.trim().is_empty() {
                            continue;
                        }
                        if decode_event(&line)?["id"].as_str() == Some(event_id) {
                            removed = true;
                        } else {
                            writeln!(file, "{line}")?;
                        }
                    }
                    file.flush()?;
                    Ok(())
                })?;
                Ok(removed)
            }
        }
    }
}

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn connect(path: &Path) -> Result<Connection> {
    create_parent(path)?;
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS webhook_events (
            id TEXT PRIMARY KEY,
            received_at TEXT NOT NULL,
            kind TEXT,
            username TEXT,
            payload_json TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_webhook_events_received_at ON webhook_events(received_at);
        CREATE INDEX IF NOT EXISTS idx_webhook_events_username ON webhook_events(username);",
    )?;
    Ok(db)
}

fn tail_limit(limit: i64, len: usize) -> usize {
    match limit {
        0 => len,
        ..0 => len.saturating_sub(limit.unsigned_abs() as usize),
        _ => limit as usize,
    }
}

struct BoundedLines(BufReader<File>);

impl Iterator for BoundedLines {
    type Item = io::Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = String::new();
        match (&mut self.0)
            .take((MAX_EVENT_BYTES + 2) as u64)
            .read_line(&mut line)
        {
            Ok(0) => None,
            Ok(_) => {
                if line.ends_with('\n') {
                    line.pop();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                }
                if line.len() > MAX_EVENT_BYTES {
                    return Some(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Stored webhook event is too large.",
                    )));
                }
                Some(Ok(line))
            }
            Err(error) => Some(Err(error)),
        }
    }
}

fn lines(path: &Path) -> Result<Option<BoundedLines>> {
    match File::open(path) {
        Ok(file) => Ok(Some(BoundedLines(BufReader::new(file)))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Could not open {}", path.display())),
    }
}

fn decode_event(encoded: &str) -> Result<Value> {
    ensure!(
        encoded.len() <= MAX_EVENT_BYTES,
        "Stored webhook event is too large."
    );
    let event: Value = serde_json::from_str(encoded)?;
    ensure!(
        event.is_object(),
        "Stored webhook payload is not a JSON object."
    );
    Ok(event)
}

fn verify_signature(body: &[u8], signature: &str, secret: &str) -> Result<bool> {
    ensure!(
        !secret.is_empty(),
        "Webhook secret is required for signature verification."
    );
    let signature = signature
        .strip_prefix("sha256=")
        .unwrap_or(signature)
        .trim();
    let mut decoded = [0_u8; 32];
    if signature.bytes().any(|byte| byte.is_ascii_uppercase())
        || hex::decode_to_slice(signature, &mut decoded).is_err()
    {
        return Ok(false);
    }

    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    Ok(mac.verify_slice(&decoded).is_ok())
}

fn classify(payload: &Value, fallback: &str) -> String {
    let kind = match payload["type"]
        .as_str()
        .map(|kind| kind.trim().to_ascii_uppercase())
        .as_deref()
    {
        Some("WORKOUT_CREATED") => "suunto-workout",
        Some("ROUTE_CREATED") => "suunto-route",
        Some("SUUNTO_247_ACTIVITY_CREATED") => "suunto-247-activity",
        Some("SUUNTO_247_SLEEP_CREATED") => "suunto-247-sleep",
        Some("SUUNTO_247_RECOVERY_CREATED") => "suunto-247-recovery",
        _ => fallback,
    };
    kind.to_owned()
}

fn followups(event: &Value) -> Value {
    let payload = if event["payload"].is_object() {
        &event["payload"]
    } else {
        event
    };
    let kind = classify(payload, text(event, "kind").unwrap_or("suunto-json"));
    let username = payload["username"].as_str();
    let mut args = json!({});
    if let Some(username) = username.filter(|username| !username.is_empty()) {
        args["account_id"] = json!(username);
    }

    let tools: &[&str] = match kind.as_str() {
        "suunto-workout" => {
            if let Some(id) = payload["workout"]["workoutKey"]
                .as_str()
                .or_else(|| payload["workoutid"].as_str())
                .filter(|id| !id.is_empty())
            {
                args["workout_id_or_key"] = json!(id);
                &["suunto_get_workout", "suunto_get_workout_fit"]
            } else {
                &[]
            }
        }
        "suunto-route" => {
            if let Some(id) = text(&payload["route"], "id") {
                args["route_id"] = json!(id);
                &["suunto_get_route", "suunto_export_route_gpx"]
            } else {
                &[]
            }
        }
        "suunto-247-activity" | "suunto-247-sleep" | "suunto-247-recovery" => {
            let samples = payload["samples"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|sample| sample["timestamp"].as_str());
            if let (Some(start), Some(end)) = (samples.clone().min(), samples.max()) {
                args["from_value"] = json!(start);
                args["to_value"] = json!(end);
                match kind.as_str() {
                    "suunto-247-activity" => &["suunto_get_activity_samples"],
                    "suunto-247-sleep" => &["suunto_get_sleep_data"],
                    _ => &["suunto_get_recovery_data"],
                }
            } else {
                &[]
            }
        }
        _ => &[],
    };
    let actions: Vec<_> = tools
        .iter()
        .map(|tool| json!({"tool": tool, "arguments": args}))
        .collect();
    json!({"type": payload["type"], "kind": kind, "username": username, "actions": actions})
}

async fn legacy_payload(body: &[u8], content_type: &str) -> Result<Value> {
    let media_type = content_type.split(';').next().unwrap_or_default().trim();
    let mut fields = HashMap::new();
    if media_type.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
        fields = serde_urlencoded::from_bytes(body)?;
    } else if media_type.eq_ignore_ascii_case("multipart/form-data") {
        let boundary = multer::parse_boundary(content_type)?;
        let stream = Body::from(body.to_vec()).into_data_stream();
        let mut form = multer::Multipart::new(stream, boundary);
        while let Some(field) = form.next_field().await? {
            if let Some(name @ ("username" | "workoutid")) = field.name() {
                let name = name.to_owned();
                fields.insert(name, field.text().await?);
            }
        }
    }
    Ok(json!({"username": fields.get("username"), "workoutid": fields.get("workoutid")}))
}

pub async fn receive_webhook(
    app: &App,
    endpoint: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> (StatusCode, Value) {
    let Some((_, fallback)) = WEBHOOK_ENDPOINTS.iter().find(|(path, _)| *path == endpoint) else {
        return error(StatusCode::NOT_FOUND, "unknown_webhook");
    };
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let payload = if endpoint == "/webhooks/suunto/legacy-workout" {
        match legacy_payload(body, content_type.unwrap_or_default()).await {
            Ok(payload) => payload,
            Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_form"),
        }
    } else {
        match serde_json::from_slice::<Value>(body) {
            Ok(payload) if payload.is_object() => payload,
            Ok(_) => return error(StatusCode::BAD_REQUEST, "expected_json_object"),
            Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_json"),
        }
    };

    let secret = &app.config.webhook_secret;
    let verified = if secret.is_empty() {
        None
    } else {
        let signature = headers
            .get("x-hmac-sha256-signature")
            .and_then(|value| value.to_str().ok());
        if !signature.is_some_and(|sig| verify_signature(body, sig, secret).unwrap_or(false)) {
            return error(StatusCode::UNAUTHORIZED, "invalid_signature");
        }
        Some(true)
    };

    let event = json!({
        "kind": classify(&payload, fallback),
        "signature_verified": verified, "content_type": content_type, "payload": payload,
    });
    let store = app.webhooks.clone();
    match tokio::task::spawn_blocking(move || store.append(event)).await {
        Ok(Ok(stored)) => (
            StatusCode::OK,
            json!({"ok": true, "event_id": stored["id"]}),
        ),
        _ => error(StatusCode::INTERNAL_SERVER_ERROR, "webhook_store_error"),
    }
}

fn error(status: StatusCode, name: &str) -> (StatusCode, Value) {
    (status, json!({"ok": false, "error": name}))
}

pub(super) async fn call(app: &App, name: &str, args: &Value) -> Result<Value> {
    if name == "suunto_verify_webhook_signature" {
        let raw = args["body"].as_str().context("body is required.")?;
        let bytes = if args["body_is_base64"].as_bool().unwrap_or(false) {
            let normalized: String = raw.chars().filter(|c| !c.is_ascii_whitespace()).collect();
            STANDARD.decode(normalized)?
        } else {
            raw.as_bytes().to_vec()
        };
        let signature = args["signature"]
            .as_str()
            .context("signature is required.")?;
        let secret = text(args, "secret").unwrap_or(&app.config.webhook_secret);
        return Ok(json!({"valid": verify_signature(&bytes, signature, secret)?}));
    }

    let store = app.webhooks.clone();
    let args = args.clone();
    let name = name.to_owned();
    tokio::task::spawn_blocking(move || {
        let event_id = || args["event_id"].as_str().context("event_id is required.");
        match name.as_str() {
            "suunto_store_webhook_event" => store.append(args["event"].clone()),
            "suunto_list_webhook_events" => {
                Ok(json!({"events": store.list(args["limit"].as_i64().unwrap_or(100))?}))
            }
            "suunto_get_webhook_event" => Ok(store.get(event_id()?)?.unwrap_or(Value::Null)),
            "suunto_delete_webhook_event" => Ok(json!({"deleted": store.delete(event_id()?)?})),
            "suunto_suggest_webhook_followups" => {
                let event = if args["event"].is_object() {
                    args["event"].clone()
                } else {
                    let id = event_id().context("Provide event or event_id.")?;
                    store
                        .get(id)?
                        .with_context(|| format!("No webhook event found for event_id={id:?}."))?
                };
                Ok(followups(&event))
            }
            _ => bail!("Unknown webhook tool: {name}"),
        }
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn sqlite_import_reads_and_writes_python_schema() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("webhooks.sqlite3");
        let python = Connection::open(&path)?;
        python.execute_batch(
            "CREATE TABLE webhook_events (
                id TEXT PRIMARY KEY,
                received_at TEXT NOT NULL,
                kind TEXT,
                username TEXT,
                payload_json TEXT NOT NULL
            );",
        )?;
        let event = json!({
            "id": "legacy", "kind": "suunto-json", "received_at": "2026-06-03T11:22:33+00:00",
            "payload": {"username": "runner"}
        });
        python.execute(
            "INSERT INTO webhook_events VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                "legacy",
                "2026-06-03T11:22:33+00:00",
                "suunto-json",
                "runner",
                event.to_string()
            ],
        )?;

        let store = Webhooks::new("sqlite", path)?;
        assert_eq!(store.get("legacy")?, Some(event));
        let stored = store.append(json!({"id": "new", "payload": {"username": "runner42"}}))?;
        let username: String = python.query_row(
            "SELECT username FROM webhook_events WHERE id = 'new'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(username, "runner42");
        assert_eq!(store.list(1)?, vec![stored]);
        assert!(store.delete("legacy")?);
        assert!(!store.delete("legacy")?);
        Ok(())
    }

    #[test]
    fn jsonl_round_trips_and_lists_newest_first() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("webhooks.jsonl");
        let store = Webhooks::new("jsonl", path.clone())?;
        let first = store.append(json!({"id": "one"}))?;
        let latest = store.append(json!({"id": "two"}))?;
        assert_eq!(store.list(1)?, vec![latest.clone()]);
        assert_eq!(store.list(0)?, vec![latest.clone(), first.clone()]);
        assert_eq!(store.list(-1)?, vec![latest]);
        assert_eq!(store.get("one")?, Some(first));
        assert!(store.delete("one")?);
        assert!(!store.delete("one")?);
        assert_eq!(store.get("one")?, None);
        assert_eq!(fs::read_to_string(path)?.lines().count(), 1);
        Ok(())
    }

    #[test]
    fn memory_history_is_bounded_by_size_and_count() -> Result<()> {
        let store = Webhooks::new("memory", PathBuf::new())?;
        for id in 0..MAX_MEMORY_EVENTS + 2 {
            store.append(json!({"id": id.to_string()}))?;
        }
        assert_eq!(store.get("0")?, None);
        assert_eq!(store.get("1")?, None);
        assert!(store.get("2")?.is_some());
        assert_eq!(store.list(0)?.len(), MAX_MEMORY_EVENTS);

        let large = "x".repeat(MAX_EVENT_BYTES / 2);
        for id in 0..4 {
            store.append(json!({"id": format!("large-{id}"), "payload": large}))?;
        }
        assert_eq!(store.get("large-0")?, None);
        assert!(store.get("large-3")?.is_some());
        Ok(())
    }

    #[test]
    fn followups_include_sample_bounds_and_account() {
        let followups = followups(&json!({"payload": {
            "type": "suunto_247_sleep_created", "username": "runner",
            "samples": [
                {"timestamp": "2026-06-03T11:22:33+00:00"},
                {"timestamp": "2026-06-03T11:12:33+00:00"},
            ]
        }}));
        assert_eq!(
            followups["actions"],
            json!([{
                "tool": "suunto_get_sleep_data",
                "arguments": {
                    "account_id": "runner",
                    "from_value": "2026-06-03T11:12:33+00:00",
                    "to_value": "2026-06-03T11:22:33+00:00",
                }
            }])
        );
    }

    #[tokio::test]
    async fn typed_and_legacy_webhooks_validate_and_store_raw_signatures() -> Result<()> {
        let app = App::new(Config::from_lookup(|key| match key {
            "SUUNTO_WEBHOOK_STORE" => Some("memory".into()),
            "SUUNTO_WEBHOOK_SECRET" => Some("test-secret".into()),
            _ => None,
        })?)?;
        let body = br#"{"username":"runner"}"#;
        let json_headers = signed_headers(body, "application/json");
        let (status, value) =
            receive_webhook(&app, "/webhooks/suunto/247/sleep", &json_headers, body).await;
        assert_eq!(status, StatusCode::OK);
        let id = value["event_id"].as_str().unwrap();
        let event = app.webhooks.get(id)?.unwrap();
        assert_eq!(event["kind"], "suunto-247-sleep");
        assert_eq!(event["signature_verified"], true);
        assert_eq!(event["payload"]["username"], "runner");
        assert_eq!(
            receive_webhook(
                &app,
                "/webhooks/suunto/247/sleep",
                &json_headers,
                br#"{"username":"someone-else"}"#
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );

        let body = b"username=runner42&workoutid=workout%2B1";
        let headers = signed_headers(body, "application/x-www-form-urlencoded");
        let (status, _) =
            receive_webhook(&app, "/webhooks/suunto/legacy-workout", &headers, body).await;
        assert_eq!(status, StatusCode::OK);
        let recent = app.webhooks.list(1)?;
        assert_eq!(recent[0]["payload"]["workoutid"], "workout+1");
        assert_eq!(recent[0]["kind"], "suunto-legacy-workout");

        let body = b"--boundary\r\nContent-Disposition: form-data; name=\"username\"\r\n\r\nrunner43\r\n--boundary--\r\n";
        let headers = signed_headers(body, "multipart/form-data; boundary=boundary");
        let (status, _) =
            receive_webhook(&app, "/webhooks/suunto/legacy-workout", &headers, body).await;
        assert_eq!(status, StatusCode::OK);
        let recent = app.webhooks.list(1)?;
        assert_eq!(recent[0]["payload"]["username"], "runner43");

        let (status, value) =
            receive_webhook(&app, "/webhooks/suunto", &HeaderMap::new(), b"{bad-json").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(value["error"], "invalid_json");
        Ok(())
    }

    fn signed_headers(body: &[u8], content_type: &'static str) -> HeaderMap {
        let mut mac = Hmac::<Sha256>::new_from_slice(b"test-secret").unwrap();
        mac.update(body);
        let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
        headers.insert("x-hmac-sha256-signature", signature.parse().unwrap());
        headers
    }
}
