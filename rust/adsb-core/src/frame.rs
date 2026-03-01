//! Parse raw hex strings into structured Mode S frames.
//!
//! Responsibilities:
//! - Classify Downlink Format (DF) from first 5 bits
//! - Extract ICAO address (bytes 1-3 for DF11/17/18, or from CRC residual)
//! - Package into `ModeFrame`
//! - Reject frames that fail CRC validation
//! - Attempt 1-2 bit error correction on CRC failures
//! - Validate residual-recovered ICAOs against a time-windowed cache

use std::collections::HashMap;

use crate::crc;
use crate::types::{df_info, hex_decode, Icao};

// DFs where ICAO is explicit in bytes 1-3
const DF_EXPLICIT_ICAO: &[u8] = &[11, 17, 18];

// DFs where ICAO is recovered from CRC residual
const DF_RESIDUAL_ICAO: &[u8] = &[0, 4, 5, 16, 20, 21];

// ---------------------------------------------------------------------------
// ICAO cache
// ---------------------------------------------------------------------------

/// Time-windowed cache of validated ICAO addresses.
///
/// ICAOs are registered when seen in DF11/17/18 frames (explicit, CRC-validated).
/// For DF0/4/5/16/20/21, the ICAO is recovered from the CRC residual — noise
/// produces fake addresses. The cache rejects residual-recovered ICAOs not
/// recently seen in a validated frame.
pub struct IcaoCache {
    ttl: f64,
    cache: HashMap<Icao, f64>, // icao -> last_seen timestamp
}

impl IcaoCache {
    pub fn new(ttl: f64) -> Self {
        IcaoCache {
            ttl,
            cache: HashMap::new(),
        }
    }

    /// Register a validated ICAO (from DF11/17/18).
    pub fn register(&mut self, icao: Icao, timestamp: f64) {
        self.cache.insert(icao, timestamp);
    }

    /// Check if an ICAO was recently seen in a validated frame.
    pub fn is_known(&mut self, icao: &Icao, timestamp: f64) -> bool {
        if let Some(&last_seen) = self.cache.get(icao) {
            if timestamp - last_seen <= self.ttl {
                return true;
            }
            self.cache.remove(icao);
        }
        false
    }

