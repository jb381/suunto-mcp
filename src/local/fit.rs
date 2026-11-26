use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use fitparser::{
    FitDataRecord, Value as FitValue,
    de::{DecodeOption, FitObject, FitStreamProcessor},
    profile::MesgNum,
};
use serde_json::{Map, Value, json};

use super::{PreviewBudget, flag, limit, within};

pub(crate) fn parse_fit(data: &[u8], args: &Value) -> Result<Value> {
    ensure!(!data.is_empty(), "Empty FIT file.");
    let include_records = flag(args, "include_records", false);
    let record_limit = limit(args, "record_limit", 1000);
    let include_messages = flag(args, "include_messages", false);
    let message_limit = limit(args, "message_limit", 2000);
    let mut processor = FitStreamProcessor::new();
    processor.add_option(DecodeOption::SkipHeaderCrcValidation);
    processor.add_option(DecodeOption::SkipDataCrcValidation);
    processor.add_option(DecodeOption::KeepCompositeFields);

    let mut input = data;
    let mut counts = BTreeMap::<String, usize>::new();
    let mut groups = BTreeMap::<&str, Vec<Value>>::from([
        ("file_ids", Vec::new()),
        ("sessions", Vec::new()),
        ("laps", Vec::new()),
        ("events", Vec::new()),
    ]);
    let mut records = Vec::new();
    let mut messages = Vec::new();
    let mut budget = PreviewBudget::default();
    while !input.is_empty() {
        let (remaining, object) = processor
            .deserialize_next(input)
            .map_err(|err| anyhow::anyhow!("Invalid FIT file: {err}"))?;
        input = remaining;
        let raw = match object {
            FitObject::DataMessage(raw) => raw,
            FitObject::Crc(_) => {
                processor.reset();
                continue;
            }
            _ => continue,
        };
        let number = raw.global_message_number();
        let kind = MesgNum::from(number);
        let name = if MesgNum::is_named_variant(i64::from(number)) {
            kind.to_string()
        } else {
            format!("unknown_{number}")
        };
        *counts.entry(name.clone()).or_default() += 1;
        let is_record =
            kind == MesgNum::Record && include_records && within(records.len(), record_limit);
        let keep_message = include_messages && within(messages.len(), message_limit);
        let group = match kind {
            MesgNum::FileId => groups.get_mut("file_ids"),
            MesgNum::Session => groups.get_mut("sessions"),
            MesgNum::Lap => groups.get_mut("laps"),
            MesgNum::Event => groups.get_mut("events"),
            _ => None,
        };
        if is_record || keep_message || group.is_some() {
            let raw_fields = raw.clone();
            let decoded = processor
                .decode_message(raw)
                .map_err(|err| anyhow::anyhow!("Invalid FIT message: {err}"))?;
            let message = message_json(&name, &decoded, &raw_fields)?;
            if keep_message {
                budget.retain(&message)?;
                messages.push(message.clone());
            }
            if is_record {
                budget.retain(&message)?;
                records.push(message);
            } else if let Some(group) = group {
                budget.retain(&message)?;
                group.push(message);
            }
        } else {
            // Decoding all messages keeps timestamps, accumulators, and developer
            // field definitions up to date for subsequent retained messages.
            processor
                .decode_message(raw)
                .map_err(|err| anyhow::anyhow!("Invalid FIT message: {err}"))?;
        }
    }

    let mut result = json!({
        "size": data.len(),
        "message_counts": counts,
        "records_included": include_records,
        "record_limit": record_limit,
        "record_count_returned": records.len(),
        "total_record_messages": counts.get("record").copied().unwrap_or_default(),
    });
    for (name, group) in groups {
        result[name] = json!(group);
    }
    if include_records {
        result["records"] = json!(records);
    }
    if include_messages {
        result["message_limit"] = json!(message_limit);
        result["message_count_returned"] = json!(messages.len());
        result["messages"] = json!(messages);
    }
    Ok(result)
}

fn message_json(
    name: &str,
    message: &FitDataRecord,
    raw: &fitparser::de::FitDataMessage,
) -> Result<Value> {
    let mut fields = Map::new();
    let mut details = Vec::new();
    for field in message.fields() {
        let field_name = field.name();
        let (field_name, units) = if field_name == format!("unknown_field_{}", field.number()) {
            (format!("unknown_{}", field.number()), None)
        } else {
            (
                field_name.to_string(),
                Some(field.units()).filter(|units| !units.is_empty()),
            )
        };
        let info = serde_json::to_value(field)?;
        let raw_value = match info["developer_data_index"].as_u64() {
            Some(index) => raw.developer_fields().get(&(index as u8, field.number())),
            None => raw.fields().get(&field.number()),
        };
        let value = fit_value(field.value())?;
        let raw_value = raw_value.map(fit_value).transpose()?.unwrap_or(Value::Null);
        fields.insert(field_name.clone(), value.clone());
        details.push(
            json!({"name": field_name, "value": value, "units": units, "raw_value": raw_value}),
        );
    }
    Ok(json!({"name": name, "fields": fields, "field_details": details}))
}

fn fit_value(value: &FitValue) -> Result<Value> {
    match value {
        FitValue::Timestamp(time) => Ok(json!(
            time.naive_utc().format("%Y-%m-%dT%H:%M:%S").to_string()
        )),
        FitValue::Invalid => Ok(Value::Null),
        FitValue::Array(items) => Ok(Value::Array(
            items.iter().map(fit_value).collect::<Result<_>>()?,
        )),
        value => Ok(serde_json::to_value(value)?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_records_with_scaled_detail_and_respects_limit() -> Result<()> {
        // FIT header, one local record definition (altitude: uint16), two records,
        // and a CRC deliberately left zero for parity with the old parser.
        let data = [
            12, 16, 0, 0, 15, 0, 0, 0, b'.', b'F', b'I', b'T', 0x40, 0, 0, 20, 0, 1, 2, 2, 0x84, 0,
            0xe8, 0x0b, 0, 0xf2, 0x0b, 0, 0,
        ];
        let parsed = parse_fit(
            &data,
            &json!({"include_records": true, "record_limit": 1, "include_messages": true}),
        )?;
        assert_eq!(parsed["total_record_messages"], 2);
        assert_eq!(parsed["record_count_returned"], 1);
        assert_eq!(parsed["message_count_returned"], 2);
        assert!(
            (parsed["records"][0]["fields"]["altitude"].as_f64().unwrap() - 109.6).abs() < 1e-9
        );
        assert_eq!(parsed["records"][0]["field_details"][0]["raw_value"], 3048);
        assert!(parse_fit(&data[..9], &json!({})).is_err());
        Ok(())
    }
}
