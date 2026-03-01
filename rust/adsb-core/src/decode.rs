//! Decode Mode S frames into typed aircraft messages.
//!
//! Handles all Downlink Formats and ADS-B Type Codes:
//! - DF17 TC 1-4:  Aircraft identification (callsign)
//! - DF17 TC 9-18: Airborne position (barometric alt + CPR-encoded lat/lon)
//! - DF17 TC 19:   Airborne velocity (ground speed or airspeed + heading)
//! - DF17 TC 20-22: Airborne position (GNSS altitude)
//! - DF4/20:       Surveillance/Comm-B altitude reply
//! - DF5/21:       Surveillance/Comm-B identity reply (squawk)
//! - DF11:         All-call reply (ICAO address acquisition)

use crate::frame::ModeFrame;
use crate::types::*;

// ---------------------------------------------------------------------------
// Altitude decoding
// ---------------------------------------------------------------------------

/// Decode 12-bit altitude code from DF17 airborne position.
///
/// The Q-bit (bit 4) selects the encoding mode:
/// - Q=1: 25-ft resolution
/// - Q=0: 100-ft Gillham gray code
pub fn decode_altitude(alt_code: u32) -> Option<i32> {
    if alt_code == 0 {
        return None;
    }

    let q_bit = (alt_code >> 4) & 1;

    if q_bit == 1 {
        // 25-ft resolution mode: remove Q-bit to get 11-bit code
        let n = ((alt_code >> 5) << 4) | (alt_code & 0x0F);
        Some(n as i32 * 25 - 1000)
    } else {
        decode_gillham_altitude(alt_code)
    }
}

/// Decode 100-ft Gillham gray code altitude.
///
/// Ported from dump1090's ModeA-to-ModeC conversion.
fn decode_gillham_altitude(alt_code: u32) -> Option<i32> {
    // Extract individual bits from interleaved positions
    let c1 = (alt_code >> 12) & 1;
    let a1 = (alt_code >> 11) & 1;
    let c2 = (alt_code >> 10) & 1;
    let a2 = (alt_code >> 9) & 1;
    let c4 = (alt_code >> 8) & 1;
    let a4 = (alt_code >> 7) & 1;
    // bit 6 = M (metric, should be 0)
    let b1 = (alt_code >> 5) & 1;
    // bit 4 = Q (should be 0 if we got here)
    let b2 = (alt_code >> 3) & 1;
    let d2 = (alt_code >> 2) & 1;
    let b4 = (alt_code >> 1) & 1;
    let d4 = alt_code & 1;
    let d1 = 0u32; // Not transmitted in Mode S

    // Mode A octal digits
    let _a_digit = a4 * 4 + a2 * 2 + a1;
    let _b_digit = b4 * 4 + b2 * 2 + b1;
    let c_digit = c4 * 4 + c2 * 2 + c1;
    let _d_digit = d4 * 4 + d2 * 2 + d1;

    // 100-ft component from C digit (Gray code)
    let mut c_bin = c_digit;
    c_bin ^= c_bin >> 2;
    c_bin ^= c_bin >> 1;

    if c_bin == 0 || c_bin == 6 || c_bin > 6 {
        return None;
    }

    // 500-ft component: Gray code from combined A and B digits
    let ab_gray = (a4 * 4 + a2 * 2 + a1) << 3 | (b4 * 4 + b2 * 2 + b1);
    let mut ab_bin = ab_gray;
    ab_bin ^= ab_bin >> 4;
    ab_bin ^= ab_bin >> 2;
    ab_bin ^= ab_bin >> 1;

    let altitude = ab_bin as i32 * 500 + c_bin as i32 * 100 - 1200;

    if !(-1200..=126750).contains(&altitude) {
        return None;
    }

    Some(altitude)
}