    /// Remove expired entries.
    pub fn prune(&mut self, now: f64) {
        let ttl = self.ttl;
        self.cache.retain(|_, &mut last_seen| now - last_seen <= ttl);
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

impl Default for IcaoCache {
    fn default() -> Self {
        IcaoCache::new(60.0)
    }
}

// ---------------------------------------------------------------------------
// ModeFrame
// ---------------------------------------------------------------------------

/// A parsed Mode S frame.
#[derive(Debug, Clone)]
pub struct ModeFrame {
    /// Downlink Format (0-24)
    pub df: u8,
    /// 3-byte ICAO address
    pub icao: Icao,
    /// Full message bytes
    pub raw: Vec<u8>,
    /// Unix timestamp
    pub timestamp: f64,
    /// Signal strength if available
    pub signal_level: Option<f64>,
    /// Message length in bits (56 or 112)
    pub msg_bits: usize,
    /// CRC validation passed
    pub crc_ok: bool,
    /// True if error correction was applied
    pub corrected: bool,
}

impl ModeFrame {
    /// Human-readable Downlink Format name.
    pub fn df_name(&self) -> &'static str {
        df_info(self.df)
            .map(|info| info.name)
            .unwrap_or("Unknown")
    }

    /// True if this is an ADS-B extended squitter (DF17).
    pub fn is_adsb(&self) -> bool {
        self.df == 17
    }

    /// True if this is a 112-bit (long) message.
    pub fn is_long(&self) -> bool {
        self.msg_bits == 112
    }

    /// Message Extended field (bytes 4-10, 56 bits) for DF17/18.
    /// Returns empty slice for short frames.
    pub fn me(&self) -> &[u8] {
        if self.is_long() && self.raw.len() >= 11 {
            &self.raw[4..11]
        } else {
            &[]
        }
    }

    /// ADS-B Type Code (first 5 bits of ME field). None for non-ADS-B.
    pub fn type_code(&self) -> Option<u8> {
        if (self.df != 17 && self.df != 18) || !self.is_long() {
            return None;
        }
        if self.raw.len() < 5 {
            return None;
        }
        Some((self.raw[4] >> 3) & 0x1F)
    }
}

// ---------------------------------------------------------------------------
// Frame parsing
// ---------------------------------------------------------------------------

/// Parse a hex string into a ModeFrame.
///
/// `validate_icao`: if true, reject residual-recovered ICAOs not in cache.
pub fn parse_frame(
    hex_str: &str,
    timestamp: f64,
    signal_level: Option<f64>,
    validate_icao: bool,
    icao_cache: &mut IcaoCache,
) -> Option<ModeFrame> {
    let hex_str = hex_str.trim();

    // Validate length: 14 hex chars (56 bits) or 28 hex chars (112 bits)
    if hex_str.len() != 14 && hex_str.len() != 28 {
        return None;
    }

    let raw = hex_decode(hex_str)?;
    let msg_bits = raw.len() * 8;
    let df = (raw[0] >> 3) & 0x1F;

    // Check if DF is recognized
    let info = df_info(df)?;

    // Validate message length matches expected for this DF
    if msg_bits != info.bits {
        return None;
    }

    let crc_remainder = crc::crc24(&raw);
    let mut corrected = false;
    let mut raw = raw;

    // Extract ICAO address
    let (icao, crc_ok) = if DF_EXPLICIT_ICAO.contains(&df) {
        let mut crc_ok = crc_remainder == 0;

        // Attempt error correction for DF17/18 if CRC fails
        if !crc_ok && (df == 17 || df == 18) {
            let hex_upper = hex_str.to_uppercase();
            if let Some(fixed_hex) = crc::try_fix(&hex_upper) {
                if let Some(fixed_raw) = hex_decode(&fixed_hex) {
                    raw = fixed_raw;
                    crc_ok = true;
                    corrected = true;
                }
            }
        }

        // Extract ICAO (possibly from corrected raw bytes)
        let icao: Icao = [raw[1], raw[2], raw[3]];
        if crc_ok && validate_icao {
            icao_cache.register(icao, timestamp);
        }
        (icao, crc_ok)
    } else if DF_RESIDUAL_ICAO.contains(&df) {
        let icao: Icao = [
            ((crc_remainder >> 16) & 0xFF) as u8,
            ((crc_remainder >> 8) & 0xFF) as u8,
            (crc_remainder & 0xFF) as u8,
        ];

        // Validate against ICAO cache if enabled
        if validate_icao && !icao_cache.is_known(&icao, timestamp) {
            return None;
        }

        (icao, true)
    } else {
        return None;
    };

    Some(ModeFrame {
        df,
        icao,
        raw,
        timestamp,
        signal_level,
        msg_bits,
        crc_ok,
        corrected,
    })
}

/// Parse a hex string without ICAO cache validation.
/// Convenience for decoding standalone frames (e.g., from test vectors).
pub fn parse_frame_uncached(
    hex_str: &str,
    timestamp: f64,
    signal_level: Option<f64>,
) -> Option<ModeFrame> {
    let mut cache = IcaoCache::new(60.0);
    parse_frame(hex_str, timestamp, signal_level, false, &mut cache)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{hex_decode, hex_encode, icao_to_string};

    #[test]
    fn test_parse_df17_identification() {
        let frame = parse_frame_uncached("8D4840D6202CC371C32CE0576098", 1.0, None);
        assert!(frame.is_some());
        let frame = frame.unwrap();
        assert_eq!(frame.df, 17);
        assert_eq!(icao_to_string(&frame.icao), "4840D6");
        assert!(frame.crc_ok);
        assert!(!frame.corrected);
        assert_eq!(frame.msg_bits, 112);
        assert!(frame.is_adsb());
        assert!(frame.is_long());
    }

    #[test]
    fn test_parse_df17_position() {
        let frame =
            parse_frame_uncached("8D40621D58C382D690C8AC2863A7", 1.0, None).unwrap();
        assert_eq!(frame.df, 17);
        assert_eq!(icao_to_string(&frame.icao), "40621D");
        assert!(frame.crc_ok);

        // TC should be 11 (airborne position with barometric altitude)
        let tc = frame.type_code().unwrap();
        assert!(tc >= 9 && tc <= 18, "TC={tc} should be airborne position");
    }

    #[test]
    fn test_parse_df17_velocity() {
        let frame =
            parse_frame_uncached("8D485020994409940838175B284F", 1.0, None).unwrap();
        assert_eq!(frame.df, 17);
        assert_eq!(icao_to_string(&frame.icao), "485020");
        assert_eq!(frame.type_code(), Some(19));
    }

    #[test]
    fn test_parse_invalid_length() {
        assert!(parse_frame_uncached("8D4840D6", 0.0, None).is_none());
        assert!(parse_frame_uncached("", 0.0, None).is_none());
    }

    #[test]
    fn test_parse_invalid_hex() {
        assert!(parse_frame_uncached("ZZZZZZZZZZZZZZ", 0.0, None).is_none());
    }

    #[test]
    fn test_me_field() {
        let frame =
            parse_frame_uncached("8D4840D6202CC371C32CE0576098", 1.0, None).unwrap();
        let me = frame.me();
        assert_eq!(me.len(), 7); // 56 bits = 7 bytes
    }

    #[test]
    fn test_type_code_identification() {
        let frame =
            parse_frame_uncached("8D4840D6202CC371C32CE0576098", 1.0, None).unwrap();
        let tc = frame.type_code().unwrap();
        assert!(tc >= 1 && tc <= 4, "TC={tc} should be identification");
    }

    #[test]
    fn test_icao_cache() {
        let mut cache = IcaoCache::new(60.0);
        let icao = [0x48, 0x40, 0xD6];

        assert!(!cache.is_known(&icao, 0.0));

        cache.register(icao, 1.0);
        assert!(cache.is_known(&icao, 2.0));

        // After TTL expires
        assert!(!cache.is_known(&icao, 62.0));
    }

    #[test]
    fn test_icao_cache_prune() {
        let mut cache = IcaoCache::new(10.0);
        cache.register([0x01, 0x02, 0x03], 0.0);
        cache.register([0x04, 0x05, 0x06], 5.0);

        assert_eq!(cache.len(), 2);
        cache.prune(12.0);
        assert_eq!(cache.len(), 1); // First entry expired
    }

    #[test]
    fn test_parse_with_icao_validation() {
        let mut cache = IcaoCache::new(60.0);

        // DF17 should succeed without prior cache entry (explicit ICAO)
        let frame = parse_frame(
            "8D4840D6202CC371C32CE0576098",
            1.0,
            None,
            true,
            &mut cache,
        );
        assert!(frame.is_some());

        // ICAO should now be in cache
        assert!(cache.is_known(&[0x48, 0x40, 0xD6], 2.0));
    }

    #[test]
    fn test_error_correction() {
        // Corrupt a bit in a valid frame (bit 40, well past DF field)
        let mut data = hex_decode("8D4840D6202CC371C32CE0576098").unwrap();
        data[5] ^= 0x01;
        let corrupted = hex_encode(&data);

        let frame = parse_frame_uncached(&corrupted, 1.0, None);
        assert!(frame.is_some(), "Error correction should fix single-bit error");
        let frame = frame.unwrap();
        assert!(frame.crc_ok);
        assert!(frame.corrected);
    }
}
