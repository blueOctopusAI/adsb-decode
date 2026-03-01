//! CRC-24 validation for Mode S messages.
//!
//! ICAO standard polynomial: x^24 + x^23 + x^22 + ... + x^10 + x^3 + 1
//! Generator: 0xFFF409
//!
//! For DF17/18 (ADS-B): last 24 bits are pure CRC. Valid frames → remainder 0.
//! For DF0/4/5/16/20/21: last 24 bits are CRC XOR'd with ICAO address.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::types::{hex_decode, hex_encode, Icao};

const GENERATOR: u32 = 0xFFF409;

// ---------------------------------------------------------------------------
// CRC lookup table (compile-time)
// ---------------------------------------------------------------------------

const fn build_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u32) << 16;
        let mut bit = 0;
        while bit < 8 {
            if crc & 0x800000 != 0 {
                crc = (crc << 1) ^ GENERATOR;
            } else {
                crc <<= 1;
            }
            crc &= 0xFFFFFF;
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = build_crc_table();

// ---------------------------------------------------------------------------
// Core CRC functions
// ---------------------------------------------------------------------------

/// Mode S CRC-24 check.
///
/// Polynomial division of the first (n-3) bytes, then XOR with the last 3
/// bytes (PI/CRC field).
///
/// - DF17/18: returns 0 when valid.
/// - DF0/4/5/16/20/21: returns ICAO address.
pub fn crc24(data: &[u8]) -> u32 {
    if data.len() <= 3 {
        let mut val = 0u32;
        for &b in data {
            val = (val << 8) | b as u32;
        }
        return val & 0xFFFFFF;
    }

    let payload_len = data.len() - 3;
    let mut crc = 0u32;

    for &byte in &data[..payload_len] {
        crc = ((crc << 8) ^ CRC_TABLE[((crc >> 16) ^ byte as u32) as usize & 0xFF]) & 0xFFFFFF;
    }

    // XOR with PI field (last 3 bytes)
    crc ^= (data[payload_len] as u32) << 16
        | (data[payload_len + 1] as u32) << 8
        | data[payload_len + 2] as u32;
    crc
}

/// Pure CRC-24 polynomial division of all bytes.
/// Used internally for syndrome table building.
fn crc24_raw(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &byte in data {
        crc = ((crc << 8) ^ CRC_TABLE[((crc >> 16) ^ byte as u32) as usize & 0xFF]) & 0xFFFFFF;
    }
    crc
}

/// Compute CRC-24 of payload bytes (all except last 3).
pub fn crc24_payload(data: &[u8]) -> u32 {
    if data.len() <= 3 {
        return 0;
    }
    crc24_raw(&data[..data.len() - 3])
}

/// Validate a Mode S message (hex string). Returns true if CRC remainder is 0.
pub fn validate(msg_hex: &str) -> bool {
    match hex_decode(msg_hex) {
        Some(data) => crc24(&data) == 0,
        None => false,
    }
}

/// Get CRC residual of a full message.
///
/// For DF17/18: returns 0 if valid.
/// For DF0/4/5/16/20/21: returns the ICAO address.
pub fn residual(msg_hex: &str) -> Option<u32> {
    hex_decode(msg_hex).map(|data| crc24(&data))
}

/// Extract ICAO address from a Mode S message hex string.
///
/// - DF11/17/18: ICAO is bytes 1-3 (explicit).
/// - DF0/4/5/16/20/21: ICAO recovered from CRC residual.
pub fn extract_icao(msg_hex: &str) -> Option<Icao> {
    let data = hex_decode(msg_hex)?;
    if data.is_empty() {
        return None;
    }
    let df = (data[0] >> 3) & 0x1F;

    match df {
        11 | 17 | 18 => {
            if data.len() < 4 {
                return None;
            }
            Some([data[1], data[2], data[3]])
        }
        0 | 4 | 5 | 16 | 20 | 21 => {
            let icao_val = crc24(&data);
            Some([
                ((icao_val >> 16) & 0xFF) as u8,
                ((icao_val >> 8) & 0xFF) as u8,
                (icao_val & 0xFF) as u8,
            ])
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Syndrome tables for error correction
// ---------------------------------------------------------------------------

fn build_syndrome_table(n_bits: usize) -> HashMap<u32, Vec<usize>> {
    let n_bytes = n_bits / 8;
    let mut table = HashMap::new();

    // Single-bit errors
    for bit in 0..n_bits {
        let mut msg = vec![0u8; n_bytes];
        msg[bit / 8] |= 1 << (7 - (bit % 8));
        let syndrome = crc24(&msg);
        table.entry(syndrome).or_insert_with(|| vec![bit]);
    }

    // Double-bit errors
    for bit1 in 0..n_bits {
        for bit2 in (bit1 + 1)..n_bits {
            let mut msg = vec![0u8; n_bytes];
            msg[bit1 / 8] |= 1 << (7 - (bit1 % 8));
            msg[bit2 / 8] |= 1 << (7 - (bit2 % 8));
            let syndrome = crc24(&msg);
            table.entry(syndrome).or_insert_with(|| vec![bit1, bit2]);
        }
    }

    table
}

static SYNDROME_TABLE_112: LazyLock<HashMap<u32, Vec<usize>>> =
    LazyLock::new(|| build_syndrome_table(112));
static SYNDROME_TABLE_56: LazyLock<HashMap<u32, Vec<usize>>> =
    LazyLock::new(|| build_syndrome_table(56));

/// Attempt to correct 1-2 bit errors in a Mode S message.
///
/// Looks up the CRC syndrome in pre-built tables. If found, flips the
/// identified bits and re-validates. Never corrects bits 0-4 (DF field)
/// to avoid turning one message type into another.
///
/// Returns corrected hex string if fixable, `None` otherwise.
pub fn try_fix(msg_hex: &str) -> Option<String> {
    let data = hex_decode(msg_hex)?;
    let n_bits = data.len() * 8;
    let syndrome = crc24(&data);

    if syndrome == 0 {
        return Some(msg_hex.to_uppercase());
    }

    let table = if n_bits == 112 {
        &*SYNDROME_TABLE_112
    } else {
        &*SYNDROME_TABLE_56
    };

    let bit_positions = table.get(&syndrome)?;

    // Safety: never correct the DF field (bits 0-4)
    if bit_positions.iter().any(|&b| b < 5) {
        return None;
    }

    // Flip the identified bits
    let mut fixed = data;
    for &bit in bit_positions {
        fixed[bit / 8] ^= 1 << (7 - (bit % 8));
    }

    // Verify the fix actually works
    if crc24(&fixed) != 0 {
        return None;
    }

    Some(hex_encode(&fixed))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Test vectors from known_frames.py
    const VALID_FRAMES: &[&str] = &[
        "8D4840D6202CC371C32CE0576098",
        "8D40621D58C382D690C8AC2863A7",
        "8D485020994409940838175B284F",
    ];

    #[test]
    fn test_crc_table_entry_zero() {
        assert_eq!(CRC_TABLE[0], 0);
    }

    #[test]
    fn test_crc_table_entry_one() {
        // First byte = 1: manual polynomial division
        // 0x010000 -> shift left 8 times with XOR
        assert_ne!(CRC_TABLE[1], 0);
    }

    #[test]
    fn test_valid_df17_remainder_zero() {
        for hex in VALID_FRAMES {
            let data = hex_decode(hex).unwrap();
            assert_eq!(crc24(&data), 0, "CRC should be 0 for valid DF17: {hex}");
        }
    }

    #[test]
    fn test_validate_hex() {
        for hex in VALID_FRAMES {
            assert!(validate(hex), "validate() should return true for: {hex}");
        }
    }

    #[test]
    fn test_validate_corrupted() {
        // Flip one bit in a valid frame
        let mut data = hex_decode(VALID_FRAMES[0]).unwrap();
        data[5] ^= 0x01;
        let corrupted = hex_encode(&data);
        assert!(!validate(&corrupted));
    }

    #[test]
    fn test_residual() {
        for hex in VALID_FRAMES {
            assert_eq!(residual(hex), Some(0));
        }
    }

    #[test]
    fn test_extract_icao_df17() {
        // "8D4840D6..." -> DF=17, ICAO=4840D6
        let icao = extract_icao("8D4840D6202CC371C32CE0576098").unwrap();
        assert_eq!(icao, [0x48, 0x40, 0xD6]);
    }

    #[test]
    fn test_extract_icao_df17_second() {
        let icao = extract_icao("8D40621D58C382D690C8AC2863A7").unwrap();
        assert_eq!(icao, [0x40, 0x62, 0x1D]);
    }

    #[test]
    fn test_crc24_payload() {
        let data = hex_decode(VALID_FRAMES[0]).unwrap();
        let payload_crc = crc24_payload(&data);
        // For DF17, payload CRC should equal the last 3 bytes
        let pi = (data[11] as u32) << 16 | (data[12] as u32) << 8 | data[13] as u32;
        assert_eq!(payload_crc, pi);
    }

    #[test]
    fn test_try_fix_already_valid() {
        let fixed = try_fix(VALID_FRAMES[0]).unwrap();
        assert_eq!(fixed, VALID_FRAMES[0]);
    }

    #[test]
    fn test_try_fix_single_bit_error() {
        // Corrupt bit 40 (byte 5, bit 0) — well past the DF field
        let mut data = hex_decode(VALID_FRAMES[0]).unwrap();
        data[5] ^= 0x01;
        let corrupted = hex_encode(&data);

        let fixed = try_fix(&corrupted);
        assert!(fixed.is_some(), "Should fix single-bit error");
        assert_eq!(fixed.unwrap(), VALID_FRAMES[0]);
    }

    #[test]
    fn test_try_fix_df_field_protection() {
        // Corrupt bit 0 (DF field) — should refuse to fix
        let mut data = hex_decode(VALID_FRAMES[0]).unwrap();
        data[0] ^= 0x80; // bit 0
        let corrupted = hex_encode(&data);

        assert!(try_fix(&corrupted).is_none());
    }

    #[test]
    fn test_syndrome_table_sizes() {
        // 112-bit: 112 single + C(112,2) double = 112 + 6216 = 6328 entries
        // (minus collisions)
        assert!(!SYNDROME_TABLE_112.is_empty());
        assert!(!SYNDROME_TABLE_56.is_empty());
        // Single-bit entries should exist for all bit positions
        assert!(SYNDROME_TABLE_112.len() > 100);
        assert!(SYNDROME_TABLE_56.len() > 50);
    }
}