/// Decode 13-bit altitude code from DF0/4/16/20.
///
/// M-bit and Q-bit select the mode:
/// - M=0, Q=1: 25-ft increments
/// - M=0, Q=0: 100-ft Gillham gray code
/// - M=1: metric altitude (rare, not implemented)
pub fn decode_altitude_13bit(alt_code_13: u32) -> Option<i32> {
    if alt_code_13 == 0 {
        return None;
    }

    let m_bit = (alt_code_13 >> 6) & 1;
    let q_bit = (alt_code_13 >> 4) & 1;

    if m_bit == 1 {
        return None; // Metric altitude — very rare
    }

    if q_bit == 1 {
        // 25-ft mode: remove M and Q bits to get 11-bit code
        let n =
            ((alt_code_13 & 0x1F80) >> 2) | ((alt_code_13 & 0x0020) >> 1) | (alt_code_13 & 0x000F);
        Some(n as i32 * 25 - 1000)
    } else {
        decode_gillham_altitude(alt_code_13)
    }
}

// ---------------------------------------------------------------------------
// Squawk decoding
// ---------------------------------------------------------------------------

/// Decode 13-bit identity code into 4-digit octal squawk.
///
/// Bits are labeled C1 A1 C2 A2 C4 A4 _ B1 D1 B2 D2 B4 D4
pub fn decode_squawk(id_code: u32) -> String {
    let c1 = (id_code >> 12) & 1;
    let a1 = (id_code >> 11) & 1;
    let c2 = (id_code >> 10) & 1;
    let a2 = (id_code >> 9) & 1;
    let c4 = (id_code >> 8) & 1;
    let a4 = (id_code >> 7) & 1;
    // bit 6 is spare (SPI)
    let b1 = (id_code >> 5) & 1;
    let d1 = (id_code >> 4) & 1;
    let b2 = (id_code >> 3) & 1;
    let d2 = (id_code >> 2) & 1;
    let b4 = (id_code >> 1) & 1;
    let d4 = id_code & 1;

    let a = a4 * 4 + a2 * 2 + a1;
    let b = b4 * 4 + b2 * 2 + b1;
    let c = c4 * 4 + c2 * 2 + c1;
    let d = d4 * 4 + d2 * 2 + d1;

    format!("{a}{b}{c}{d}")
}

// ---------------------------------------------------------------------------
// Main decode functions
// ---------------------------------------------------------------------------

/// Decode TC 1-4: Aircraft identification (callsign).
pub fn decode_identification(frame: &ModeFrame) -> Option<IdentificationMsg> {
    let tc = frame.type_code()?;
    if !(1..=4).contains(&tc) {
        return None;
    }

    let me = frame.me();
    if me.len() < 7 {
        return None;
    }

    let category = me[0] & 0x07;

    // Decode 8 callsign characters (6 bits each, packed into 48 bits)
    let bits = u64::from_be_bytes({
        let mut buf = [0u8; 8];
        buf[1..8].copy_from_slice(me);
        buf
    });

    let mut callsign = String::with_capacity(8);
    for i in 0..8 {
        let idx = ((bits >> (42 - i * 6)) & 0x3F) as usize;
        if idx < CALLSIGN_CHARSET.len() {
            callsign.push(CALLSIGN_CHARSET[idx] as char);
        } else {
            callsign.push(' ');
        }
    }

    Some(IdentificationMsg {
        icao: frame.icao,
        callsign,
        category,
        timestamp: frame.timestamp,
    })
}

