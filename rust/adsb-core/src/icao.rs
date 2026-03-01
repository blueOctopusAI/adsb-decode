//! ICAO address resolution — country lookup, military detection, N-number decode.
//!
//! Every aircraft has a unique 24-bit ICAO address assigned by country.
//! Address ranges are allocated in blocks (e.g., 0xA00000-0xAFFFFF = United States).

use crate::types::{icao_to_u32, Icao};

// ---------------------------------------------------------------------------
// Country allocation blocks (sorted by start address)
// ---------------------------------------------------------------------------

struct CountryBlock {
    start: u32,
    end: u32,
    country: &'static str,
}

const COUNTRY_BLOCKS: &[CountryBlock] = &[
    CountryBlock {
        start: 0x004000,
        end: 0x0043FF,
        country: "Zimbabwe",
    },
    CountryBlock {
        start: 0x006000,
        end: 0x006FFF,
        country: "Mozambique",
    },
    CountryBlock {
        start: 0x008000,
        end: 0x00FFFF,
        country: "South Africa",
    },
    CountryBlock {
        start: 0x010000,
        end: 0x017FFF,
        country: "Egypt",
    },
    CountryBlock {
        start: 0x018000,
        end: 0x01FFFF,
        country: "Libya",
    },
    CountryBlock {
        start: 0x020000,
        end: 0x027FFF,
        country: "Morocco",
    },
    CountryBlock {
        start: 0x028000,
        end: 0x02FFFF,
        country: "Tunisia",
    },
    CountryBlock {
        start: 0x030000,
        end: 0x0303FF,
        country: "Botswana",
    },
    CountryBlock {
        start: 0x032000,
        end: 0x032FFF,
        country: "Burundi",
    },
    CountryBlock {
        start: 0x034000,
        end: 0x034FFF,
        country: "Cameroon",
    },
    CountryBlock {
        start: 0x038000,
        end: 0x038FFF,
        country: "Congo",
    },
    CountryBlock {
        start: 0x03E000,
        end: 0x03EFFF,
        country: "Ivory Coast",
    },
    CountryBlock {
        start: 0x040000,
        end: 0x040FFF,
        country: "DR Congo",
    },
    CountryBlock {
        start: 0x042000,
        end: 0x042FFF,
        country: "Ethiopia",
    },
    CountryBlock {
        start: 0x044000,
        end: 0x044FFF,
        country: "Equatorial Guinea",
    },
    CountryBlock {
        start: 0x046000,
        end: 0x046FFF,
        country: "Gabon",
    },
    CountryBlock {
        start: 0x048000,
        end: 0x048FFF,
        country: "Ghana",
    },
    CountryBlock {
        start: 0x04A000,
        end: 0x04AFFF,
        country: "Guinea",
    },
    CountryBlock {
        start: 0x04C000,
        end: 0x04CFFF,
        country: "Kenya",
    },
    CountryBlock {
        start: 0x050000,
        end: 0x050FFF,
        country: "Liberia",
    },
    CountryBlock {
        start: 0x054000,
        end: 0x054FFF,
        country: "Madagascar",
    },
    CountryBlock {
        start: 0x058000,
        end: 0x058FFF,
        country: "Malawi",
    },
    CountryBlock {
        start: 0x05A000,
        end: 0x05AFFF,
        country: "Mali",
    },
    CountryBlock {
        start: 0x05C000,
        end: 0x05CFFF,
        country: "Mauritania",
    },
    CountryBlock {
        start: 0x060000,
        end: 0x060FFF,
        country: "Niger",
    },
    CountryBlock {
        start: 0x062000,
        end: 0x062FFF,
        country: "Nigeria",
    },
    CountryBlock {
        start: 0x064000,
        end: 0x064FFF,
        country: "Uganda",
    },
    CountryBlock {
        start: 0x068000,
        end: 0x068FFF,
        country: "Senegal",
    },
    CountryBlock {
        start: 0x06A000,
        end: 0x06AFFF,
        country: "Sierra Leone",
    },
    CountryBlock {
        start: 0x06C000,
        end: 0x06CFFF,
        country: "Somalia",
    },
    CountryBlock {
        start: 0x070000,
        end: 0x070FFF,
        country: "Sudan",
    },
    CountryBlock {
        start: 0x074000,
        end: 0x074FFF,
        country: "Tanzania",
    },
    CountryBlock {
        start: 0x078000,
        end: 0x078FFF,
        country: "Chad",
    },
    CountryBlock {
        start: 0x07C000,
        end: 0x07CFFF,
        country: "Zambia",
    },
    CountryBlock {
        start: 0x080000,
        end: 0x080FFF,
        country: "Comoros",
    },
    CountryBlock {
        start: 0x084000,
        end: 0x084FFF,
        country: "Djibouti",
    },
    CountryBlock {
        start: 0x088000,
        end: 0x088FFF,
        country: "Eritrea",
    },
    CountryBlock {
        start: 0x08A000,
        end: 0x08AFFF,
        country: "Gambia",
    },
    CountryBlock {
        start: 0x08C000,
        end: 0x08CFFF,
        country: "Burkina Faso",
    },
    CountryBlock {
        start: 0x098000,
        end: 0x098FFF,
        country: "Lesotho",
    },
    CountryBlock {
        start: 0x09A000,
        end: 0x09AFFF,
        country: "Namibia",
    },
    CountryBlock {
        start: 0x0A0000,
        end: 0x0A7FFF,
        country: "Algeria",
    },
    CountryBlock {
        start: 0x0C0000,
        end: 0x0C4FFF,
        country: "Angola",
    },
    CountryBlock {
        start: 0x0C8000,
        end: 0x0C8FFF,
        country: "Rwanda",
    },
    CountryBlock {
        start: 0x0CA000,
        end: 0x0CAFFF,
        country: "Togo",
    },
    CountryBlock {
        start: 0x0CC000,
        end: 0x0CCFFF,
        country: "Benin",
    },
    CountryBlock {
        start: 0x0D0000,
        end: 0x0D7FFF,
        country: "Bahamas",
    },
    CountryBlock {
        start: 0x0D8000,
        end: 0x0DFFFF,
        country: "Barbados",
    },
    CountryBlock {
        start: 0x0E0000,
        end: 0x0E3FFF,
        country: "Belize",
    },
    CountryBlock {
        start: 0x0E4000,
        end: 0x0E7FFF,
        country: "Colombia",
    },
    CountryBlock {
        start: 0x0E8000,
        end: 0x0EBFFF,
        country: "Costa Rica",
    },
    CountryBlock {
        start: 0x0EC000,
        end: 0x0EFFFF,
        country: "Cuba",
    },
    CountryBlock {
        start: 0x0F0000,
        end: 0x0F3FFF,
        country: "El Salvador",
    },
    CountryBlock {
        start: 0x0F4000,
        end: 0x0F7FFF,
        country: "Guatemala",
    },
    CountryBlock {
        start: 0x0F8000,
        end: 0x0FBFFF,
        country: "Guyana",
    },
    CountryBlock {
        start: 0x0FC000,
        end: 0x0FFFFF,
        country: "Haiti",
    },
    CountryBlock {
        start: 0x100000,
        end: 0x103FFF,
        country: "Honduras",
    },
    CountryBlock {
        start: 0x108000,
        end: 0x10BFFF,
        country: "Jamaica",
    },
    CountryBlock {
        start: 0x110000,
        end: 0x113FFF,
        country: "Nicaragua",
    },
    CountryBlock {
        start: 0x114000,
        end: 0x117FFF,
        country: "Panama",
    },
    CountryBlock {
        start: 0x118000,
        end: 0x11BFFF,
        country: "Dominican Republic",
    },
    CountryBlock {
        start: 0x11C000,
        end: 0x11FFFF,
        country: "Trinidad and Tobago",
    },
    CountryBlock {
        start: 0x120000,
        end: 0x123FFF,
        country: "Suriname",
    },
    CountryBlock {
        start: 0x140000,
        end: 0x143FFF,
        country: "Antigua and Barbuda",
    },
    CountryBlock {
        start: 0x200000,
        end: 0x27FFFF,
        country: "Unassigned",
    },
    CountryBlock {
        start: 0x300000,
        end: 0x33FFFF,
        country: "Italy",
    },
    CountryBlock {
        start: 0x340000,
        end: 0x37FFFF,
        country: "Spain",
    },
    CountryBlock {
        start: 0x380000,
        end: 0x3BFFFF,
        country: "France",
    },
    CountryBlock {
        start: 0x3C0000,
        end: 0x3FFFFF,
        country: "Germany",
    },
    CountryBlock {
        start: 0x400000,
        end: 0x43FFFF,
        country: "United Kingdom",
    },
    CountryBlock {
        start: 0x440000,
        end: 0x447FFF,
        country: "Austria",
    },
    CountryBlock {
        start: 0x448000,
        end: 0x44FFFF,
        country: "Belgium",
    },
    CountryBlock {
        start: 0x450000,
        end: 0x457FFF,
        country: "Bulgaria",
    },
    CountryBlock {
        start: 0x458000,
        end: 0x45FFFF,
        country: "Denmark",
    },
    CountryBlock {
        start: 0x460000,
        end: 0x467FFF,
        country: "Finland",
    },
    CountryBlock {
        start: 0x468000,
        end: 0x46FFFF,
        country: "Greece",
    },
    CountryBlock {
        start: 0x470000,
        end: 0x477FFF,
        country: "Hungary",
    },
    CountryBlock {
        start: 0x478000,
        end: 0x47FFFF,
        country: "Norway",
    },
    CountryBlock {
        start: 0x480000,
        end: 0x487FFF,
        country: "Netherlands",
    },
    CountryBlock {
        start: 0x488000,
        end: 0x48FFFF,
        country: "Poland",
    },
    CountryBlock {
        start: 0x490000,
        end: 0x497FFF,
        country: "Portugal",
    },
    CountryBlock {
        start: 0x498000,
        end: 0x49FFFF,
        country: "Czech Republic",
    },
    CountryBlock {
        start: 0x4A0000,
        end: 0x4A7FFF,
        country: "Romania",
    },
    CountryBlock {
        start: 0x4A8000,
        end: 0x4AFFFF,
        country: "Sweden",
    },
    CountryBlock {
        start: 0x4B0000,
        end: 0x4B7FFF,
        country: "Switzerland",
    },
    CountryBlock {
        start: 0x4B8000,
        end: 0x4BFFFF,
        country: "Turkey",
    },
    CountryBlock {
        start: 0x4C0000,
        end: 0x4C7FFF,
        country: "Yugoslavia/Serbia",
    },
    CountryBlock {
        start: 0x4CA000,
        end: 0x4CAFFF,
        country: "Cyprus",
    },
    CountryBlock {
        start: 0x4CC000,
        end: 0x4CCFFF,
        country: "Ireland",
    },
    CountryBlock {
        start: 0x4D0000,
        end: 0x4D03FF,
        country: "Iceland",
    },
    CountryBlock {
        start: 0x500000,
        end: 0x5003FF,
        country: "Sri Lanka",
    },
    CountryBlock {
        start: 0x501000,
        end: 0x5013FF,
        country: "Malaysia",
    },
    CountryBlock {
        start: 0x508000,
        end: 0x50FFFF,
        country: "Indonesia",
    },
    CountryBlock {
        start: 0x510000,
        end: 0x5107FF,
        country: "Iraq",
    },
    CountryBlock {
        start: 0x600000,
        end: 0x6003FF,
        country: "Singapore",
    },
    CountryBlock {
        start: 0x680000,
        end: 0x6803FF,
        country: "Thailand",
    },
    CountryBlock {
        start: 0x681000,
        end: 0x6813FF,
        country: "Vietnam",
    },
    CountryBlock {
        start: 0x700000,
        end: 0x700FFF,
        country: "Afghanistan",
    },
    CountryBlock {
        start: 0x710000,
        end: 0x717FFF,
        country: "Pakistan",
    },
    CountryBlock {
        start: 0x718000,
        end: 0x71FFFF,
        country: "Bangladesh",
    },
    CountryBlock {
        start: 0x720000,
        end: 0x727FFF,
        country: "Myanmar",
    },
    CountryBlock {
        start: 0x730000,
        end: 0x737FFF,
        country: "Kuwait",
    },
    CountryBlock {
        start: 0x738000,
        end: 0x73FFFF,
        country: "Laos",
    },
    CountryBlock {
        start: 0x740000,
        end: 0x747FFF,
        country: "Nepal",
    },
    CountryBlock {
        start: 0x748000,
        end: 0x74FFFF,
        country: "Oman",
    },
    CountryBlock {
        start: 0x750000,
        end: 0x757FFF,
        country: "Saudi Arabia",
    },
    CountryBlock {
        start: 0x758000,
        end: 0x75FFFF,
        country: "South Korea",
    },
    CountryBlock {
        start: 0x760000,
        end: 0x767FFF,
        country: "North Korea",
    },
    CountryBlock {
        start: 0x768000,
        end: 0x76FFFF,
        country: "Syria",
    },
    CountryBlock {
        start: 0x770000,
        end: 0x777FFF,
        country: "Taiwan",
    },
    CountryBlock {
        start: 0x778000,
        end: 0x77FFFF,
        country: "Jordan",
    },
    CountryBlock {
        start: 0x780000,
        end: 0x7BFFFF,
        country: "China",
    },
    CountryBlock {
        start: 0x7C0000,
        end: 0x7FFFFF,
        country: "Australia",
    },
    CountryBlock {
        start: 0x800000,
        end: 0x83FFFF,
        country: "India",
    },
    CountryBlock {
        start: 0x840000,
        end: 0x87FFFF,
        country: "Japan",
    },
    CountryBlock {
        start: 0x880000,
        end: 0x887FFF,
        country: "Thailand",
    },
    CountryBlock {
        start: 0x890000,
        end: 0x890FFF,
        country: "Vietnam",
    },
    CountryBlock {
        start: 0x894000,
        end: 0x894FFF,
        country: "Hong Kong",
    },
    CountryBlock {
        start: 0x895000,
        end: 0x8953FF,
        country: "Macau",
    },
    CountryBlock {
        start: 0x896000,
        end: 0x896FFF,
        country: "Cambodia",
    },
    CountryBlock {
        start: 0x897000,
        end: 0x8973FF,
        country: "Philippines",
    },
    CountryBlock {
        start: 0x898000,
        end: 0x898FFF,
        country: "Mongolia",
    },
    CountryBlock {
        start: 0x899000,
        end: 0x8993FF,
        country: "Maldives",
    },
    CountryBlock {
        start: 0x8A0000,
        end: 0x8A7FFF,
        country: "UAE",
    },
    CountryBlock {
        start: 0x900000,
        end: 0x9003FF,
        country: "Israel",
    },
    CountryBlock {
        start: 0xA00000,
        end: 0xAFFFFF,
        country: "United States",
    },
    CountryBlock {
        start: 0xC00000,
        end: 0xC3FFFF,
        country: "Canada",
    },
    CountryBlock {
        start: 0xC80000,
        end: 0xC87FFF,
        country: "New Zealand",
    },
    CountryBlock {
        start: 0xC88000,
        end: 0xC88FFF,
        country: "Fiji",
    },
    CountryBlock {
        start: 0xE00000,
        end: 0xE3FFFF,
        country: "Argentina",
    },
    CountryBlock {
        start: 0xE40000,
        end: 0xE7FFFF,
        country: "Brazil",
    },
    CountryBlock {
        start: 0xE80000,
        end: 0xE83FFF,
        country: "Chile",
    },
    CountryBlock {
        start: 0xE84000,
        end: 0xE87FFF,
        country: "Ecuador",
    },
    CountryBlock {
        start: 0xE88000,
        end: 0xE8BFFF,
        country: "Paraguay",
    },
    CountryBlock {
        start: 0xE8C000,
        end: 0xE8FFFF,
        country: "Peru",
    },
    CountryBlock {
        start: 0xE90000,
        end: 0xE93FFF,
        country: "Uruguay",
    },
    CountryBlock {
        start: 0xE94000,
        end: 0xE97FFF,
        country: "Venezuela",
    },
    CountryBlock {
        start: 0xF00000,
        end: 0xF07FFF,
        country: "ICAO (special)",
    },
    CountryBlock {
        start: 0xF09000,
        end: 0xF093FF,
        country: "ICAO (special)",
    },
];

