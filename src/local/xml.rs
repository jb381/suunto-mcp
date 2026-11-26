use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufReader, Read},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use quick_xml::{
    Reader, XmlVersion,
    escape::resolve_xml_entity,
    events::{BytesStart, Event},
};
use serde_json::{Map, Value, json};

use super::{PreviewBudget, flag, limit, open_file, within};

const MAX_XML_EVENT_BYTES: usize = 1024 * 1024;
const MAX_XML_DEPTH: usize = 256;
const MAX_XML_COUNT_KEYS: usize = 4096;

struct LimitedEvent<R> {
    source: R,
    remaining: usize,
}

impl<R: BufRead> BufRead for LimitedEvent<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        let input = self.source.fill_buf()?;
        if self.remaining == 0 && !input.is_empty() {
            return Err(io::Error::other("XML markup or text exceeds 1 MiB."));
        }
        Ok(&input[..input.len().min(self.remaining)])
    }

    fn consume(&mut self, amount: usize) {
        self.remaining -= amount;
        self.source.consume(amount);
    }
}

impl<R: BufRead> Read for LimitedEvent<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = self.fill_buf()?.read(output)?;
        self.consume(count);
        Ok(count)
    }
}

pub(super) struct XmlReader<R> {
    reader: Reader<LimitedEvent<R>>,
    buffer: Vec<u8>,
    depth: usize,
    root_seen: bool,
}

impl<R: BufRead> XmlReader<R> {
    pub(super) fn new(source: R) -> Self {
        let mut reader = Reader::from_reader(LimitedEvent {
            source,
            remaining: MAX_XML_EVENT_BYTES,
        });
        reader.config_mut().expand_empty_elements = true;
        Self {
            reader,
            buffer: Vec::new(),
            depth: 0,
            root_seen: false,
        }
    }

    pub(super) fn next(&mut self) -> Result<Event<'_>> {
        self.reader.get_mut().remaining = MAX_XML_EVENT_BYTES;
        self.buffer.clear();
        let event = self.reader.read_event_into(&mut self.buffer)?;
        match &event {
            Event::Start(_) => {
                if self.depth == 0 {
                    ensure!(!self.root_seen, "Expected a single XML root.");
                    self.root_seen = true;
                }
                self.depth += 1;
                ensure!(
                    self.depth <= MAX_XML_DEPTH,
                    "XML nesting exceeds {MAX_XML_DEPTH}."
                );
            }
            Event::End(_) => {
                ensure!(self.depth > 0, "Unexpected XML closing tag.");
                self.depth -= 1;
            }
            Event::DocType(doctype) => ensure!(
                !doctype.contains("<!ENTITY"),
                "XML entity declarations are unsupported."
            ),
            Event::GeneralRef(reference) => ensure!(
                reference.resolve_char_ref()?.is_some()
                    || resolve_xml_entity(&reference.xml10_content()).is_some(),
                "Unsupported XML entity."
            ),
            Event::Text(text) if self.depth == 0 => ensure!(
                text.xml10_content().trim().is_empty(),
                "Unexpected text outside the XML root."
            ),
            Event::Eof => ensure!(
                self.root_seen && self.depth == 0,
                "Incomplete XML document."
            ),
            _ => {}
        }
        Ok(event)
    }
}

pub(super) fn attributes(element: &BytesStart<'_>) -> Result<Map<String, Value>> {
    element
        .attributes()
        .map(|item| {
            let attr = item?;
            let key = attr.key.into_inner().into();
            let value = attr.normalized_value(XmlVersion::Implicit1_0)?.into_owned();
            Ok((key, Value::String(value)))
        })
        .collect()
}

#[derive(Default)]
struct Counts(BTreeMap<String, usize>);

impl Counts {
    fn increment(&mut self, key: &str) -> Result<()> {
        if let Some(count) = self.0.get_mut(key) {
            *count += 1;
        } else {
            ensure!(
                self.0.len() < MAX_XML_COUNT_KEYS && key.len() <= 1024,
                "XML contains too many distinct names or an oversized name."
            );
            self.0.insert(key.into(), 1);
        }
        Ok(())
    }
}

#[derive(Default)]
struct Workout {
    kind: String,
    attrs: Option<Map<String, Value>>,
    metadata: Vec<Value>,
    events: Vec<Value>,
}

