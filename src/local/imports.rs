use std::{
    fs,
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use uuid::Uuid;

use crate::App;

use super::{
    PreviewBudget, expand_path, flag, gpx::parse_gpx_reader, limit, open_file, parse_fit,
    persist_file, prepare_file, read_file, required, tabular::parse_csv, xml::parse_health,
};

pub(crate) fn call(app: &App, name: &str, args: &Value) -> Result<Value> {
    let (kind, register) = match name {
        "suunto_parse_fit_file" => ("fit", false),
        "suunto_parse_gpx_file" => ("gpx", false),
        "suunto_parse_activity_csv" => ("csv", false),
        "suunto_parse_health_export" => ("health", false),
        "suunto_import_fit_file" => ("fit", true),
        "suunto_import_gpx_file" => ("gpx", true),
        "suunto_import_activity_csv" => ("csv", true),
        "suunto_import_health_export" => ("health", true),
        "suunto_import_activity_json" => ("json", true),
        "suunto_list_imported_files" => return list(app, args),
        "suunto_get_imported_activity" => return get(app, args),
        _ => bail!("Unknown local tool: {name}"),
    };
    let path = expand_path(required(args, "path")?);
    let parsed = parse(kind, &path, args)?;
    if !register {
        return Ok(parsed);
    }
    let summary = summary(kind, &parsed);
    let record = record_import(app, &path, kind, args, &summary)?;
    if kind == "json" {
        Ok(json!({"imported": record, "summary": summary}))
    } else {
        Ok(json!({"imported": record, "parsed": parsed}))
    }
}

fn parse(kind: &str, path: &Path, args: &Value) -> Result<Value> {
    let mut result = match kind {
        "fit" => parse_fit(&read_file(path)?, args)?,
        "gpx" => parse_gpx_reader(BufReader::new(open_file(path)?), args)?,
        "csv" => return parse_csv(path, args),
        "health" => return parse_health(path, args),
        "json" => return Ok(serde_json::from_slice(&read_file(path)?)?),
        _ => bail!("Unsupported import kind: {kind}"),
    };
    result["path"] = json!(path);
    Ok(result)
}

fn summary(kind: &str, parsed: &Value) -> Value {
    let keys = match kind {
        "fit" => &["message_counts", "sessions"][..],
        "gpx" => &["metadata", "track_count", "route_count", "waypoint_count"],
        "csv" => &[
            "delimiter",
            "has_header",
            "fieldnames",
            "row_count",
            "rows_included",
        ],
        "health" => &[
            "element_counts",
            "record_counts",
            "workout_counts",
            "records_included",
            "workouts_included",
            "activity_summaries_included",
        ],
        _ => {
            let top_level = match parsed {
                Value::Object(_) => "dict",
                Value::Array(_) => "list",
                Value::Null => "null",
                Value::String(_) => "str",
                Value::Bool(_) => "bool",
                Value::Number(number) if number.is_f64() => "float",
                Value::Number(_) => "int",
            };
            return json!({
                "top_level_type": top_level,
                "keys": parsed.as_object().map(|object| object.keys().collect::<Vec<_>>()),
                "items": parsed.as_array().map(Vec::len),
            });
        }
    };
    let mut summary: Map<String, Value> = keys
        .iter()
        .map(|key| (key.to_string(), parsed[key].clone()))
        .collect();
    if kind == "fit" {
        summary.insert(
            "laps_count".into(),
            json!(parsed["laps"].as_array().map_or(0, Vec::len)),
        );
        summary.insert(
            "events_count".into(),
            json!(parsed["events"].as_array().map_or(0, Vec::len)),
        );
    }
    Value::Object(summary)
}

fn index_path(app: &App) -> PathBuf {
    app.config.data_dir.join("imports.json")
}

fn read_index(path: &Path) -> Result<Value> {
    match fs::metadata(path) {
        Ok(_) => {
            let index: Value = serde_json::from_slice(&read_file(path)?)?;
            ensure!(
                index["imports"].is_object(),
                "Invalid import index at {}.",
                path.display()
            );
            Ok(index)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(json!({"imports": {}})),
        Err(err) => Err(err.into()),
    }
}

fn list(app: &App, args: &Value) -> Result<Value> {
    let index = read_index(&index_path(app))?;
    let mut budget = PreviewBudget::default();
    let mut records = Vec::new();
    for record in index["imports"]
        .as_object()
        .context("Invalid import index.")?
        .values()
    {
        if args["account_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .is_some_and(|account| record["account_id"] != account)
            || args["kind"]
                .as_str()
                .filter(|value| !value.is_empty())
                .is_some_and(|kind| record["kind"] != kind)
        {
            continue;
        }
        budget.retain(record)?;
        records.push(record);
    }
    Ok(json!({"imports": records}))
}

fn get(app: &App, args: &Value) -> Result<Value> {
    let index = read_index(&index_path(app))?;
    let id = required(args, "import_id")?;
    let record = index["imports"]
        .get(id)
        .with_context(|| format!("No imported file found for import_id={id:?}."))?;
    let path = record["stored_path"]
        .as_str()
        .filter(|path| !path.is_empty())
        .or_else(|| record["source_path"].as_str())
        .context("Import record has no path.")?;
    let kind = required(record, "kind")?;
    let options = match kind {
        "health" => json!({
            "include_records": flag(args, "include_health_records", false),
            "record_limit": limit(args, "health_record_limit", 1000),
        }),
        "csv" => json!({"preview_limit": limit(args, "csv_preview_limit", 100)}),
        _ => args.clone(),
    };
    let parsed = parse(kind, &expand_path(path), &options)?;
    Ok(json!({"imported": record, "parsed": parsed}))
}

fn record_import(
    app: &App,
    path: &Path,
    kind: &str,
    args: &Value,
    summary: &Value,
) -> Result<Value> {
    let files_dir = app.config.data_dir.join("files");
    let mut staged = if flag(args, "copy_file", true) {
        fs::create_dir_all(&files_dir)?;
        Some(NamedTempFile::new_in(&files_dir)?)
    } else {
        None
    };
    let (sha256, size) = digest_and_copy(path, staged.as_mut())?;
    let stored_path = if staged.is_some() {
        let filename = path
            .file_name()
            .context("Import path has no filename.")?
            .to_string_lossy();
        Some(files_dir.join(format!("{}-{filename}", &sha256[..16])))
    } else {
        None
    };
    let id = Uuid::new_v4().to_string();
    let record = json!({
        "import_id": id,
        "source_path": path,
        "stored_path": stored_path,
        "account_id": args["account_id"],
        "kind": kind,
        "sha256": sha256,
        "size": size,
        "imported_at": Utc::now(),
        "parsed_summary": summary,
    });

    let _lock = app
        .imports_lock
        .lock()
        .map_err(|_| anyhow::anyhow!("Import index lock is unavailable."))?;
    let index_path = index_path(app);
    let mut index = read_index(&index_path)?;
    index["imports"][id] = record.clone();
    let mut index_file = prepare_file(&index_path)?;
    serde_json::to_writer_pretty(index_file.as_file_mut(), &index)?;
    ensure!(
        index_file.as_file().metadata()?.len() <= super::MAX_BUFFERED_FILE,
        "Import index exceeds 16 MiB."
    );
    let mut created = false;
    if let (Some(copy), Some(target)) = (staged, &stored_path) {
        copy.as_file().sync_all()?;
        match copy.persist_noclobber(target) {
            Ok(_) => created = true,
            Err(err) if err.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err.into()),
        }
    }
    if let Err(err) = persist_file(index_file, &index_path) {
        if created && let Some(path) = stored_path {
            fs::remove_file(path)
                .context("Cannot clean up staged import after an index write error.")?;
        }
        return Err(err);
    }
    Ok(record)
}

fn digest_and_copy(path: &Path, mut output: Option<&mut NamedTempFile>) -> Result<(String, u64)> {
    let mut input = open_file(path)?;
    let mut sha256 = Sha256::new();
    let mut size = 0;
    let mut buffer = [0_u8; 32 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let bytes = &buffer[..count];
        sha256.update(bytes);
        if let Some(output) = &mut output {
            output.write_all(bytes)?;
        }
        size += count as u64;
    }
    Ok((format!("{:x}", sha256.finalize()), size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn app(dir: &Path) -> Result<App> {
        App::new(Config::from_lookup(|key| match key {
            "SUUNTO_LOCAL_DATA_DIR" => Some(dir.to_string_lossy().into_owned()),
            "SUUNTO_TOKEN_STORE" | "SUUNTO_WEBHOOK_STORE" => Some("memory".into()),
            _ => None,
        })?)
    }

    #[test]
    fn opens_python_import_index_and_appends_compatible_records() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let app = app(dir.path())?;
        let path = dir.path().join("activity.csv");
        fs::write(&path, "time,distance\n08:00,5000\n")?;
        let legacy = json!({"imports": {
            "python-record": {
                "import_id": "python-record", "source_path": path, "stored_path": null, "account_id": "alice",
                "kind": "csv", "sha256": "example", "size": 22, "imported_at": "2026-06-01T08:00:00Z",
                "parsed_summary": {}
            }
        }});
        fs::write(index_path(&app), serde_json::to_vec(&legacy)?)?;
        let parsed = call(
            &app,
            "suunto_get_imported_activity",
            &json!({"import_id": "python-record"}),
        )?;
        assert_eq!(parsed["parsed"]["rows"][0]["distance"], "5000");

        let imported = call(
            &app,
            "suunto_import_activity_csv",
            &json!({"path": path, "account_id": "alice"}),
        )?;
        let record = &imported["imported"];
        assert_eq!(record["kind"], "csv");
        assert_eq!(
            record["parsed_summary"]["fieldnames"],
            json!(["time", "distance"])
        );
        let copy = record["stored_path"]
            .as_str()
            .context("No copied CSV path.")?;
        assert_eq!(fs::read(copy)?, fs::read(&path)?);
        let count = call(
            &app,
            "suunto_list_imported_files",
            &json!({"kind": "csv", "account_id": "alice"}),
        )?;
        assert_eq!(count["imports"].as_array().map(Vec::len), Some(2));
        assert_eq!(
            read_index(&index_path(&app))?["imports"]["python-record"],
            legacy["imports"]["python-record"]
        );
        Ok(())
    }

    #[test]
    fn concurrent_imports_preserve_both_index_entries_and_deduplicate_copies() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let app = app(dir.path())?;
        let path = dir.path().join("activity.json");
        fs::write(&path, r#"{"name":"ride"}"#)?;
        let results = std::thread::scope(|scope| {
            let import = || call(&app, "suunto_import_activity_json", &json!({"path": path}));
            let first = scope.spawn(import);
            let second = scope.spawn(import);
            [first.join().unwrap(), second.join().unwrap()]
        });
        let [first, second] = results;
        let (first, second) = (first?, second?);
        assert_ne!(
            first["imported"]["import_id"],
            second["imported"]["import_id"]
        );
        assert_eq!(
            first["imported"]["stored_path"],
            second["imported"]["stored_path"]
        );
        assert_eq!(
            read_index(&index_path(&app))?["imports"]
                .as_object()
                .map(Map::len),
            Some(2)
        );
        Ok(())
    }
}