// US military ICAO block
const US_MILITARY_START: u32 = 0xADF7C8;
const US_MILITARY_END: u32 = 0xAFFFFF;

// US civil N-number range
const US_CIVIL_START: u32 = 0xA00001;
const US_CIVIL_END: u32 = 0xADF7C7;

// Characters for N-number suffix (A-Z excluding I and O)
const NNUM_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";

// Military callsign prefixes
const MILITARY_CALLSIGNS: &[&str] = &[
    "RCH", "DUKE", "DOOM", "JAKE", "TOPCAT", "REACH", "EVAC", "TEAL", "SPAR", "SAM", "EXEC",
    "CRZR", "MOOSE", "CANAF", "ASCOT", "RAFR", "GAF", "URAN", "CNV", "FAF", "IAM", "SUI",
];

/// Look up country of registration from ICAO address.
pub fn lookup_country(icao: &Icao) -> Option<&'static str> {
    let addr = icao_to_u32(icao);
    for block in COUNTRY_BLOCKS {
        if addr >= block.start && addr <= block.end {
            return Some(block.country);
        }
    }
    None
}

/// Look up country from a hex string.
pub fn lookup_country_hex(icao_hex: &str) -> Option<&'static str> {
    let addr = u32::from_str_radix(icao_hex, 16).ok()?;
    for block in COUNTRY_BLOCKS {
        if addr >= block.start && addr <= block.end {
            return Some(block.country);
        }
    }
    None
}