/// Decode TC 5-8 (surface) or TC 9-18/20-22 (airborne position).
pub fn decode_position(frame: &ModeFrame) -> Option<PositionMsg> {
    let tc = frame.type_code()?;

    let is_surface = (5..=8).contains(&tc);
    let is_airborne_baro = (9..=18).contains(&tc);
    let is_airborne_gnss = (20..=22).contains(&tc);

    if !is_surface && !is_airborne_baro && !is_airborne_gnss {
        return None;
    }

    let me = frame.me();
    if me.len() < 7 {
        return None;
    }

    let bits = u64::from_be_bytes({
        let mut buf = [0u8; 8];
        buf[1..8].copy_from_slice(me);
        buf
    });

    let ss = ((bits >> 49) & 0x03) as u8;

    let altitude_ft = if is_airborne_baro || is_airborne_gnss {
        let alt_code = ((bits >> 36) & 0x0FFF) as u32;
        decode_altitude(alt_code)
    } else {
        None
    };

    let cpr_odd = ((bits >> 34) & 1) == 1;
    let cpr_lat = ((bits >> 17) & 0x1FFFF) as u32;
    let cpr_lon = (bits & 0x1FFFF) as u32;

    Some(PositionMsg {
        icao: frame.icao,
        altitude_ft,
        cpr_lat,
        cpr_lon,
        cpr_odd,
        surveillance_status: ss,
        timestamp: frame.timestamp,
        is_surface,
    })
}

/// Decode TC 19: Airborne velocity.
pub fn decode_velocity(frame: &ModeFrame) -> Option<VelocityMsg> {
    if frame.type_code()? != 19 {
        return None;
    }

    let me = frame.me();
    if me.len() < 7 {
        return None;
    }

    let bits = u64::from_be_bytes({
        let mut buf = [0u8; 8];
        buf[1..8].copy_from_slice(me);
        buf
    });

    let subtype = ((bits >> 48) & 0x07) as u8;

    match subtype {
        1 | 2 => Some(decode_ground_velocity(frame.icao, bits, frame.timestamp)),
        3 | 4 => Some(decode_airspeed(frame.icao, bits, subtype, frame.timestamp)),
        _ => None,
    }
}

fn decode_ground_velocity(icao: Icao, bits: u64, timestamp: f64) -> VelocityMsg {
    let ew_dir = (bits >> 42) & 1; // 0=East, 1=West
    let ew_vel = ((bits >> 32) & 0x3FF) as i32 - 1;
    let ns_dir = (bits >> 31) & 1; // 0=North, 1=South
    let ns_vel = ((bits >> 21) & 0x3FF) as i32 - 1;

    let vr_sign = (bits >> 19) & 1; // 0=up, 1=down
    let vr_val = ((bits >> 10) & 0x1FF) as i32 - 1;

    let (speed, heading) = if ew_vel >= 0 && ns_vel >= 0 {
        let vx = if ew_dir == 1 { -ew_vel } else { ew_vel } as f64;
        let vy = if ns_dir == 1 { -ns_vel } else { ns_vel } as f64;
        let spd = (vx * vx + vy * vy).sqrt();
        let hdg = vx.atan2(vy).to_degrees().rem_euclid(360.0);
        (Some(round2(spd)), Some(round2(hdg)))
    } else {
        (None, None)
    };

    let vrate = if vr_val >= 0 {
        let rate = vr_val * 64;
        Some(if vr_sign == 1 { -rate } else { rate })
    } else {
        None
    };

    VelocityMsg {
        icao,
        speed_kts: speed,
        heading_deg: heading,
        vertical_rate_fpm: vrate,
        speed_type: SpeedType::Ground,
        timestamp,
    }
}

fn decode_airspeed(icao: Icao, bits: u64, _subtype: u8, timestamp: f64) -> VelocityMsg {
    let hdg_available = (bits >> 42) & 1;
    let hdg_raw = ((bits >> 32) & 0x3FF) as u32;

    let speed_type_bit = (bits >> 31) & 1; // 0=IAS, 1=TAS
    let speed_raw = ((bits >> 21) & 0x3FF) as i32;

    let vr_sign = (bits >> 10) & 1;
    let vr_val = ((bits >> 1) & 0x1FF) as i32 - 1;

    let heading = if hdg_available == 1 {
        Some(round2(hdg_raw as f64 * 360.0 / 1024.0))
    } else {
        None
    };

    let speed = if speed_raw > 0 {
        Some((speed_raw - 1) as f64)
    } else {
        None
    };

    let vrate = if vr_val >= 0 {
        let rate = vr_val * 64;
        Some(if vr_sign == 1 { -rate } else { rate })
    } else {
        None
    };

    VelocityMsg {
        icao,
        speed_kts: speed,
        heading_deg: heading,
        vertical_rate_fpm: vrate,
        speed_type: if speed_type_bit == 1 {
            SpeedType::TAS
        } else {
            SpeedType::IAS
        },
        timestamp,
    }
}

