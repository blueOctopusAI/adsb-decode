//! Mode S Comm-B BDS register decoding (Enhanced Mode S, EHS).
//!
//! DF20/DF21 long Comm-B replies carry a 56-bit MB field that holds data from
//! one BDS (Binary Data Store) register. ADS-B itself doesn't carry these
//! fields — they only show up as responses to selective interrogation by
//! ground radar — but we receive them passively whenever we hear the reply.
//!
//! What this unlocks vs ADS-B alone:
//!
//! - **BDS 4,0** (Selected Vertical Intention) — the autopilot setting:
//!   MCP/FCU selected altitude, FMS selected altitude, barometric pressure
//!   setting. ADS-B never tells you what altitude the pilot is *targeting*.
//!
//! - **BDS 5,0** (Track and Turn Report) — true track angle, ground speed,
//!   track angle rate, **true airspeed**, roll angle. ADS-B velocity
//!   messages give ground velocity but not always TAS, and never roll.
//!
//! - **BDS 6,0** (Heading and Speed Report) — magnetic heading,
//!   indicated airspeed, mach number, barometric and inertial vertical
//!   rates. ADS-B gives heading-of-track-over-ground; this is true heading.
//!
//! Register identification is the hard problem: the MB field is 56 bits with
//! no register tag. We decode against each register and score by plausibility
//! (status bits set, values within physical bounds), returning the
//! highest-scoring register only if it clearly wins. Ambiguous frames return
//! None — better to drop than mislabel.
//!
//! Bit numbering throughout follows ICAO 9871 1-indexed-from-MSB. The
//! `bit()` and `bits()` helpers do the index conversion.

use crate::types::{Bds40, Bds50, Bds60, CommBRegister, Icao};

/// Extract a single bit. ICAO bit numbering is 1-based MSB-first.
fn bit(mb: &[u8], icao_bit: usize) -> bool {
    let zero_based = icao_bit - 1;
    let byte = zero_based / 8;
    let pos = 7 - (zero_based % 8);
    (mb[byte] >> pos) & 1 != 0
}

/// Extract `len` consecutive bits starting at ICAO bit `start_icao_bit`,
/// MSB-first, returned right-aligned in a u64.
fn bits(mb: &[u8], start_icao_bit: usize, len: usize) -> u64 {
    let mut out: u64 = 0;
    for i in 0..len {
        out = (out << 1) | (bit(mb, start_icao_bit + i) as u64);
    }
    out
}

/// Convert an unsigned bit pattern to a signed integer using two's complement.
fn signed(value: u64, width: u32) -> i64 {
    let max = 1u64 << width;
    if value >= max / 2 {
        (value as i64) - (max as i64)
    } else {
        value as i64
    }
}

/// Decode the MB field as BDS 4,0 (Selected Vertical Intention).
///
/// Layout (per ICAO 9871):
///
/// | ICAO bits | Field                                    |
/// |-----------|------------------------------------------|
/// | 1         | Status: MCP/FCU selected altitude        |
/// | 2-13      | MCP/FCU selected altitude (16 ft units)  |
/// | 14        | Status: FMS selected altitude            |
/// | 15-26     | FMS selected altitude (16 ft units)      |
/// | 27        | Status: barometric pressure setting      |
/// | 28-39     | Barometric pressure (0.1 mb above 800)   |
pub fn decode_bds40(mb: &[u8]) -> Bds40 {
    let mcp_altitude_ft = bit(mb, 1).then(|| (bits(mb, 2, 12) * 16) as i32);
    let fms_altitude_ft = bit(mb, 14).then(|| (bits(mb, 15, 12) * 16) as i32);
    let baro_setting_mb = bit(mb, 27).then(|| 800.0 + (bits(mb, 28, 12) as f64) * 0.1);
    Bds40 {
        mcp_altitude_ft,
        fms_altitude_ft,
        baro_setting_mb,
    }
}

