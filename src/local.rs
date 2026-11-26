mod fit;
mod gpx;
mod imports;
mod tabular;
mod xml;

pub(crate) use crate::config::expand_path;
pub(crate) use fit::parse_fit;
pub(crate) use gpx::parse_gpx;
pub(crate) use imports::call;

use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::App;

const MAX_BUFFERED_FILE: u64 = 16 * 1024 * 1024;
const MAX_PREVIEW_BYTES: usize = 8 * 1024 * 1024;

fn open_file(path: &Path) -> Result<File> {
    ensure!(
        fs::metadata(path)
            .with_context(|| format!("Cannot inspect {}", path.display()))?
            .is_file(),
        "Expected a regular file at {}.",
        path.display()
    );
    File::open(path).with_context(|| format!("Cannot open {}", path.display()))
}

pub(crate) fn read_file(path: &Path) -> Result<Vec<u8>> {
    let file = open_file(path)?;
    ensure!(
        file.metadata()?.len() <= MAX_BUFFERED_FILE,
        "File exceeds the 16 MiB buffered input limit."
    );
    let mut bytes = Vec::new();
    file.take(MAX_BUFFERED_FILE + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_BUFFERED_FILE,
        "File exceeds the 16 MiB buffered input limit."
    );
    Ok(bytes)
}

fn required<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    args[name]
        .as_str()
        .with_context(|| format!("{name} is required."))
}

fn flag(args: &Value, name: &str, default: bool) -> bool {
    args[name].as_bool().unwrap_or(default)
}

fn limit(args: &Value, name: &str, default: i64) -> i64 {
    args[name].as_i64().unwrap_or(default)
}

fn within(count: usize, limit: i64) -> bool {
    (count as u64) < limit.max(0) as u64
}

fn safe_filename(value: &str) -> String {
    let mut clean = String::new();
    let mut in_invalid_run = false;
    for c in value.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            clean.push(c);
            in_invalid_run = false;
        } else if !in_invalid_run {
            clean.push('-');
            in_invalid_run = true;
        }
    }
    let clean = clean.trim_matches(['.', '-']);
    if clean.is_empty() {
        "suunto-export".into()
    } else {
        clean.into()
    }
}

fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn prepare_file(path: &Path) -> Result<NamedTempFile> {
    fs::create_dir_all(parent_dir(path))?;
    Ok(NamedTempFile::new_in(parent_dir(path))?)
}

fn prepare_output_file(path: &Path) -> Result<NamedTempFile> {
    let directory = parent_dir(path);
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    let staged = {
        use std::os::unix::fs::PermissionsExt;
        tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o666))
            .tempfile_in(directory)?
    };
    #[cfg(not(unix))]
    let staged = NamedTempFile::new_in(directory)?;
    if let Ok(existing) = fs::metadata(path) {
        staged.as_file().set_permissions(existing.permissions())?;
    }
    Ok(staged)
}