/// Decode DF0/4/16/20: altitude from surveillance replies.
pub fn decode_df_altitude(frame: &ModeFrame) -> Option<AltitudeMsg> {
    if !matches!(frame.df, 0 | 4 | 16 | 20) {
        return None;
    }

    if frame.raw.len() < 4 {
        return None;
    }

    let alt_code = ((frame.raw[2] as u32 & 0x1F) << 8) | frame.raw[3] as u32;
    let altitude_ft = decode_altitude_13bit(alt_code);

    Some(AltitudeMsg {
        icao: frame.icao,
        altitude_ft,
        timestamp: frame.timestamp,
    })
}

/// Decode DF5/21: identity (squawk) from surveillance replies.
pub fn decode_df_squawk(frame: &ModeFrame) -> Option<SquawkMsg> {
    if !matches!(frame.df, 5 | 21) {
        return None;
    }

    if frame.raw.len() < 4 {
        return None;
    }

    let id_code = ((frame.raw[2] as u32 & 0x1F) << 8) | frame.raw[3] as u32;
    let squawk = decode_squawk(id_code);

    Some(SquawkMsg {
        icao: frame.icao,
        squawk,
        timestamp: frame.timestamp,
    })
}

/// Decode any ModeFrame into the appropriate typed message.
///
/// Routes to the correct decoder based on DF and TC.
pub fn decode(frame: &ModeFrame) -> Option<DecodedMsg> {
    if !frame.crc_ok {
        return None;
    }

    match frame.df {
        17 | 18 => {
            let tc = frame.type_code()?;
            match tc {
                1..=4 => decode_identification(frame).map(DecodedMsg::Identification),
                5..=18 | 20..=22 => decode_position(frame).map(DecodedMsg::Position),
                19 => decode_velocity(frame).map(DecodedMsg::Velocity),
                _ => None,
            }
        }
        0 | 4 | 16 | 20 => decode_df_altitude(frame).map(DecodedMsg::Altitude),
        5 | 21 => decode_df_squawk(frame).map(DecodedMsg::Squawk),
        _ => None,
    }
}