/// Check if an aircraft is military.
///
/// Two detection methods:
/// 1. ICAO address in a military allocation block (US military)
/// 2. Callsign matches known military patterns
pub fn is_military(icao: &Icao, callsign: Option<&str>) -> bool {
    let addr = icao_to_u32(icao);

    // US military address block
    if (US_MILITARY_START..=US_MILITARY_END).contains(&addr) {
        return true;
    }

    // Military callsign check
    if let Some(cs) = callsign {
        let cs = cs.trim().to_uppercase();
        for prefix in MILITARY_CALLSIGNS {
            if cs.starts_with(prefix) {
                return true;
            }
        }
    }

    false
}

/// Decode 1-2 letter suffix from remainder for N-number.
fn letter_suffix(remainder: u32, max_letters: u32) -> Option<String> {
    if max_letters == 1 {
        if (remainder as usize) < NNUM_CHARS.len() {
            return Some(String::from(NNUM_CHARS[remainder as usize] as char));
        }
        return None;
    }

    // 2-letter zone: 24 first letters * 25 options each (bare + 24 second letters)
    let first_idx = remainder / 25;
    if first_idx as usize >= NNUM_CHARS.len() {
        return None;
    }
    let second_rem = remainder % 25;
    if second_rem == 0 {
        return Some(String::from(NNUM_CHARS[first_idx as usize] as char));
    }
    let second_idx = second_rem - 1;
    if (second_idx as usize) < NNUM_CHARS.len() {
        let mut s = String::with_capacity(2);
        s.push(NNUM_CHARS[first_idx as usize] as char);
        s.push(NNUM_CHARS[second_idx as usize] as char);
        return Some(s);
    }
    None
}

