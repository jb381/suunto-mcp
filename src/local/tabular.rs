use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use csv::{ReaderBuilder, StringRecord};
use encoding_rs::{Encoding, UTF_8};
use encoding_rs_io::{DecodeReaderBytes, DecodeReaderBytesBuilder};
use serde_json::{Map, Value, json};

use super::{PreviewBudget, flag, limit, open_file, within};

const SNIFF_SIZE: u64 = 8192;
const MAX_ROW_SIZE: usize = 1024 * 1024;

fn decoder(file: File, label: &str) -> Result<DecodeReaderBytes<File, Vec<u8>>> {
    let label = label.replace('_', "-");
    let encoding = if label.eq_ignore_ascii_case("utf-8-sig") {
        Some(UTF_8)
    } else if label.eq_ignore_ascii_case("utf-16") {
        None
    } else {
        Some(
            Encoding::for_label(label.as_bytes())
                .with_context(|| format!("Unsupported encoding: {label}"))?,
        )
    };
    Ok(DecodeReaderBytesBuilder::new()
        .encoding(encoding)
        .bom_override(encoding.is_none())
        .strip_bom(true)
        .utf8_passthru(true)
        .build(file))
}

struct LimitedRow<R> {
    reader: R,
    remaining: usize,
}

impl<R: Read> Read for LimitedRow<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 && !out.is_empty() {
            return Err(io::Error::other("CSV row exceeds 1 MiB."));
        }
        let max = out.len().min(self.remaining);
        let count = self.reader.read(&mut out[..max])?;
        self.remaining -= count;
        Ok(count)
    }
}

fn sniff_delimiter(sample: &[u8]) -> u8 {
    let complete = if sample.len() == SNIFF_SIZE as usize {
        sample
            .iter()
            .rposition(|byte| matches!(byte, b'\r' | b'\n'))
            .map_or(sample, |end| &sample[..=end])
    } else {
        sample
    };
    for delimiter in *b",\t;|" {
        let mut rows = ReaderBuilder::new()
            .delimiter(delimiter)
            .has_headers(false)
            .flexible(true)
            .from_reader(complete)
            .into_byte_records();
        if let Some(Ok(first)) = rows.next()
            && first.len() > 1
            && rows.all(|row| row.is_ok_and(|row| row.len() == first.len()))
        {
            return delimiter;
        }
    }
    b','
}

pub(super) fn parse_csv(path: &Path, args: &Value) -> Result<Value> {
    let has_header = flag(args, "has_header", true);
    let preview_limit = limit(args, "preview_limit", 100);
    let encoding = args["encoding"].as_str().unwrap_or("utf-8-sig");
    let delimiter = match args["delimiter"].as_str().filter(|value| !value.is_empty()) {
        Some(value) => {
            let bytes = value.as_bytes();
            ensure!(
                bytes.len() == 1 && !matches!(bytes[0], b'\r' | b'\n'),
                "delimiter must be a single ASCII character other than a line break."
            );
            bytes[0]
        }
        None => {
            let mut sample = Vec::new();
            decoder(open_file(path)?, encoding)?
                .take(SNIFF_SIZE)
                .read_to_end(&mut sample)?;
            sniff_delimiter(&sample)
        }
    };
    let input = LimitedRow {
        reader: decoder(open_file(path)?, encoding)?,
        remaining: MAX_ROW_SIZE,
    };
    let mut reader = ReaderBuilder::new()
        .has_headers(false)
        .delimiter(delimiter)
        .flexible(true)
        .from_reader(input);
    let mut record = StringRecord::new();
    let mut headings: Vec<String> = Vec::new();
    let mut first = true;
    let mut rows = Vec::new();
    let mut row_count = 0;
    let mut budget = PreviewBudget::default();
    loop {
        reader.get_mut().remaining = MAX_ROW_SIZE;
        if !reader.read_record(&mut record)? {
            break;
        }
        if first {
            first = false;
            headings = if has_header {
                record.iter().map(str::to_owned).collect()
            } else {
                (1..=record.len())
                    .map(|index| format!("column_{index}"))
                    .collect()
            };
            budget.retain(&json!(headings))?;
            if has_header {
                continue;
            }
        }
        row_count += 1;
        if within(rows.len(), preview_limit) {
            let mut row: Map<String, Value> = headings
                .iter()
                .enumerate()
                .map(|(index, name)| (name.clone(), json!(record.get(index).unwrap_or_default())))
                .collect();
            if record.len() > headings.len() {
                let extra = record
                    .iter()
                    .skip(headings.len())
                    .collect::<Vec<_>>()
                    .join(",");
                row.insert("_extra".into(), json!(extra));
            }
            let row = Value::Object(row);
            budget.retain(&row)?;
            rows.push(row);
        }
    }

    Ok(json!({
        "source_path": path,
        "delimiter": char::from(delimiter).to_string(),
        "has_header": has_header,
        "fieldnames": headings,
        "row_count": row_count,
        "rows_included": rows.len(),
        "preview_limit": preview_limit,
        "rows": rows,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_tsv_with_bom_and_limits_csv_preview() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("activity.tsv");
        fs::write(
            &path,
            "\u{feff}date\tdistance\n2026-06-01\t5000\n2026-06-02\t3000\n",
        )?;
        let parsed = parse_csv(&path, &json!({"preview_limit": 1}))?;
        assert_eq!(parsed["delimiter"], "\t");
        assert_eq!(parsed["fieldnames"], json!(["date", "distance"]));
        assert_eq!(parsed["row_count"], 2);
        assert_eq!(parsed["rows_included"], 1);
        assert_eq!(parsed["rows"][0]["distance"], "5000");
        Ok(())
    }

    #[test]
    fn supports_no_header_and_windows_encoding() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("activity.csv");
        fs::write(&path, b"\"caf\xe9; Sunday\";5000\r\nRun;3000;extra\r\n")?;
        let parsed = parse_csv(
            &path,
            &json!({"delimiter": ";", "has_header": false, "encoding": "windows-1252"}),
        )?;
        assert_eq!(parsed["fieldnames"], json!(["column_1", "column_2"]));
        assert_eq!(parsed["rows"][0]["column_1"], "café; Sunday");
        assert_eq!(parsed["rows"][1]["_extra"], "extra");
        Ok(())
    }

    #[test]
    fn sniffs_large_tsv_when_sample_ends_mid_row() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("activity.tsv");
        fs::write(
            &path,
            format!("type\tdistance\n{}", "running\t5000\n".repeat(1000)),
        )?;
        let parsed = parse_csv(&path, &json!({"preview_limit": 0}))?;
        assert_eq!(parsed["delimiter"], "\t");
        assert_eq!(parsed["row_count"], 1000);
        Ok(())
    }
}
