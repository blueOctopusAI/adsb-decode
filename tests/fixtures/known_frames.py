"""Published ADS-B test vectors for validation.

These are real ADS-B frames from published sources (pyModeS documentation,
ICAO Annex 10, academic papers). Each frame has a known hex string and
expected decoded values.

Sources:
- pyModeS documentation: https://mode-s.org/decode/
- Junzi Sun, "The 1090 Megahertz Riddle" (2nd ed.)
- ICAO Doc 9871 (Technical Provisions for Mode S)
"""

# DF17 TC1-4: Aircraft Identification
# Format: (hex_string, expected_icao, expected_callsign)
IDENTIFICATION_FRAMES = [
    # Example from "The 1090MHz Riddle" — aircraft identification
    ("8D4840D6202CC371C32CE0576098", "4840D6", "KLM1023 "),
    ("8D406B902015A678D4D220AA4BDA", "406B90", "EZY85MH "),
]

# DF17 TC9-18: Airborne Position (CPR encoded)
# Format: (hex_string, expected_icao, expected_altitude_ft, cpr_format, cpr_lat, cpr_lon)
POSITION_FRAMES = [
    # Even/odd pair from "The 1090MHz Riddle"
    # Even frame (cpr_format=0)
    ("8D40621D58C382D690C8AC2863A7", "40621D", 38000, 0, 93000, 51372),
    # Odd frame (cpr_format=1)
    ("8D40621D58C386435CC412692AD6", "40621D", 38000, 1, 74158, 50194),
]

# Expected position after global CPR decode of the above pair
POSITION_DECODED = {
    "icao": "40621D",
    "lat": 52.2572,   # approximate
    "lon": 3.9194,    # approximate
    "alt_ft": 38000,
}

# DF17 TC19: Airborne Velocity
# Format: (hex_string, expected_icao, expected_speed_kts, expected_heading_deg, expected_vrate_fpm)
VELOCITY_FRAMES = [
    # Ground speed example from "The 1090MHz Riddle"
    ("8D485020994409940838175B284F", "485020", 159, 182.88, -832),
]

# DF5: Surveillance Identity Reply (squawk codes)
# Format: (hex_string, expected_squawk)
SQUAWK_FRAMES = [
    # These will be populated with real captured frames
]

# CRC test vectors
# Format: (hex_string, expected_crc_remainder)
CRC_VECTORS = [
    # Valid DF17 frame — remainder should be 0x000000
    ("8D4840D6202CC371C32CE0576098", 0x000000),
    ("8D40621D58C382D690C8AC2863A7", 0x000000),
    ("8D485020994409940838175B284F", 0x000000),
]

# ICAO address test vectors
# Format: (icao_hex, expected_country, expected_military)
ICAO_VECTORS = [
    ("A00001", "United States", False),   # First US civil address
    ("ADF7C7", "United States", False),   # Last US civil address (N-number range)
    ("ADF7C8", "United States", True),    # US military block starts
    ("4840D6", "Netherlands", False),
    ("406B90", "United Kingdom", False),
    ("40621D", "United Kingdom", False),
    ("3C6586", "Germany", False),
]