/// Round to 2 decimal places.
fn round2(val: f64) -> f64 {
    (val * 100.0).round() / 100.0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::parse_frame_uncached;

    fn parse(hex: &str) -> ModeFrame {
        parse_frame_uncached(hex, 1.0, None).expect("valid frame")
    }

    // -- Identification --

    #[test]
    fn test_decode_identification_klm() {
        let frame = parse("8D4840D6202CC371C32CE0576098");
        let msg = decode_identification(&frame).unwrap();
        assert_eq!(msg.callsign, "KLM1023 ");
        assert_eq!(icao_to_string(&msg.icao), "4840D6");
    }

    #[test]
    fn test_decode_identification_ezy() {
        let frame = parse("8D406B902015A678D4D220AA4BDA");
        let msg = decode_identification(&frame).unwrap();
        assert_eq!(msg.callsign, "EZY85MH ");
        assert_eq!(icao_to_string(&msg.icao), "406B90");
    }

    // -- Position --

    #[test]
    fn test_decode_position_even() {
        let frame = parse("8D40621D58C382D690C8AC2863A7");
        let msg = decode_position(&frame).unwrap();
        assert_eq!(icao_to_string(&msg.icao), "40621D");
        assert_eq!(msg.altitude_ft, Some(38000));
        assert!(!msg.cpr_odd); // even frame
        assert_eq!(msg.cpr_lat, 93000);
        assert_eq!(msg.cpr_lon, 51372);
    }

    #[test]
    fn test_decode_position_odd() {
        let frame = parse("8D40621D58C386435CC412692AD6");
        let msg = decode_position(&frame).unwrap();
        assert_eq!(msg.altitude_ft, Some(38000));
        assert!(msg.cpr_odd); // odd frame
        assert_eq!(msg.cpr_lat, 74158);
        assert_eq!(msg.cpr_lon, 50194);
    }

    // -- Velocity --

    #[test]
    fn test_decode_velocity_ground() {
        let frame = parse("8D485020994409940838175B284F");
        let msg = decode_velocity(&frame).unwrap();
        assert_eq!(icao_to_string(&msg.icao), "485020");

        // Expected: speed≈159 kts, heading≈182.88°, vrate=-832
        let speed = msg.speed_kts.unwrap();
        assert!(
            (speed - 159.0).abs() < 1.0,
            "Speed should be ~159, got {speed}"
        );

        let heading = msg.heading_deg.unwrap();
        assert!(
            (heading - 182.88).abs() < 0.1,
            "Heading should be ~182.88, got {heading}"
        );

        assert_eq!(msg.vertical_rate_fpm, Some(-832));
        assert_eq!(msg.speed_type, SpeedType::Ground);
    }

    // -- Altitude --

    #[test]
    fn test_decode_altitude_25ft() {
        // Q-bit set: 25-ft resolution
        // alt_code = 0b_0000_0001_0001_0 = 0x011 with Q=1
        // n = 0b_0000_0001_000 | 0b_0001 = reconstructed without Q bit
        // The standard test: alt_code where Q=1 and n*25 - 1000 gives expected
        let alt = decode_altitude(0b_110000_1_0000); // Q-bit at position 4
        assert!(alt.is_some());
    }

    #[test]
    fn test_decode_altitude_zero() {
        assert_eq!(decode_altitude(0), None);
    }

    #[test]
    fn test_decode_altitude_13bit_zero() {
        assert_eq!(decode_altitude_13bit(0), None);
    }

    // -- Squawk --

    #[test]
    fn test_decode_altitude_25ft_exact_value() {
        // alt_code = 0xC38 (Q-bit set, n*25 - 1000 = 38000)
        // 0xC38 = 0b_110000_1_11000
        // Q-bit is at position 4 = 1 (set)
        // n = upper 7 bits (0b_110000_1 >> 5 << 4) | lower 4 (0b_1000) = (0x18 << 4) | 8 = 392
        // But let's compute: n = 0x30 << 4 | 0x8 = 0x308 = 776? No...
        // Actually: 38000 + 1000 = 39000. 39000 / 25 = 1560
        // 1560 in binary = 0b11000011000
        // Insert Q-bit at position 4: 0b110000_1_1000 = 0xC38? Let's verify
        // n = ((0xC38 >> 5) << 4) | (0xC38 & 0x0F) = (0x61 << 4) | 0x08 = 0x618 = 1560
        // altitude = 1560 * 25 - 1000 = 39000 - 1000 = 38000
        let alt = decode_altitude(0xC38);
        assert_eq!(alt, Some(38000));
    }

    #[test]
    fn test_decode_gillham_altitude() {
        // Test the Gillham (gray code) path: Q-bit = 0
        // Construct a code with Q-bit clear that produces a valid altitude
        // A=1,B=0,C=1: c_bin=1 (100ft), ab_bin=8 (4000ft) → 4000+100-1200 = 2900
        let alt_code = 0b_0_1_0_0_0_0_0_0_0_0_0_0_0u32; // A1=1, rest 0
        // This gives c_digit=0 which is invalid. Let's try a known working pattern.
        // C1=1: sets c_digit bit, making c_bin valid
        // A1=1, C1=1: alt_code = (C1<<12)|(A1<<11) = 0x1800
        let alt = decode_altitude(0x1800);
        assert!(alt.is_some(), "Valid Gillham code should decode");
        let val = alt.unwrap();
        assert!((-1200..=126750).contains(&val), "Altitude {} out of range", val);
    }

    #[test]
    fn test_decode_gillham_invalid_c_zero() {
        // All zeros except some A/B bits → c_digit=0 → c_bin=0 → returns None
        let alt = decode_altitude(0b_0_0_0_0_0_0_0_1_0_0_0_0_0); // only B1=1
        assert!(alt.is_none(), "C=0 should be invalid in Gillham");
    }

    #[test]
    fn test_decode_gillham_range() {
        // Systematically test that all valid Gillham codes produce in-range altitudes
        let mut valid_count = 0;
        for code in 0..0x2000u32 {
            let q_bit = (code >> 4) & 1;
            if q_bit == 1 {
                continue; // Skip 25ft mode
            }
            if let Some(alt) = decode_altitude(code) {
                assert!(
                    (-1200..=126750).contains(&alt),
                    "Gillham code 0x{:04X} gave altitude {} out of range",
                    code,
                    alt
                );
                valid_count += 1;
            }
        }
        assert!(valid_count > 0, "Should have some valid Gillham codes");
    }

    #[test]
    fn test_decode_squawk_7500() {
        // 7500 = A=7, B=5, C=0, D=0
        // A: a4=1,a2=1,a1=1 -> 7
        // B: b4=1,b2=0,b1=1 -> 5
        // C: c4=0,c2=0,c1=0 -> 0
        // D: d4=0,d2=0,d1=0 -> 0
        let id_code = 0b0_1_0_1_0_1_0_1_0_0_0_1_0;
        assert_eq!(decode_squawk(id_code), "7500");
    }

    #[test]
    fn test_decode_squawk_7600() {
        // 7600 = A=7, B=6, C=0, D=0
        let id_code = 0b0_1_0_1_0_1_0_0_0_1_0_1_0;
        assert_eq!(decode_squawk(id_code), "7600");
    }

    #[test]
    fn test_decode_squawk_7700() {
        // 7700 = A=7, B=7, C=0, D=0
        // A: a4=1,a2=1,a1=1 -> 7
        // B: b4=1,b2=1,b1=1 -> 7
        // C: c4=0,c2=0,c1=0 -> 0
        // D: d4=0,d2=0,d1=0 -> 0
        // Bit layout: C1 A1 C2 A2 C4 A4 _ B1 D1 B2 D2 B4 D4
        //             0  1  0  1  0  1  0 1  0  1  0  1  0
        let id_code = 0b0_1_0_1_0_1_0_1_0_1_0_1_0;
        assert_eq!(decode_squawk(id_code), "7700");
    }

    // -- Full decode routing --

    #[test]
    fn test_decode_routes_identification() {
        let frame = parse("8D4840D6202CC371C32CE0576098");
        let msg = decode(&frame).unwrap();
        assert!(matches!(msg, DecodedMsg::Identification(_)));
    }

    #[test]
    fn test_decode_routes_position() {
        let frame = parse("8D40621D58C382D690C8AC2863A7");
        let msg = decode(&frame).unwrap();
        assert!(matches!(msg, DecodedMsg::Position(_)));
    }

    #[test]
    fn test_decode_routes_velocity() {
        let frame = parse("8D485020994409940838175B284F");
        let msg = decode(&frame).unwrap();
        assert!(matches!(msg, DecodedMsg::Velocity(_)));
    }

    #[test]
    fn test_decode_msg_icao() {
        let frame = parse("8D4840D6202CC371C32CE0576098");
        let msg = decode(&frame).unwrap();
        assert_eq!(icao_to_string(msg.icao()), "4840D6");
    }

    #[test]
    fn test_decode_crc_failed_returns_none() {
        // Construct a frame with crc_ok = false
        let frame = ModeFrame {
            df: 17,
            icao: [0x48, 0x40, 0xD6],
            raw: vec![0; 14],
            timestamp: 1.0,
            signal_level: None,
            msg_bits: 112,
            crc_ok: false,
            corrected: false,
        };
        assert!(decode(&frame).is_none());
    }
}
