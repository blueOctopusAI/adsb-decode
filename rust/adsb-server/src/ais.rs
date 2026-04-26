//! AIS (Automatic Identification System) message parsing for the
//! AISStream.io WebSocket feed.
//!
//! Parses two of the 24 AIS message types AISStream relays:
//!
//! - **PositionReport** (and its Class B variants) — gives MMSI + position +
//!   speed-over-ground + course-over-ground + heading. The bread and butter
//!   of "where is each ship right now."
//! - **ShipStaticData** — gives MMSI + ship name + vessel type + flag.
//!   Sent less often (every few minutes per ship); used to populate the
//!   `vessels` metadata table.
//!
//! Other message types (BaseStationReport, SafetyBroadcast, IFF, etc.) are
//! silently skipped — they don't contribute to "show ships on the map."
//!
//! Timestamps come from system clock at receive time. AISStream emits a
//! `time_utc` field in metadata, but its format is custom; using receive
//! time is good enough for "live ship positions" since the network delay
//! is sub-second.
//!
//! No I/O, no async — pure parse logic. Tested against the example messages
//! in AISStream's published documentation.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VesselPosition {
    pub mmsi: String,
    pub lat: f64,
    pub lon: f64,
    /// Speed over ground in knots (None when AIS reports "not available")
    pub speed_kts: Option<f64>,
    /// Course over ground in degrees (None when AIS reports "not available")
    pub course_deg: Option<f64>,
    /// True heading in degrees (None when AIS reports the
    /// "not available" sentinel value 511)
    pub heading_deg: Option<f64>,
    /// Optional ship name from the message metadata
    pub ship_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VesselStatic {
    pub mmsi: String,
    pub name: Option<String>,
    /// Numeric AIS ship-type code mapped to a string label (cargo, tanker, etc.)
    pub vessel_type: Option<String>,
    /// Flag state / country, if available
    pub flag: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AisParsed {
    Position(VesselPosition),
    Static(VesselStatic),
}

/// Parse one raw AISStream WebSocket message (JSON string). Returns
/// `None` for message types we don't handle, or for malformed JSON.
pub fn parse_message(raw: &str) -> Option<AisParsed> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let msg_type = v.get("MessageType")?.as_str()?;
    let metadata = v.get("Metadata")?;
    let message = v.get("Message")?;

    let mmsi = mmsi_from_metadata(metadata)?;

    match msg_type {
        // Class A position reports (types 1, 2, 3)
        "PositionReport" => {
            let inner = message.get("PositionReport")?;
            Some(AisParsed::Position(VesselPosition {
                mmsi,
                lat: lat_from(inner, metadata)?,
                lon: lon_from(inner, metadata)?,
                speed_kts: extract_sog(inner),
                course_deg: extract_cog(inner),
                heading_deg: extract_heading(inner),
                ship_name: ship_name_from_metadata(metadata),
            }))
        }
        // Class B position reports — equivalent shape, different field
        // path because AISStream models them as separate inner objects.
        "StandardClassBPositionReport" => {
            let inner = message.get("StandardClassBPositionReport")?;
            Some(AisParsed::Position(VesselPosition {
                mmsi,
                lat: lat_from(inner, metadata)?,
                lon: lon_from(inner, metadata)?,
                speed_kts: extract_sog(inner),
                course_deg: extract_cog(inner),
                heading_deg: extract_heading(inner),
                ship_name: ship_name_from_metadata(metadata),
            }))
        }
        "ExtendedClassBPositionReport" => {
            let inner = message.get("ExtendedClassBPositionReport")?;
            Some(AisParsed::Position(VesselPosition {
                mmsi,
                lat: lat_from(inner, metadata)?,
                lon: lon_from(inner, metadata)?,
                speed_kts: extract_sog(inner),
                course_deg: extract_cog(inner),
                heading_deg: extract_heading(inner),
                ship_name: ship_name_from_metadata(metadata),
            }))
        }
        "ShipStaticData" => {
            let inner = message.get("ShipStaticData")?;
            Some(AisParsed::Static(VesselStatic {
                mmsi,
                name: ship_name_from_static(inner),
                vessel_type: ship_type_from_static(inner),
                flag: None, // AISStream doesn't relay MID-derived flag here
            }))
        }
        _ => None,
    }
}

fn mmsi_from_metadata(metadata: &serde_json::Value) -> Option<String> {
    metadata
        .get("MMSI")
        .and_then(|v| v.as_u64())
        .map(|n| n.to_string())
}

fn ship_name_from_metadata(metadata: &serde_json::Value) -> Option<String> {
    metadata
        .get("ShipName")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().trim_end_matches('@').trim().to_string())
        .filter(|s| !s.is_empty())
}

fn lat_from(inner: &serde_json::Value, metadata: &serde_json::Value) -> Option<f64> {
    // The AIS Message inner object uses Latitude (capitalized);
    // the Metadata uses lowercase latitude. Try inner first, then metadata.
    inner
        .get("Latitude")
        .and_then(|v| v.as_f64())
        .or_else(|| metadata.get("latitude").and_then(|v| v.as_f64()))
        .filter(|&n| n.abs() <= 90.0)
}

