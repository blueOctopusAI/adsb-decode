//! Shared types, error enum, and decoded message types for adsb-core.

use serde::Serialize;
use thiserror::Error;

/// All errors produced by adsb-core.
#[derive(Debug, Error)]
pub enum AdsbError {
    #[error("invalid hex string: {0}")]
    InvalidHex(String),
    #[error("invalid frame length: expected {expected} bits, got {actual}")]
    InvalidLength { expected: usize, actual: usize },
    #[error("unrecognized downlink format: {0}")]
    UnknownDf(u8),
    #[error("CRC validation failed")]
    CrcFailed,
    #[error("CPR decode failed: {0}")]
    CprFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, AdsbError>;

// ---------------------------------------------------------------------------
// Downlink Format metadata
// ---------------------------------------------------------------------------

/// Metadata for a Downlink Format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DfInfo {
    pub name: &'static str,
    pub bits: usize,
}

/// Known Downlink Format table.
pub const DF_TABLE: &[(u8, DfInfo)] = &[
    (
        0,
        DfInfo {
            name: "Short air-air surveillance",
            bits: 56,
        },
    ),
    (
        4,
        DfInfo {
            name: "Surveillance altitude reply",
            bits: 56,
        },
    ),
    (
        5,
        DfInfo {
            name: "Surveillance identity reply",
            bits: 56,
        },
    ),
    (
        11,
        DfInfo {
            name: "All-call reply",
            bits: 56,
        },
    ),
    (
        16,
        DfInfo {
            name: "Long air-air surveillance",
            bits: 112,
        },
    ),
    (
        17,
        DfInfo {
            name: "ADS-B extended squitter",
            bits: 112,
        },
    ),
    (
        18,
        DfInfo {
            name: "TIS-B / ADS-R",
            bits: 112,
        },
    ),
    (
        20,
        DfInfo {
            name: "Comm-B altitude reply",
            bits: 112,
        },
    ),
    (
        21,
        DfInfo {
            name: "Comm-B identity reply",
            bits: 112,
        },
    ),
];

/// Look up DF metadata. Returns `None` for unrecognized DFs.
pub fn df_info(df: u8) -> Option<&'static DfInfo> {
    DF_TABLE
        .iter()
        .find(|(d, _)| *d == df)
        .map(|(_, info)| info)
}

// ---------------------------------------------------------------------------
// ICAO address helpers
// ---------------------------------------------------------------------------

/// 3-byte ICAO address. Stored as raw bytes to avoid per-frame String allocation.
pub type Icao = [u8; 3];

/// Format ICAO address as 6-char uppercase hex string.
pub fn icao_to_string(icao: &Icao) -> String {
    format!("{:02X}{:02X}{:02X}", icao[0], icao[1], icao[2])
}

/// Parse a 6-char hex string into an ICAO address.
pub fn icao_from_hex(hex: &str) -> Option<Icao> {
    if hex.len() != 6 {
        return None;
    }
    let val = u32::from_str_radix(hex, 16).ok()?;
    Some([
        ((val >> 16) & 0xFF) as u8,
        ((val >> 8) & 0xFF) as u8,
        (val & 0xFF) as u8,
    ])
}

/// Convert ICAO bytes to u32 for numeric comparisons.
pub fn icao_to_u32(icao: &Icao) -> u32 {
    ((icao[0] as u32) << 16) | ((icao[1] as u32) << 8) | (icao[2] as u32)
}

/// Build ICAO from a 24-bit integer.
pub fn icao_from_u32(val: u32) -> Icao {
    [
        ((val >> 16) & 0xFF) as u8,
        ((val >> 8) & 0xFF) as u8,
        (val & 0xFF) as u8,
    ]
}

// ---------------------------------------------------------------------------
// Hex utilities
// ---------------------------------------------------------------------------

/// Decode a hex string into bytes. Case-insensitive, must be even length.
pub fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.trim();
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for chunk in hex.as_bytes().chunks(2) {
        let high = hex_digit(chunk[0])?;
        let low = hex_digit(chunk[1])?;
        bytes.push((high << 4) | low);
    }
    Some(bytes)
}

/// Encode bytes as uppercase hex string.
pub fn hex_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for &b in data {
        s.push(HEX_CHARS[(b >> 4) as usize] as char);
        s.push(HEX_CHARS[(b & 0x0F) as usize] as char);
    }
    s
}

const HEX_CHARS: &[u8; 16] = b"0123456789ABCDEF";

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// ADS-B callsign character set
// ---------------------------------------------------------------------------

/// ADS-B character set for callsign encoding (6 bits per character).
pub const CALLSIGN_CHARSET: &[u8; 64] =
    b"#ABCDEFGHIJKLMNOPQRSTUVWXYZ##### ###############0123456789######";

// ---------------------------------------------------------------------------
// Decoded message types
// ---------------------------------------------------------------------------

/// TC 1-4: Aircraft identification (callsign).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IdentificationMsg {
    pub icao: Icao,
    pub callsign: String,
    pub category: u8,
    pub timestamp: f64,
}

/// TC 5-8 (surface) or TC 9-18/20-22 (airborne): CPR-encoded position.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PositionMsg {
    pub icao: Icao,
    pub altitude_ft: Option<i32>,
    pub cpr_lat: u32,
    pub cpr_lon: u32,
    pub cpr_odd: bool,
    pub surveillance_status: u8,
    pub timestamp: f64,
    pub is_surface: bool,
}