/// Decode the MB field as BDS 5,0 (Track and Turn Report).
///
/// Layout:
///
/// | ICAO bits | Field                                    |
/// |-----------|------------------------------------------|
/// | 1         | Status: roll angle                       |
/// | 2         | Sign: roll angle (1 = negative)          |
/// | 3-11      | Roll angle (45/256 deg per LSB)          |
/// | 12        | Status: true track angle                 |
/// | 13        | Sign: true track angle                   |
/// | 14-23     | True track angle (90/512 deg per LSB)    |
/// | 24        | Status: ground speed                     |
/// | 25-34     | Ground speed (2 kt per LSB)              |
/// | 35        | Status: track angle rate                 |
/// | 36        | Sign: track angle rate                   |
/// | 37-45     | Track angle rate (8/256 deg/s per LSB)   |
/// | 46        | Status: true airspeed                    |
/// | 47-56     | True airspeed (2 kt per LSB)             |
pub fn decode_bds50(mb: &[u8]) -> Bds50 {
    let roll_deg = bit(mb, 1).then(|| {
        let raw = bits(mb, 2, 10);
        signed(raw, 10) as f64 * (45.0 / 256.0)
    });
    let true_track_deg = bit(mb, 12).then(|| {
        let raw = bits(mb, 13, 11);
        let val = signed(raw, 11) as f64 * (90.0 / 512.0);
        // Normalize to [0, 360).
        (val + 360.0) % 360.0
    });
    let ground_speed_kts = bit(mb, 24).then(|| (bits(mb, 25, 10) * 2) as u32);
    let track_rate_dps = bit(mb, 35).then(|| {
        let raw = bits(mb, 36, 10);
        signed(raw, 10) as f64 * (8.0 / 256.0)
    });
    let true_airspeed_kts = bit(mb, 46).then(|| (bits(mb, 47, 10) * 2) as u32);

    Bds50 {
        roll_deg,
        true_track_deg,
        ground_speed_kts,
        track_rate_dps,
        true_airspeed_kts,
    }
}

/// Decode the MB field as BDS 6,0 (Heading and Speed Report).
///
/// Layout:
///
/// | ICAO bits | Field                                    |
/// |-----------|------------------------------------------|
/// | 1         | Status: magnetic heading                 |
/// | 2         | Sign: magnetic heading                   |
/// | 3-12      | Magnetic heading (90/512 deg per LSB)    |
/// | 13        | Status: indicated airspeed               |
/// | 14-23     | IAS (1 kt per LSB)                       |
/// | 24        | Status: mach                             |
/// | 25-34     | Mach (2.048/512 per LSB)                 |
/// | 35        | Status: barometric vertical rate         |
/// | 36        | Sign: barometric vertical rate           |
/// | 37-45     | Baro vertical rate (32 fpm per LSB)      |
/// | 46        | Status: inertial vertical rate           |
/// | 47        | Sign: inertial vertical rate             |
/// | 48-56     | Inertial vertical rate (32 fpm per LSB)  |
pub fn decode_bds60(mb: &[u8]) -> Bds60 {
    let magnetic_heading_deg = bit(mb, 1).then(|| {
        let raw = bits(mb, 2, 11);
        let val = signed(raw, 11) as f64 * (90.0 / 512.0);
        (val + 360.0) % 360.0
    });
    let indicated_airspeed_kts = bit(mb, 13).then(|| bits(mb, 14, 10) as u32);
    let mach = bit(mb, 24).then(|| (bits(mb, 25, 10) as f64) * (2.048 / 512.0));
    let baro_vertical_rate_fpm = bit(mb, 35).then(|| {
        let raw = bits(mb, 36, 10);
        (signed(raw, 10) as i32) * 32
    });
    let inertial_vertical_rate_fpm = bit(mb, 46).then(|| {
        let raw = bits(mb, 47, 10);
        (signed(raw, 10) as i32) * 32
    });

    Bds60 {
        magnetic_heading_deg,
        indicated_airspeed_kts,
        mach,
        baro_vertical_rate_fpm,
        inertial_vertical_rate_fpm,
    }
}

