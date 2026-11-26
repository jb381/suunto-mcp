use std::io::BufRead;

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, NaiveDateTime};
use quick_xml::{escape::resolve_xml_entity, events::Event};
use serde_json::{Map, Value, json};

use super::{
    PreviewBudget, flag, limit, within,
    xml::{XmlReader, attributes},
};

const EARTH_RADIUS_M: f64 = 6_378_137.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Element {
    Gpx,
    Metadata,
    Author,
    Track,
    Segment,
    Route,
    TrackPoint,
    RoutePoint,
    Waypoint,
    Name,
    Description,
    Elevation,
    Time,
    Type,
    Ignored,
}

impl Element {
    fn child(self, name: &str) -> Self {
        use Element::*;
        match (self, name) {
            (Gpx, "metadata") => Metadata,
            (Gpx | Metadata, "author") => Author,
            (Gpx, "trk") => Track,
            (Track, "trkseg") => Segment,
            (Gpx, "rte") => Route,
            (Segment, "trkpt") => TrackPoint,
            (Route, "rtept") => RoutePoint,
            (Gpx, "wpt") => Waypoint,
            (TrackPoint | RoutePoint | Waypoint, "ele") => Elevation,
            (TrackPoint | RoutePoint | Waypoint, "type") => Type,
            (Gpx | Metadata | TrackPoint | RoutePoint | Waypoint, "time") => Time,
            (Gpx | Metadata | Track | Route | TrackPoint | RoutePoint | Waypoint, "desc") => {
                Description
            }
            (
                Gpx | Metadata | Author | Track | Route | TrackPoint | RoutePoint | Waypoint,
                "name",
            ) => Name,
            _ => Ignored,
        }
    }

    fn point(self) -> bool {
        matches!(self, Self::TrackPoint | Self::RoutePoint | Self::Waypoint)
    }

    fn captures_text(self) -> bool {
        matches!(
            self,
            Self::Author
                | Self::Name
                | Self::Description
                | Self::Elevation
                | Self::Time
                | Self::Type
        )
    }
}

#[derive(Default)]
struct Frame {
    text: String,
}

#[derive(Default)]
struct Point {
    lat: f64,
    lon: f64,
    elevation: Option<f64>,
    time: Value,
    name: Value,
    description: Value,
    kind: Value,
}

impl Point {
    fn from_attributes(attrs: &Map<String, Value>) -> Result<Self> {
        let coordinate = |name| -> Result<f64> {
            let value: f64 = attrs
                .get(name)
                .and_then(Value::as_str)
                .with_context(|| format!("Missing GPX coordinate {name}."))?
                .parse()?;
            ensure!(value.is_finite(), "Invalid GPX coordinate.");
            Ok(value)
        };
        Ok(Self {
            lat: coordinate("lat")?,
            lon: coordinate("lon")?,
            ..Self::default()
        })
    }

    fn payload(&self) -> Value {
        json!({
            "latitude": self.lat,
            "longitude": self.lon,
            "elevation": self.elevation,
            "time": self.time,
            "name": self.name,
            "description": self.description,
            "type": self.kind,
        })
    }

    fn distances(&self, previous: &Self) -> (f64, f64) {
        let lat = (self.lat - previous.lat).to_radians();
        let lon = (self.lon - previous.lon).to_radians();
        if (self.lat - previous.lat).abs() > 0.2 || (self.lon - previous.lon).abs() > 0.2 {
            let a = (lat / 2.0).sin().powi(2)
                + self.lat.to_radians().cos()
                    * previous.lat.to_radians().cos()
                    * (lon / 2.0).sin().powi(2);
            let distance = 2.0 * EARTH_RADIUS_M * a.clamp(0.0, 1.0).sqrt().asin();
            return (distance, distance);
        }
        let horizontal = EARTH_RADIUS_M * lat.hypot(lon * self.lat.to_radians().cos());
        let vertical = self
            .elevation
            .zip(previous.elevation)
            .map_or(0.0, |(a, b)| a - b);
        (horizontal, horizontal.hypot(vertical))
    }
}

#[derive(Default)]
struct PointList {
    count: usize,
    preview: Vec<Value>,
}

#[derive(Default)]
struct Segment {
    points: PointList,
    length_2d: f64,
    length_3d: f64,
    previous: Option<Point>,
}

#[derive(Default)]
struct Track {
    name: Value,
    description: Value,
    segments: Vec<Value>,
}

#[derive(Default)]
struct Route {
    name: Value,
    description: Value,
    points: PointList,
}

struct Gpx {
    metadata: Value,
    tracks: Vec<Value>,
    routes: Vec<Value>,
    waypoints: PointList,
    track: Track,
    segment: Segment,
    route: Route,
    point: Option<Point>,
    budget: PreviewBudget,
    retain_limit: i64,
    track_points_retained: usize,
    route_points_retained: usize,
    include_points: bool,
}