fn lon_from(inner: &serde_json::Value, metadata: &serde_json::Value) -> Option<f64> {
    inner
        .get("Longitude")
        .and_then(|v| v.as_f64())
        .or_else(|| metadata.get("longitude").and_then(|v| v.as_f64()))
        .filter(|&n| n.abs() <= 180.0)
}

fn extract_sog(inner: &serde_json::Value) -> Option<f64> {
    let n = inner.get("Sog").and_then(|v| v.as_f64())?;
    // 102.3 is AIS's "not available" sentinel for speed
    if n >= 102.3 {
        None
    } else {
        Some(n)
    }
}

fn extract_cog(inner: &serde_json::Value) -> Option<f64> {
    let n = inner.get("Cog").and_then(|v| v.as_f64())?;
    // 360.0 is AIS's "not available" sentinel for course
    if n >= 360.0 {
        None
    } else {
        Some(n)
    }
}

fn extract_heading(inner: &serde_json::Value) -> Option<f64> {
    let n = inner.get("TrueHeading").and_then(|v| v.as_f64())?;
    // 511 is AIS's "not available" sentinel for true heading
    if n >= 511.0 {
        None
    } else {
        Some(n)
    }
}

fn ship_name_from_static(inner: &serde_json::Value) -> Option<String> {
    inner
        .get("Name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().trim_end_matches('@').trim().to_string())
        .filter(|s| !s.is_empty())
}

fn ship_type_from_static(inner: &serde_json::Value) -> Option<String> {
    let code = inner.get("ShipType").and_then(|v| v.as_u64())?;
    Some(ship_type_label(code as u32).to_string())
}

/// Map the AIS numeric ship-type code (per ITU-R M.1371) to a human label.
/// Coarse-grained — just enough for a "vessel type" column on the map.
pub fn ship_type_label(code: u32) -> &'static str {
    match code {
        0 => "Not Available",
        1..=19 => "Reserved",
        20..=29 => "WIG",
        30 => "Fishing",
        31..=32 => "Towing",
        33 => "Dredging",
        34 => "Diving Ops",
        35 => "Military Ops",
        36 => "Sailing",
        37 => "Pleasure Craft",
        38..=39 => "Reserved",
        40..=49 => "High Speed Craft",
        50 => "Pilot",
        51 => "Search and Rescue",
        52 => "Tug",
        53 => "Port Tender",
        54 => "Anti-Pollution",
        55 => "Law Enforcement",
        56..=59 => "Other (Special)",
        60..=69 => "Passenger",
        70..=79 => "Cargo",
        80..=89 => "Tanker",
        90..=99 => "Other",
        _ => "Unknown",
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_POSITION_REPORT: &str = r#"{
        "MessageType": "PositionReport",
        "Metadata": {
            "MMSI": 259000420,
            "ShipName": "SHIP_NAME",
            "latitude": 66.02695,
            "longitude": 12.253821666666665,
            "time_utc": "2022-12-29 18:22:32.318353 +0000 UTC"
        },
        "Message": {
            "PositionReport": {
                "UserID": 259000420,
                "Latitude": 66.02695,
                "Longitude": 12.253821666666665,
                "Sog": 12.5,
                "Cog": 308.2,
                "TrueHeading": 235,
                "Timestamp": 31
            }
        }
    }"#;

    #[test]
    fn parse_position_report_extracts_canonical_fields() {
        let parsed = parse_message(SAMPLE_POSITION_REPORT).unwrap();
        match parsed {
            AisParsed::Position(p) => {
                assert_eq!(p.mmsi, "259000420");
                assert!((p.lat - 66.02695).abs() < 1e-6);
                assert!((p.lon - 12.253821).abs() < 1e-3);
                assert_eq!(p.speed_kts, Some(12.5));
                assert_eq!(p.course_deg, Some(308.2));
                assert_eq!(p.heading_deg, Some(235.0));
                assert_eq!(p.ship_name.as_deref(), Some("SHIP_NAME"));
            }
            _ => panic!("expected Position"),
        }
    }

    #[test]
    fn parse_skips_unknown_message_types() {
        let raw = r#"{
            "MessageType": "BaseStationReport",
            "Metadata": {"MMSI": 1, "latitude": 0, "longitude": 0, "time_utc": ""},
            "Message": {"BaseStationReport": {}}
        }"#;
        assert!(parse_message(raw).is_none());
    }

    #[test]
    fn parse_returns_none_on_malformed_json() {
        assert!(parse_message("not json at all").is_none());
        assert!(parse_message("{}").is_none());
        assert!(parse_message(r#"{"MessageType": "PositionReport"}"#).is_none());
    }

    #[test]
    fn parse_returns_none_when_mmsi_missing() {
        let raw = r#"{
            "MessageType": "PositionReport",
            "Metadata": {"latitude": 0, "longitude": 0, "time_utc": ""},
            "Message": {"PositionReport": {"Latitude": 0, "Longitude": 0}}
        }"#;
        assert!(parse_message(raw).is_none());
    }

    #[test]
    fn parse_class_b_position_report() {
        let raw = r#"{
            "MessageType": "StandardClassBPositionReport",
            "Metadata": {
                "MMSI": 367000001,
                "ShipName": "BAYSIDE",
                "latitude": 32.7,
                "longitude": -79.9,
                "time_utc": ""
            },
            "Message": {
                "StandardClassBPositionReport": {
                    "Latitude": 32.7,
                    "Longitude": -79.9,
                    "Sog": 4.2,
                    "Cog": 90.0,
                    "TrueHeading": 90
                }
            }
        }"#;
        let parsed = parse_message(raw).unwrap();
        match parsed {
            AisParsed::Position(p) => {
                assert_eq!(p.mmsi, "367000001");
                assert_eq!(p.speed_kts, Some(4.2));
            }
            _ => panic!("expected Position from Class B report"),
        }
    }

    #[test]
    fn parse_extended_class_b_position_report() {
        let raw = r#"{
            "MessageType": "ExtendedClassBPositionReport",
            "Metadata": {"MMSI": 367000002, "latitude": 33.0, "longitude": -80.0, "time_utc": ""},
            "Message": {
                "ExtendedClassBPositionReport": {
                    "Latitude": 33.0,
                    "Longitude": -80.0,
                    "Sog": 8.0,
                    "Cog": 180.0,
                    "TrueHeading": 511
                }
            }
        }"#;
        let parsed = parse_message(raw).unwrap();
        if let AisParsed::Position(p) = parsed {
            // 511 is the "not available" sentinel; should map to None
            assert_eq!(p.heading_deg, None);
        } else {
            panic!("expected Position");
        }
    }

    #[test]
    fn position_drops_not_available_sentinels() {
        let raw = r#"{
            "MessageType": "PositionReport",
            "Metadata": {"MMSI": 1, "latitude": 0, "longitude": 0, "time_utc": ""},
            "Message": {
                "PositionReport": {
                    "Latitude": 0.0,
                    "Longitude": 0.0,
                    "Sog": 102.3,
                    "Cog": 360.0,
                    "TrueHeading": 511
                }
            }
        }"#;
        let parsed = parse_message(raw).unwrap();
        if let AisParsed::Position(p) = parsed {
            assert!(p.speed_kts.is_none());
            assert!(p.course_deg.is_none());
            assert!(p.heading_deg.is_none());
        } else {
            panic!("expected Position");
        }
    }

    #[test]
    fn position_rejects_invalid_lat_lon() {
        let raw = r#"{
            "MessageType": "PositionReport",
            "Metadata": {"MMSI": 1, "latitude": 999, "longitude": -200, "time_utc": ""},
            "Message": {"PositionReport": {"Latitude": 999, "Longitude": -200}}
        }"#;
        assert!(parse_message(raw).is_none());
    }

    #[test]
    fn parse_ship_static_data() {
        let raw = r#"{
            "MessageType": "ShipStaticData",
            "Metadata": {"MMSI": 259000420, "latitude": 0, "longitude": 0, "time_utc": ""},
            "Message": {
                "ShipStaticData": {
                    "Name": "ATLANTIC PIONEER@@@@@",
                    "ShipType": 70,
                    "Destination": "CHARLESTON"
                }
            }
        }"#;
        let parsed = parse_message(raw).unwrap();
        match parsed {
            AisParsed::Static(s) => {
                assert_eq!(s.mmsi, "259000420");
                // Trailing @ symbols are AIS padding; should be stripped
                assert_eq!(s.name.as_deref(), Some("ATLANTIC PIONEER"));
                assert_eq!(s.vessel_type.as_deref(), Some("Cargo"));
            }
            _ => panic!("expected Static"),
        }
    }

    #[test]
    fn ship_name_padding_trimmed_in_metadata_too() {
        let raw = r#"{
            "MessageType": "PositionReport",
            "Metadata": {"MMSI": 1, "ShipName": "TEST@@@@@", "latitude": 0, "longitude": 0, "time_utc": ""},
            "Message": {"PositionReport": {"Latitude": 0, "Longitude": 0}}
        }"#;
        let parsed = parse_message(raw).unwrap();
        if let AisParsed::Position(p) = parsed {
            assert_eq!(p.ship_name.as_deref(), Some("TEST"));
        } else {
            panic!("expected Position");
        }
    }

    #[test]
    fn ship_type_label_covers_main_categories() {
        assert_eq!(ship_type_label(30), "Fishing");
        assert_eq!(ship_type_label(36), "Sailing");
        assert_eq!(ship_type_label(60), "Passenger");
        assert_eq!(ship_type_label(70), "Cargo");
        assert_eq!(ship_type_label(80), "Tanker");
        assert_eq!(ship_type_label(89), "Tanker"); // upper boundary of Tanker
        assert_eq!(ship_type_label(100), "Unknown");
        assert_eq!(ship_type_label(0), "Not Available");
        assert_eq!(ship_type_label(52), "Tug");
        assert_eq!(ship_type_label(51), "Search and Rescue");
    }
}