/// Plausibility score for a BDS 4,0 decode. Higher = more likely this register.
fn score_bds40(d: &Bds40) -> i32 {
    let mut score = 0;
    if let Some(alt) = d.mcp_altitude_ft {
        score += if (0..=65520).contains(&alt) { 2 } else { -10 };
    }
    if let Some(alt) = d.fms_altitude_ft {
        score += if (0..=65520).contains(&alt) { 2 } else { -10 };
    }
    if let Some(p) = d.baro_setting_mb {
        score += if (800.0..=1200.0).contains(&p) {
            2
        } else {
            -10
        };
    }
    score
}

/// Plausibility score for a BDS 5,0 decode.
fn score_bds50(d: &Bds50) -> i32 {
    let mut score = 0;
    if let Some(r) = d.roll_deg {
        score += if r.abs() <= 60.0 { 2 } else { -10 };
    }
    if let Some(t) = d.true_track_deg {
        score += if (0.0..360.0).contains(&t) { 2 } else { -10 };
    }
    if let Some(g) = d.ground_speed_kts {
        score += if g <= 700 { 2 } else { -10 };
    }
    if let Some(rate) = d.track_rate_dps {
        score += if rate.abs() <= 16.0 { 2 } else { -10 };
    }
    if let Some(t) = d.true_airspeed_kts {
        score += if t <= 700 { 2 } else { -10 };
    }
    score
}

/// Plausibility score for a BDS 6,0 decode.
fn score_bds60(d: &Bds60) -> i32 {
    let mut score = 0;
    if let Some(h) = d.magnetic_heading_deg {
        score += if (0.0..360.0).contains(&h) { 2 } else { -10 };
    }
    if let Some(ias) = d.indicated_airspeed_kts {
        score += if ias <= 500 { 2 } else { -10 };
    }
    if let Some(m) = d.mach {
        score += if m <= 1.0 { 2 } else { -10 };
    }
    if let Some(vr) = d.baro_vertical_rate_fpm {
        score += if vr.abs() <= 6000 { 2 } else { -10 };
    }
    if let Some(vr) = d.inertial_vertical_rate_fpm {
        score += if vr.abs() <= 6000 { 2 } else { -10 };
    }
    score
}

/// Cross-consistency check: BDS 5,0 ground speed should match BDS 6,0 IAS
/// roughly. We don't have both registers in one frame, but we can flag a
/// frame as "likely BDS 5,0 alone" only if one register clearly dominates.
const MIN_DOMINANCE: i32 = 4;