/// Convert US civil ICAO address to N-number (tail number).
///
/// US civil aircraft addresses (0xA00001-0xADF7C7) encode the registration
/// using a base conversion.
pub fn icao_to_n_number(icao: &Icao) -> Option<String> {
    let addr = icao_to_u32(icao);
    if !(US_CIVIL_START..=US_CIVIL_END).contains(&addr) {
        return None;
    }

    let offset = addr - US_CIVIL_START;

    let d1 = offset / 101711;
    if d1 > 8 {
        return None;
    }
    let mut remainder = offset % 101711;
    let mut prefix = format!("N{}", d1 + 1);

    if remainder == 0 {
        return Some(prefix);
    }
    remainder -= 1;

    // After d1: 10 * 10111 digit addresses, then 600 letter suffix addresses
    if remainder < 10 * 10111 {
        let d2 = remainder / 10111;
        remainder %= 10111;
        prefix.push(char::from(b'0' + d2 as u8));

        if remainder == 0 {
            return Some(prefix);
        }
        remainder -= 1;

        if remainder < 10 * 951 {
            let d3 = remainder / 951;
            remainder %= 951;
            prefix.push(char::from(b'0' + d3 as u8));

            if remainder == 0 {
                return Some(prefix);
            }
            remainder -= 1;

            if remainder < 10 * 35 {
                let d4 = remainder / 35;
                remainder %= 35;
                prefix.push(char::from(b'0' + d4 as u8));

                if remainder == 0 {
                    return Some(prefix);
                }
                remainder -= 1;

                // After d4: 10 digits then 24 single letters
                if remainder < 10 {
                    return Some(format!("{prefix}{remainder}"));
                }
                remainder -= 10;
                if let Some(suffix) = letter_suffix(remainder, 1) {
                    return Some(format!("{prefix}{suffix}"));
                }
            } else {
                // Letter suffix after 3 digits
                remainder -= 10 * 35;
                if let Some(suffix) = letter_suffix(remainder, 2) {
                    return Some(format!("{prefix}{suffix}"));
                }
            }
        } else {
            // Letter suffix after 2 digits
            remainder -= 10 * 951;
            if let Some(suffix) = letter_suffix(remainder, 2) {
                return Some(format!("{prefix}{suffix}"));
            }
        }
    } else {
        // Letter suffix after 1 digit
        remainder -= 10 * 10111;
        if let Some(suffix) = letter_suffix(remainder, 2) {
            return Some(format!("{prefix}{suffix}"));
        }
    }

    None
}