pub(super) fn parse_health(path: &Path, args: &Value) -> Result<Value> {
    let include_records = flag(args, "include_records", false);
    let record_limit = limit(args, "record_limit", 1000);
    let include_workouts = flag(args, "include_workouts", true);
    let workout_limit = limit(args, "workout_limit", 500);
    let include_summaries = flag(args, "include_activity_summaries", true);
    let summary_limit = limit(args, "activity_summary_limit", 500);

    let file = open_file(path)?;
    let mut reader = XmlReader::new(BufReader::new(file));
    let mut element_counts = Counts::default();
    let mut record_counts = Counts::default();
    let mut workout_counts = Counts::default();
    let mut records = Vec::new();
    let mut workouts = Vec::new();
    let mut activity_summaries = Vec::new();
    let mut current_workout: Option<Workout> = None;
    let mut budget = PreviewBudget::default();
    loop {
        match reader.next()? {
            Event::Start(element) => {
                let tag = element.local_name();
                match tag.as_ref() {
                    "Record" => {
                        let attrs = attributes(&element)?;
                        record_counts.increment(
                            attrs
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown"),
                        )?;
                        if include_records && within(records.len(), record_limit) {
                            let item = Value::Object(attrs);
                            budget.retain(&item)?;
                            records.push(item);
                        }
                    }
                    "ActivitySummary"
                        if include_summaries && within(activity_summaries.len(), summary_limit) =>
                    {
                        let item = Value::Object(attributes(&element)?);
                        budget.retain(&item)?;
                        activity_summaries.push(item);
                    }
                    "Workout" => {
                        ensure!(
                            current_workout.is_none(),
                            "Unexpected nested Workout element."
                        );
                        let attrs = attributes(&element)?;
                        let kind = attrs
                            .get("workoutActivityType")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .into();
                        let attrs = (include_workouts && within(workouts.len(), workout_limit))
                            .then_some(attrs);
                        current_workout = Some(Workout {
                            kind,
                            attrs,
                            ..Workout::default()
                        });
                    }
                    "MetadataEntry" | "WorkoutEvent" => {
                        if let Some(workout) = current_workout
                            .as_mut()
                            .filter(|workout| workout.attrs.is_some())
                        {
                            let item = Value::Object(attributes(&element)?);
                            budget.retain(&item)?;
                            if tag.as_ref() == "MetadataEntry" {
                                workout.metadata.push(item);
                            } else {
                                workout.events.push(item);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::End(element) => {
                let tag = element.local_name();
                element_counts.increment(tag.as_ref())?;
                if tag.as_ref() == "Workout" {
                    let workout = current_workout
                        .take()
                        .context("No matching Workout element.")?;
                    workout_counts.increment(&workout.kind)?;
                    if let Some(attrs) = workout.attrs {
                        let mut item = Value::Object(attrs);
                        budget.retain(&item)?;
                        if !workout.metadata.is_empty() {
                            item["metadata"] = Value::Array(workout.metadata);
                        }
                        if !workout.events.is_empty() {
                            item["events"] = Value::Array(workout.events);
                        }
                        workouts.push(item);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(json!({
        "source_path": path,
        "element_counts": element_counts.0,
        "record_counts": record_counts.0,
        "workout_counts": workout_counts.0,
        "records_included": records.len(),
        "record_limit": record_limit,
        "workouts_included": workouts.len(),
        "workout_limit": workout_limit,
        "activity_summaries_included": activity_summaries.len(),
        "activity_summary_limit": summary_limit,
        "records": records,
        "workouts": workouts,
        "activity_summaries": activity_summaries,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn streams_health_records_and_workout_details() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("export.xml");
        fs::write(
            &path,
            r#"<?xml version="1.0"?>
          <!DOCTYPE HealthData [<!ELEMENT HealthData ANY>]>
          <HealthData>
            <Record type="steps" value="500"/>
            <Record type="heart_rate" value="130"/>
            <Record type="steps" value="200"/>
            <Workout workoutActivityType="run">
              <MetadataEntry key="weather" value="wind &amp; rain"/>
              <WorkoutEvent type="pause"/>
            </Workout>
            <ActivitySummary dateComponents="2026-06-01"/>
          </HealthData>"#,
        )?;
        let parsed = parse_health(&path, &json!({"include_records": true, "record_limit": 1}))?;
        assert_eq!(parsed["element_counts"]["Record"], 3);
        assert_eq!(parsed["record_counts"]["steps"], 2);
        assert_eq!(parsed["workout_counts"]["run"], 1);
        assert_eq!(parsed["records_included"], 1);
        assert_eq!(parsed["activity_summaries_included"], 1);
        assert_eq!(parsed["workouts"][0]["metadata"][0]["value"], "wind & rain");
        assert_eq!(parsed["workouts"][0]["events"][0]["type"], "pause");
        Ok(())
    }

    #[test]
    fn rejects_entity_expansion_and_truncated_xml() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("export.xml");
        fs::write(
            &path,
            b"<!DOCTYPE HealthData [<!ENTITY test 'text'>]><HealthData/>",
        )?;
        assert!(parse_health(&path, &json!({})).is_err());
        fs::write(&path, b"<HealthData><Record/>")?;
        assert!(parse_health(&path, &json!({})).is_err());
        Ok(())
    }
}
