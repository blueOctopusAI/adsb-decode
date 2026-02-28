# STYLE.md — adsb-decode

## Voice

Technical but accessible. This project bridges two audiences:
- **Engineers** (Daniel, defense contractors) who want to see the signal processing rigor
- **Non-engineers** (portfolio visitors) who want to understand what it does and why it matters

Default to the engineering voice. Switch to plain language in README.md and user-facing output.

## Code Style

- Python 3.10+, type hints on all public functions
- Docstrings on modules and public classes/functions (Google style)
- Constants in UPPER_SNAKE_CASE with comments explaining the value's origin
- Hex values for protocol constants (0xFFF409, not 16773129)
- Bit operations documented with bit position references
- No abbreviations in variable names except established ones (icao, crc, cpr, df, tc)

## Documentation Style

- HOW-IT-WORKS.md: Write like a textbook. Explain the physics, show the math, trace the bits.
- README.md: Write like a project pitch. What, why, how, demo.
- Code comments: Explain WHY, not WHAT. Reference ICAO doc sections where applicable.

## CLI Output

- Use Rich for formatted terminal output
- Aircraft tables: ICAO | Callsign | Alt | Speed | Heading | Lat | Lon | Age
- Timestamps in UTC (aviation standard)
- Altitudes in feet (aviation standard)
- Speeds in knots (aviation standard)
- Distances in nautical miles

## Error Messages

Direct and actionable:
- Bad: "Error processing frame"
- Good: "CRC check failed on frame at offset 0x1A3F — skipping (noise or partial frame)"
- Bad: "Hardware not found"
- Good: "No RTL-SDR device detected. Run 'bash scripts/setup-rtlsdr.sh' to install drivers."