impl Gpx {
    fn new(args: &Value) -> Self {
        let include_points = flag(args, "include_points", true);
        Self {
            metadata: json!({"name": null, "description": null, "author_name": null, "time": null}),
            tracks: Vec::new(),
            routes: Vec::new(),
            waypoints: PointList::default(),
            track: Track::default(),
            segment: Segment::default(),
            route: Route::default(),
            point: None,
            budget: PreviewBudget::default(),
            retain_limit: if include_points {
                limit(args, "point_limit", 5000)
            } else {
                0
            },
            track_points_retained: 0,
            route_points_retained: 0,
            include_points,
        }
    }

    fn close_point(&mut self, kind: Element) -> Result<()> {
        let point = self.point.take().context("Unmatched GPX point.")?;
        let waypoint_count = self.waypoints.count;
        let (list, kept) = match kind {
            Element::TrackPoint => (&mut self.segment.points, self.track_points_retained),
            Element::RoutePoint => (&mut self.route.points, self.route_points_retained),
            _ => (&mut self.waypoints, waypoint_count),
        };
        list.count += 1;
        if within(kept, self.retain_limit) {
            let payload = point.payload();
            self.budget.retain(&payload)?;
            list.preview.push(payload);
            match kind {
                Element::TrackPoint => self.track_points_retained += 1,
                Element::RoutePoint => self.route_points_retained += 1,
                _ => {}
            }
        }
        if kind == Element::TrackPoint {
            if let Some(previous) = &self.segment.previous {
                let (flat, spatial) = point.distances(previous);
                self.segment.length_2d += flat;
                self.segment.length_3d += spatial;
            }
            self.segment.previous = Some(point);
        }
        Ok(())
    }

    fn close(&mut self, kind: Element, parent: Element, text: String) -> Result<()> {
        use Element::*;
        match kind {
            Track => {
                let track = std::mem::take(&mut self.track);
                let mut payload = json!({"name": track.name, "description": track.description});
                self.budget.retain(&payload)?;
                payload["segments"] = Value::Array(track.segments);
                self.tracks.push(payload);
            }
            Segment => {
                let segment = std::mem::take(&mut self.segment);
                let mut payload = json!({
                    "point_count": segment.points.count,
                    "length_2d_m": segment.length_2d,
                    "length_3d_m": segment.length_3d,
                });
                self.budget.retain(&payload)?;
                if self.include_points {
                    payload["points"] = Value::Array(segment.points.preview);
                }
                self.track.segments.push(payload);
            }
            Route => {
                let route = std::mem::take(&mut self.route);
                let mut payload = json!({
                    "name": route.name,
                    "description": route.description,
                    "point_count": route.points.count,
                });
                self.budget.retain(&payload)?;
                if self.include_points {
                    payload["points"] = Value::Array(route.points.preview);
                }
                self.routes.push(payload);
            }
            TrackPoint | RoutePoint | Waypoint => self.close_point(kind)?,
            Name | Description | Time | Author | Type | Elevation => {
                self.close_field(kind, parent, text)?
            }
            _ => {}
        }
        Ok(())
    }