/// TC 19: Airborne velocity.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VelocityMsg {
    pub icao: Icao,
    pub speed_kts: Option<f64>,
    pub heading_deg: Option<f64>,
    pub vertical_rate_fpm: Option<i32>,
    pub speed_type: SpeedType,
    pub timestamp: f64,
}

/// Speed type for velocity messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SpeedType {
    Ground,
    IAS,
    TAS,
}

impl std::fmt::Display for SpeedType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpeedType::Ground => write!(f, "ground"),
            SpeedType::IAS => write!(f, "IAS"),
            SpeedType::TAS => write!(f, "TAS"),
        }
    }
}

/// DF0/4/16/20: Altitude reply.
///
/// `comm_b` is populated only for DF20 long replies and only when one BDS
/// register dominates the plausibility scoring. ADS-B-equipped aircraft
/// rarely reply to selective interrogation, so this field stays None most
/// of the time.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AltitudeMsg {
    pub icao: Icao,
    pub altitude_ft: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comm_b: Option<CommBRegister>,
    pub timestamp: f64,
}

/// DF5/21: Identity reply (squawk code).
///
/// `comm_b` is populated only for DF21 long replies. See `AltitudeMsg`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SquawkMsg {
    pub icao: Icao,
    pub squawk: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comm_b: Option<CommBRegister>,
    pub timestamp: f64,
}

/// Mode S Comm-B register. The 56-bit MB field of a DF20/DF21 long reply
/// contains data from one of these registers; the variant indicates which
/// the plausibility identifier picked.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "register")]
pub enum CommBRegister {
    /// BDS 4,0 — Selected Vertical Intention. Autopilot setting data not in ADS-B.
    Bds40(Bds40),
    /// BDS 5,0 — Track and Turn Report. True airspeed + roll angle, neither in ADS-B.
    Bds50(Bds50),
    /// BDS 6,0 — Heading and Speed Report. Magnetic heading + IAS + Mach.
    Bds60(Bds60),
}

/// BDS 4,0 — Selected Vertical Intention payload.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Bds40 {
    pub mcp_altitude_ft: Option<i32>,
    pub fms_altitude_ft: Option<i32>,
    pub baro_setting_mb: Option<f64>,
}

/// BDS 5,0 — Track and Turn Report payload.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Bds50 {
    pub roll_deg: Option<f64>,
    pub true_track_deg: Option<f64>,
    pub ground_speed_kts: Option<u32>,
    pub track_rate_dps: Option<f64>,
    pub true_airspeed_kts: Option<u32>,
}

/// BDS 6,0 — Heading and Speed Report payload.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Bds60 {
    pub magnetic_heading_deg: Option<f64>,
    pub indicated_airspeed_kts: Option<u32>,
    pub mach: Option<f64>,
    pub baro_vertical_rate_fpm: Option<i32>,
    pub inertial_vertical_rate_fpm: Option<i32>,
}

/// Union type for all decoded messages.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum DecodedMsg {
    Identification(IdentificationMsg),
    Position(PositionMsg),
    Velocity(VelocityMsg),
    Altitude(AltitudeMsg),
    Squawk(SquawkMsg),
}

impl DecodedMsg {
    /// Get the ICAO address from any message type.
    pub fn icao(&self) -> &Icao {
        match self {
            DecodedMsg::Identification(m) => &m.icao,
            DecodedMsg::Position(m) => &m.icao,
            DecodedMsg::Velocity(m) => &m.icao,
            DecodedMsg::Altitude(m) => &m.icao,
            DecodedMsg::Squawk(m) => &m.icao,
        }
    }

    /// Get the timestamp from any message type.
    pub fn timestamp(&self) -> f64 {
        match self {
            DecodedMsg::Identification(m) => m.timestamp,
            DecodedMsg::Position(m) => m.timestamp,
            DecodedMsg::Velocity(m) => m.timestamp,
            DecodedMsg::Altitude(m) => m.timestamp,
            DecodedMsg::Squawk(m) => m.timestamp,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icao_roundtrip() {
        let icao = icao_from_hex("4840D6").unwrap();
        assert_eq!(icao, [0x48, 0x40, 0xD6]);
        assert_eq!(icao_to_string(&icao), "4840D6");
    }

    #[test]
    fn test_icao_to_u32() {
        let icao = [0xA0, 0x00, 0x01];
        assert_eq!(icao_to_u32(&icao), 0xA00001);
    }

    #[test]
    fn test_icao_from_u32() {
        assert_eq!(icao_from_u32(0x4840D6), [0x48, 0x40, 0xD6]);
    }

    #[test]
    fn test_hex_decode() {
        assert_eq!(hex_decode("4840D6"), Some(vec![0x48, 0x40, 0xD6]));
        assert_eq!(hex_decode("odd"), None); // odd length
        assert_eq!(hex_decode("ZZZZ"), None); // invalid chars
    }

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex_encode(&[0x48, 0x40, 0xD6]), "4840D6");
    }

    #[test]
    fn test_df_info() {
        assert_eq!(df_info(17).unwrap().name, "ADS-B extended squitter");
        assert_eq!(df_info(17).unwrap().bits, 112);
        assert!(df_info(3).is_none());
    }
}