/// Convert N-number from hex string convenience wrapper.
pub fn icao_hex_to_n_number(icao_hex: &str) -> Option<String> {
    let addr = u32::from_str_radix(icao_hex, 16).ok()?;
    let icao = crate::types::icao_from_u32(addr);
    icao_to_n_number(&icao)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::icao_from_hex;

    #[test]
    fn test_lookup_country_us() {
        assert_eq!(lookup_country_hex("A00001"), Some("United States"));
    }

    #[test]
    fn test_lookup_country_uk() {
        assert_eq!(lookup_country_hex("4840D6"), Some("Netherlands"));
        assert_eq!(lookup_country_hex("406B90"), Some("United Kingdom"));
        assert_eq!(lookup_country_hex("40621D"), Some("United Kingdom"));
    }

    #[test]
    fn test_lookup_country_germany() {
        assert_eq!(lookup_country_hex("3C6586"), Some("Germany"));
    }

    #[test]
    fn test_lookup_country_unknown() {
        assert_eq!(lookup_country_hex("FFFFFF"), None);
    }

    #[test]
    fn test_is_military_us_block() {
        let icao = icao_from_hex("ADF7C8").unwrap();
        assert!(is_military(&icao, None));
    }

    #[test]
    fn test_is_not_military_us_civil() {
        let icao = icao_from_hex("A00001").unwrap();
        assert!(!is_military(&icao, None));
        let icao = icao_from_hex("ADF7C7").unwrap();
        assert!(!is_military(&icao, None));
    }

    #[test]
    fn test_is_military_callsign() {
        let icao = icao_from_hex("A00001").unwrap();
        assert!(is_military(&icao, Some("RCH123")));
        assert!(is_military(&icao, Some("DUKE01")));
        assert!(!is_military(&icao, Some("UAL123")));
    }

    #[test]
    fn test_n_number_first_address() {
        // 0xA00001 should be N1
        assert_eq!(icao_hex_to_n_number("A00001"), Some("N1".into()));
    }

    #[test]
    fn test_n_number_last_civil() {
        // 0xADF7C7 should be the last valid N-number
        assert!(icao_hex_to_n_number("ADF7C7").is_some());
    }

    #[test]
    fn test_n_number_military_returns_none() {
        assert!(icao_hex_to_n_number("ADF7C8").is_none());
    }

    #[test]
    fn test_n_number_non_us_returns_none() {
        assert!(icao_hex_to_n_number("4840D6").is_none());
    }

    #[test]
    fn test_n_number_known_values() {
        // N10 = A00001 + 1 (bare d1=1, then remainder=1, -1=0, d2=0 -> N10)
        // Let's verify the offset math:
        // offset=0 -> N1, offset=1 -> remainder after d1=0 is 1, -1=0 -> d2=0 -> N10
        assert_eq!(icao_hex_to_n_number("A00002"), Some("N10".into()));
    }

    #[test]
    fn test_n_number_chars_no_i_or_o() {
        assert!(!NNUM_CHARS.contains(&b'I'));
        assert!(!NNUM_CHARS.contains(&b'O'));
        assert_eq!(NNUM_CHARS.len(), 24);
    }
}