    fn close_field(&mut self, kind: Element, parent: Element, text: String) -> Result<()> {
        use Element::*;
        let text = Value::String(text);
        match (kind, parent) {
            (Name, Gpx | Metadata) => self.metadata["name"] = text,
            (Description, Gpx | Metadata) => self.metadata["description"] = text,
            (Name, Author) => self.metadata["author_name"] = text,
            (Author, Gpx | Metadata)
                if text.as_str().is_some_and(|text| !text.trim().is_empty()) =>
            {
                self.metadata["author_name"] = text;
            }
            (Time, Gpx | Metadata) => self.metadata["time"] = timestamp(&text),
            (Name, Track) => self.track.name = text,
            (Description, Track) => self.track.description = text,
            (Name, Route) => self.route.name = text,
            (Description, Route) => self.route.description = text,
            (_, element) if element.point() => {
                let point = self.point.as_mut().context("Missing GPX point.")?;
                match kind {
                    Name => point.name = text,
                    Description => point.description = text,
                    Time => point.time = timestamp(&text),
                    Type => point.kind = text,
                    Elevation => {
                        point.elevation = text.as_str().and_then(|text| text.trim().parse().ok())
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(mut self, point_limit: i64) -> Value {
        let mut left = self.retain_limit.max(0) as usize;
        let trim = |list: &mut Vec<Value>, left: &mut usize| {
            list.truncate(*left);
            *left -= list.len();
        };
        for track in &mut self.tracks {
            for segment in track["segments"].as_array_mut().into_iter().flatten() {
                if let Some(list) = segment.get_mut("points").and_then(Value::as_array_mut) {
                    trim(list, &mut left);
                }
            }
        }
        for route in &mut self.routes {
            if let Some(list) = route.get_mut("points").and_then(Value::as_array_mut) {
                trim(list, &mut left);
            }
        }
        trim(&mut self.waypoints.preview, &mut left);
        json!({
            "metadata": self.metadata,
            "track_count": self.tracks.len(),
            "route_count": self.routes.len(),
            "waypoint_count": self.waypoints.count,
            "tracks": self.tracks,
            "routes": self.routes,
            "waypoints": self.include_points.then_some(self.waypoints.preview),
            "points_included": self.include_points,
            "point_limit": point_limit,
            "points_returned": self.retain_limit.max(0) as usize - left,
        })
    }
}

fn timestamp(value: &Value) -> Value {
    let raw = value.as_str().unwrap_or_default().trim();
    if let Ok(date) = DateTime::parse_from_rfc3339(raw) {
        json!(date.to_rfc3339())
    } else if let Ok(date) = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f") {
        json!(date.format("%Y-%m-%dT%H:%M:%S%.f").to_string())
    } else {
        Value::Null
    }
}

fn append_text(stack: &mut [(Element, Frame)], text: &str) -> Result<()> {
    if let Some((kind, frame)) = stack.last_mut().filter(|(kind, _)| kind.captures_text()) {
        ensure!(
            frame.text.len() + text.len() <= 1024 * 1024,
            "GPX text field exceeds 1 MiB."
        );
        frame.text.push_str(text);
        // The author's name may be a nested element (GPX 1.1) or direct text (GPX 1.0).
        if *kind == Element::Author && frame.text.trim().is_empty() {
            frame.text.clear();
        }
    }
    Ok(())
}

pub(crate) fn parse_gpx(data: &[u8], args: &Value) -> Result<Value> {
    parse_gpx_reader(data, args)
}

pub(super) fn parse_gpx_reader(source: impl BufRead, args: &Value) -> Result<Value> {
    let mut reader = XmlReader::new(source);
    let mut stack: Vec<(Element, Frame)> = Vec::new();
    let mut gpx = Gpx::new(args);
    loop {
        match reader.next()? {
            Event::Start(start) => {
                let tag = start.local_name();
                let kind = if let Some((parent, _)) = stack.last() {
                    parent.child(tag.as_ref())
                } else {
                    ensure!(tag.as_ref() == "gpx", "Expected GPX root element.");
                    Element::Gpx
                };
                if kind.point() {
                    gpx.point = Some(Point::from_attributes(&attributes(&start)?)?);
                }
                stack.push((kind, Frame::default()));
            }
            Event::End(_) => {
                let (closed, frame) = stack.pop().context("Unexpected GPX closing tag.")?;
                let parent = stack.last().map_or(Element::Ignored, |(kind, _)| *kind);
                gpx.close(closed, parent, frame.text)?;
            }
            Event::Text(text) => append_text(&mut stack, &text.xml10_content())?,
            Event::CData(text) => append_text(&mut stack, &text.xml10_content())?,
            Event::GeneralRef(reference) => {
                if let Some(ch) = reference.resolve_char_ref()? {
                    append_text(&mut stack, ch.encode_utf8(&mut [0; 4]))?;
                } else if let Some(text) = resolve_xml_entity(&reference.xml10_content()) {
                    append_text(&mut stack, text)?;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(gpx.finish(limit(args, "point_limit", 5000)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpx_point_limit_applies_in_track_then_route_then_waypoint_order() -> Result<()> {
        let gpx = br#"<?xml version="1.0"?>
            <gpx version="1.1" xmlns="http://www.topografix.com/GPX/1/1">
              <metadata><name>Ride &amp; run</name><author><name>Jane</name></author></metadata>
              <wpt lat="60" lon="25"><name>marker</name></wpt>
              <rte><name>route</name><rtept lat="60" lon="25"><desc>route point</desc></rtept></rte>
              <trk><name>track</name><trkseg>
                <trkpt lat="60" lon="25"><ele>100</ele><time>2026-06-01T08:00:00Z</time></trkpt>
                <trkpt lat="60" lon="25.001"><ele>150</ele></trkpt>
              </trkseg></trk>
            </gpx>"#;
        let output = parse_gpx(gpx, &json!({"point_limit": 2}))?;
        assert_eq!(output["metadata"]["name"], "Ride & run");
        assert_eq!(output["metadata"]["author_name"], "Jane");
        assert_eq!(output["points_returned"], 2);
        assert_eq!(output["waypoints"], json!([]));
        assert_eq!(output["routes"][0]["points"], json!([]));
        let segment = &output["tracks"][0]["segments"][0];
        assert_eq!(segment["point_count"], 2);
        assert_eq!(segment["points"][0]["time"], "2026-06-01T08:00:00+00:00");
        assert!(segment["length_3d_m"].as_f64() > segment["length_2d_m"].as_f64());
        let output = parse_gpx(gpx, &json!({"include_points": false}))?;
        assert!(output["tracks"][0]["segments"][0].get("points").is_none());
        assert!(output["waypoints"].is_null());
        Ok(())
    }
}