fn persist_file(file: NamedTempFile, path: &Path) -> Result<()> {
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

pub(crate) fn binary_output(
    app: &App,
    data: &[u8],
    filename: &str,
    content_type: Option<&str>,
    mode: &str,
) -> Result<Value> {
    ensure!(
        matches!(mode, "metadata" | "base64" | "file" | "both"),
        "Invalid output_mode."
    );
    let filename = safe_filename(filename);
    let mut output = json!({
        "filename": filename,
        "content_type": content_type,
        "size": data.len(),
        "sha256": format!("{:x}", Sha256::digest(data)),
        "output_mode": mode,
    });
    if matches!(mode, "file" | "both") {
        if app.config.export_dir.as_os_str().is_empty() {
            ensure!(
                mode != "file",
                "SUUNTO_EXPORT_DIR is required for file output."
            );
        } else {
            let path = app.config.export_dir.join(filename);
            let mut file = prepare_output_file(&path)?;
            file.write_all(data)?;
            persist_file(file, &path)?;
            output["path"] = json!(path);
        }
    }
    if matches!(mode, "base64" | "both") {
        let max = app.config.max_base64_bytes;
        if data.len() > max {
            output["base64_omitted"] = json!(true);
            output["note"] = json!(format!(
                "Payload is {} bytes, above SUUNTO_MAX_BASE64_BYTES={max}.",
                data.len()
            ));
        } else {
            output["base64"] = json!(STANDARD.encode(data));
        }
    }
    Ok(output)
}

fn validate_guide(data: &Value) -> Result<Value> {
    ensure!(data.is_object(), "Guide JSON must be an object.");
    let mut errors = Vec::new();
    if data["usage"] != "workout" {
        errors.push("usage must be 'workout'.");
    }
    if data["name"].as_str().is_none_or(str::is_empty) {
        errors.push("name is required.");
    }
    if data["owner"].as_str().is_none_or(str::is_empty) {
        errors.push("owner is required and should match the Suunto OAuth app name.");
    }
    match data["steps"].as_array() {
        Some(steps) if steps.len() > 1000 => errors.push("steps must contain at most 1000 items."),
        Some(steps) if !steps.is_empty() => {}
        _ => errors.push("steps must be a non-empty list."),
    }
    if !data["externalId"].is_null()
        && data["externalId"]
            .as_str()
            .is_none_or(|id| id.chars().count() > 64)
    {
        errors.push("externalId must be a string with at most 64 characters.");
    }
    Ok(json!({"valid": errors.is_empty(), "errors": errors}))
}

pub(crate) fn guide_call(name: &str, args: &Value) -> Result<Value> {
    match name {
        "suunto_validate_guide_json" if !args["guide"].is_null() => validate_guide(&args["guide"]),
        "suunto_validate_guide_json" | "suunto_create_guide_zip" => {
            let guide = read_file(&expand_path(required(args, "guide_json_path")?))?;
            let validation = validate_guide(&serde_json::from_slice(&guide)?)?;
            if name == "suunto_validate_guide_json" {
                return Ok(validation);
            }
            if validation["valid"] != true {
                return Ok(json!({"created": false, "validation": validation}));
            }
            let path = expand_path(required(args, "output_path")?);
            let icon_path = expand_path(required(args, "icon_png_path")?);
            let mut icon = open_file(&icon_path)?;
            let mut file = prepare_output_file(&path)?;
            let mut archive = ZipWriter::new(file.as_file_mut());
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            archive.start_file("guide.json", options)?;
            archive.write_all(&guide)?;
            archive.start_file("icon.png", options)?;
            io::copy(&mut icon, &mut archive)?;
            let size = archive.finish()?.metadata()?.len();
            persist_file(file, &path)?;
            Ok(json!({"created": true, "path": path, "size": size, "validation": validation}))
        }
        _ => bail!("Unknown guide tool: {name}"),
    }
}

#[derive(Default)]
struct PreviewBudget(usize);

impl PreviewBudget {
    fn retain(&mut self, value: &Value) -> Result<()> {
        self.0 += retained_size(value);
        ensure!(
            self.0 <= MAX_PREVIEW_BYTES,
            "Parsed preview exceeds 8 MiB. Request fewer samples."
        );
        Ok(())
    }
}

fn retained_size(value: &Value) -> usize {
    let children = match value {
        Value::Object(map) => map
            .iter()
            .map(|(name, value)| name.len() + retained_size(value))
            .sum(),
        Value::Array(values) => values.iter().map(retained_size).sum(),
        Value::String(string) => string.len(),
        _ => 0,
    };
    children + std::mem::size_of::<Value>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn export_names_are_safe() {
        assert_eq!(safe_filename("../a å//ß b.fit"), "a-b.fit");
        assert_eq!(safe_filename("...//"), "suunto-export");
    }

    #[test]
    fn binary_outputs_respect_opt_in_paths_and_inline_limit() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let app = App::new(Config::from_lookup(|key| match key {
            "SUUNTO_EXPORT_DIR" => Some(dir.path().to_string_lossy().into_owned()),
            "SUUNTO_MAX_BASE64_BYTES" => Some("3".into()),
            "SUUNTO_TOKEN_STORE" | "SUUNTO_WEBHOOK_STORE" => Some("memory".into()),
            _ => None,
        })?)?;
        let result = binary_output(&app, b"data", "../sample.fit", None, "both")?;
        assert_eq!(result["filename"], "sample.fit");
        assert_eq!(result["base64_omitted"], true);
        assert_eq!(fs::read(dir.path().join("sample.fit"))?, b"data");

        let mut app = app;
        app.config.export_dir.clear();
        let result = binary_output(&app, b"ABC", "sample.fit", None, "both")?;
        assert_eq!(result["base64"], "QUJD");
        assert!(result.get("path").is_none());
        assert!(binary_output(&app, b"ABC", "sample.fit", None, "file").is_err());
        Ok(())
    }

    #[test]
    fn guide_validates_and_zips_expected_entries() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let guide = dir.path().join("input.json");
        let icon = dir.path().join("source.png");
        let output = dir.path().join("export/guide.zip");
        let data = json!({"usage": "workout", "name": "Intervals", "owner": "Demo", "steps": [{}]});
        fs::write(&guide, data.to_string())?;
        fs::write(&icon, b"icon")?;
        let result = guide_call(
            "suunto_create_guide_zip",
            &json!({"guide_json_path": guide, "icon_png_path": icon, "output_path": output}),
        )?;
        assert_eq!(result["created"], true);
        let mut archive = zip::ZipArchive::new(File::open(output)?)?;
        assert_eq!(archive.len(), 2);
        assert_eq!(
            serde_json::from_reader::<_, Value>(archive.by_name("guide.json")?)?,
            data
        );
        let mut icon = Vec::new();
        archive.by_name("icon.png")?.read_to_end(&mut icon)?;
        assert_eq!(icon, b"icon");
        assert_eq!(validate_guide(&json!({}))?["valid"], false);
        Ok(())
    }
}