/// Identify the most-plausible BDS register for a 56-bit MB field, returning
/// the decoded register variant. Returns None when the MB is empty (all
/// status bits clear) or no register dominates.
///
/// `_icao`: address of the originating aircraft, reserved for future
/// per-aircraft state tracking that lets us bias toward registers we've
/// recently seen from the same aircraft. Currently unused.
pub fn identify_comm_b(mb: &[u8], _icao: &Icao) -> Option<CommBRegister> {
    if mb.len() != 7 {
        return None;
    }

    let bds40 = decode_bds40(mb);
    let bds50 = decode_bds50(mb);
    let bds60 = decode_bds60(mb);

    let s40 = score_bds40(&bds40);
    let s50 = score_bds50(&bds50);
    let s60 = score_bds60(&bds60);

    // All zero-status MB blocks score 0 and produce nothing useful.
    let max = s40.max(s50).max(s60);
    if max <= 0 {
        return None;
    }

    // The winning register must beat the runner-up by MIN_DOMINANCE; otherwise
    // the MB block is ambiguous (it decoded to plausible values under multiple
    // registers) and we drop the frame rather than guess.
    let mut scores = [s40, s50, s60];
    scores.sort_unstable();
    if scores[2] - scores[1] < MIN_DOMINANCE {
        return None;
    }

    if s40 == max {
        Some(CommBRegister::Bds40(bds40))
    } else if s50 == max {
        Some(CommBRegister::Bds50(bds50))
    } else {
        Some(CommBRegister::Bds60(bds60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 7-byte MB block from a bit-string of 0s and 1s. Spaces ignored.
    fn mb(bits_str: &str) -> [u8; 7] {
        let cleaned: String = bits_str.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(cleaned.len(), 56, "MB must be exactly 56 bits");
        let mut out = [0u8; 7];
        for (i, c) in cleaned.chars().enumerate() {
            if c == '1' {
                out[i / 8] |= 1 << (7 - (i % 8));
            }
        }
        out
    }

    #[test]
    fn bit_extract_msb_first() {
        let block = [0b1000_0000u8, 0, 0, 0, 0, 0, 0];
        assert!(bit(&block, 1));
        assert!(!bit(&block, 2));
        let block = [0, 0b0100_0000u8, 0, 0, 0, 0, 0];
        assert!(bit(&block, 10));
    }

    #[test]
    fn bits_extract_concatenated_msb_first() {
        // bits 1..=4 = 1010 = 10
        let block = [0b1010_0000u8, 0, 0, 0, 0, 0, 0];
        assert_eq!(bits(&block, 1, 4), 0b1010);
    }

    #[test]
    fn signed_sign_extension() {
        assert_eq!(signed(0b0000_0001, 8), 1);
        assert_eq!(signed(0b1111_1111, 8), -1);
        assert_eq!(signed(0b1000_0000, 8), -128);
        assert_eq!(signed(0b0111_1111, 8), 127);
    }

    #[test]
    fn bds40_decodes_mcp_altitude() {
        // Layout: 1 (status mcp) + 12 (mcp alt) + 1 (status fms) + 12 (fms alt) +
        //         1 (status baro) + 12 (baro) + 17 (reserved) = 56
        // Status mcp = 1, code 1875 (× 16 = 30,000 ft) = 011101010011
        let block = mb("1 011101010011 0 000000000000 0 000000000000 00000000000000000");
        let d = decode_bds40(&block);
        assert_eq!(d.mcp_altitude_ft, Some(30000));
        assert_eq!(d.fms_altitude_ft, None);
        assert_eq!(d.baro_setting_mb, None);
    }

    #[test]
    fn bds40_decodes_baro_setting() {
        // Status baro = 1, 12 bits = 132 → 800 + 13.2 = 813.2 mb
        // 132 = 000010000100
        let block = mb("0 000000000000 0 000000000000 1 000010000100 00000000000000000");
        let d = decode_bds40(&block);
        assert!((d.baro_setting_mb.unwrap() - 813.2).abs() < 0.05);
    }

    #[test]
    fn bds50_decodes_ground_speed() {
        // Layout: 1 (status roll) + 10 (roll) + 1 (status track) + 11 (track) +
        //         1 (status gs) + 10 (gs) + 1 (status rate) + 10 (rate) +
        //         1 (status tas) + 10 (tas) = 56
        // GS status = 1, 10 bits = 200 → 400 kt; 200 = 0011001000
        let block = mb("0 0000000000 \
             0 00000000000 \
             1 0011001000 \
             0 0000000000 \
             0 0000000000");
        let d = decode_bds50(&block);
        assert_eq!(d.ground_speed_kts, Some(400));
        assert_eq!(d.true_airspeed_kts, None);
    }

    #[test]
    fn bds50_decodes_negative_roll() {
        // Roll status = 1; raw 10-bit two's-complement = 1100000001 = unsigned 769.
        // signed(769, 10) = -255 → -255 * 45/256 = -44.82°
        let block = mb("1 1100000001 \
             0 00000000000 \
             0 0000000000 \
             0 0000000000 \
             0 0000000000");
        let d = decode_bds50(&block);
        let r = d.roll_deg.unwrap();
        assert!(r < 0.0 && (r + 44.82).abs() < 0.1, "got {r}");
    }

    #[test]
    fn bds60_decodes_mach_and_ias() {
        // IAS status=1, 10 bits = 250 → 250 kt
        // Mach status=1, 10 bits = 200 → 200 * 0.004 = 0.8
        // 250 = 0011111010, 200 = 0011001000
        let block = mb("0 00000000000 1 0011111010 1 0011001000 0 0000000000 0 0000000000");
        let d = decode_bds60(&block);
        assert_eq!(d.indicated_airspeed_kts, Some(250));
        let m = d.mach.unwrap();
        assert!((m - 0.8).abs() < 0.01, "got mach {m}");
    }

    #[test]
    fn bds60_decodes_negative_vertical_rate() {
        // Bit 35 status=1, bit 36 sign=1, bits 37-45 magnitude = 50
        // 50 = 000110010, two's-complement 10-bit: 1 000110010 = 562 → 562-1024 = -462 → * 32 = -14784
        // Wait that doesn't match. Let me recompute: 462 * 32 = 14784. Magnitude 50 = sign+magnitude form? No, ICAO says two's complement (sign+magnitude is rare in Mode S).
        // For negative VR, sign bit 1 means negative, BUT all 10 bits are interpreted as two's complement. So sign=1, magnitude=50 -> raw 10 bits = 1000110010 = 562 unsigned -> 562-1024 = -462 -> *32 = -14784 fpm. That's well outside plausible range, but let's see what the decoder does.
        // Better test: pick a value that round-trips cleanly.
        // -1024 fpm = -32 in 10-bit ts complement = 0b1111100000 = 992 unsigned. 992-1024 = -32. -32 * 32 = -1024.
        // 992 = 1111100000
        let block = mb("0 00000000000 0 0000000000 0 0000000000 1 1111100000 0 0000000000");
        let d = decode_bds60(&block);
        assert_eq!(d.baro_vertical_rate_fpm, Some(-1024));
    }

    #[test]
    fn identify_returns_none_for_empty_mb() {
        let block = [0u8; 7];
        assert!(identify_comm_b(&block, &[0xAA, 0xBB, 0xCC]).is_none());
    }

    #[test]
    fn identify_picks_bds50_for_track_data() {
        // BDS 5,0 with all five fields set to plausible values:
        // roll=20° (status=1, sign=0, mag = 20 * 256/45 ≈ 114, 9 bits = 001110010)
        // track=180° (status=1, sign=0, 10 bits = 180 * 512/90 = 1024, but that's 11 bits.
        //   180° wraps to 0° in 11-bit signed: 180/0.17578 ≈ 1024, but max is 1023.
        //   Use 90°: 90 * 512/90 = 512 = 01000000000 (11 bits)
        // gs=300 kt (status=1, 10 bits = 150 = 0010010110)
        // track rate=0 (status=1, sign=0, 10 bits = 0)
        // tas=400 kt (status=1, 10 bits = 200 = 0011001000)
        let block = mb("1 0001110010 \
             1 01000000000 \
             1 0010010110 \
             1 0000000000 \
             1 0011001000");
        let r = identify_comm_b(&block, &[0xAA, 0xBB, 0xCC]).unwrap();
        match r {
            CommBRegister::Bds50(d) => {
                assert_eq!(d.ground_speed_kts, Some(300));
                assert_eq!(d.true_airspeed_kts, Some(400));
                assert!((d.true_track_deg.unwrap() - 90.0).abs() < 0.5);
            }
            other => panic!("expected Bds50, got {other:?}"),
        }
    }

    #[test]
    fn identify_returns_none_for_short_mb() {
        let too_short = [0u8; 5];
        assert!(identify_comm_b(&too_short, &[0xAA, 0xBB, 0xCC]).is_none());
    }
}
