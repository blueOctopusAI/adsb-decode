# CLAUDE.md

This file provides guidance to Claude Code when working with code in this repository.

## What This Is

An ADS-B (Automatic Dependent Surveillance-Broadcast) radio protocol decoder that processes raw 1090 MHz radio signals into identified aircraft on a map. Built from scratch in Python — no dump1090 dependency, no borrowed decoders. Every byte of the protocol is decoded and documented.

Part of the Blue Octopus Technology portfolio. Demonstrates AI-accelerated reverse engineering of live radio protocols, complementing the ctf-lab binary analysis work.

## The Signal Chain

```
RTL-SDR Dongle (1090 MHz) → Raw IQ Samples → Magnitude Signal → Mode S Frames → Typed Messages → Aircraft State → Database + Map
```

Each stage has a dedicated module in `src/`. See HOW-IT-WORKS.md for the full signal chain deep dive.

## Project Structure

```
src/
├── cli.py           # Click CLI — capture, decode, track, stats, export
├── capture.py       # IQ file readers, frame readers, live capture wrapper
├── demodulator.py   # IQ → bits (PPM demod, preamble detection, numpy DSP)
├── frame_parser.py  # Raw bits → ModeFrame objects (DF classification)
├── decoder.py       # Frames → typed messages (callsign, position, velocity)
├── cpr.py           # Compact Position Reporting math (global + local decode)
├── crc.py           # CRC-24 validation (ICAO standard polynomial)
├── icao.py          # ICAO address → country, military block detection, N-number
├── tracker.py       # Per-aircraft state machine with CPR pairing
├── database.py      # SQLite persistence (WAL mode, 5 tables)
├── filters.py       # Military, emergency squawk, anomaly, geofence detection
├── exporters.py     # CSV, JSON, KML (Google Earth), GeoJSON output
└── web/
    ├── app.py       # Flask app factory
    ├── routes.py    # REST API + page routes
    ├── templates/   # Jinja2 — map, table, detail, stats pages
    └── static/      # Leaflet.js map, CSS, aircraft icons
```

## Commands

```bash
# Development
pip install -e ".[dev]"          # Install in dev mode
pytest                           # Run all tests
pytest tests/test_crc.py -v      # Run specific test file

# CLI
adsb setup                       # Check RTL-SDR hardware
adsb capture --duration 60       # Capture 60s of frames
adsb decode data/capture.bin     # Decode a capture file
adsb track --file data/cap.bin   # Track aircraft from file
adsb track --live --port 8080    # Live tracking with web dashboard
adsb stats                       # Show database statistics
adsb export --format kml         # Export flight paths

# Hardware setup (macOS)
bash scripts/setup-rtlsdr.sh     # Install librtlsdr via Homebrew
bash scripts/validate-hardware.sh # Check dongle is detected
bash scripts/capture-iq.sh 60    # Capture 60s raw IQ samples
bash scripts/capture-frames.sh 60 # Capture 60s demodulated frames
```

## Database Schema (6 tables)

- **receivers** — sensor nodes (name, lat, lon, altitude, description). Multi-receiver from day one.
- **aircraft** — ICAO address, registration, country, military flag, first/last seen
- **sightings** — per-session appearance (callsign, squawk, signal strength)
- **positions** — lat, lon, alt, speed, heading, vertical rate, timestamp, `receiver_id`
- **captures** — metadata per capture session (source, duration, frame counts, `receiver_id`)
- **events** — detected anomalies (emergency squawk, rapid descent, military, circling)

SQLite with WAL mode, foreign keys enabled, indexed queries. Test fixtures use tmp_path.

Every position and capture is tagged with which receiver heard it. Single-receiver deployments work identically — one row in receivers table. Adding receivers is adding data sources, not refactoring.

## Key Technical Details

### CRC-24
- Generator polynomial: 0xFFF409 (ICAO standard)
- Applied to full message — valid frames produce remainder 0x000000
- Used for error detection AND ICAO address recovery in DF11/17/18

### Compact Position Reporting (CPR)
- Encodes lat/lon into 17-bit values using Nb=17 zones
- Even and odd frames use different zone counts (NZ=15 base)
- Global decode requires even+odd pair within 10 seconds
- Local decode uses reference position (receiver or last known)
- Edge cases: zone boundary crossings, polar regions, antimeridian

### Mode S Downlink Formats
- DF0: Short air-air surveillance (altitude)
- DF4: Surveillance altitude reply
- DF5: Surveillance identity reply (squawk)
- DF11: All-call reply (ICAO address acquisition)
- DF16: Long air-air surveillance
- DF17: ADS-B extended squitter (the main event)
- DF18: TIS-B / ADS-R
- DF20: Comm-B altitude reply
- DF21: Comm-B identity reply

### ADS-B Type Codes (within DF17)
- TC 1-4: Aircraft identification (callsign)
- TC 5-8: Surface position
- TC 9-18: Airborne position (barometric altitude)
- TC 19: Airborne velocity
- TC 20-22: Airborne position (GNSS altitude)
- TC 28: Aircraft status (emergency/priority)
- TC 29: Target state and status
- TC 31: Aircraft operational status

## Division of Labor

**Human does:** Plug in RTL-SDR, position antenna, run capture scripts, validate against FlightAware.

**Claude does:** All code, tests, documentation, CLI, web dashboard, DSP, protocol decode, CPR math.

## Testing Strategy

- Unit tests for every module with known test vectors
- Published ADS-B frame examples as fixtures (known hex → known decode)
- CPR math validated against published even/odd pairs
- CRC validated against ICAO standard polynomial
- Integration tests: hex frames → full decode pipeline
- Database tests use pytest tmp_path fixtures

## Sensitive Data

- **Never commit** capture files (.iq, .bin) — they contain real aircraft data
- **Never commit** database files (.db) — contain position histories
- Receiver coordinates (lat/lon) should use placeholder `[RECEIVER_LAT]`/`[RECEIVER_LON]` in committed docs
- ICAO addresses are public (broadcast over radio) — safe to reference in tests

## Companion Docs

- **SOUL.md** — Agent personality for this project
- **STYLE.md** — Output and documentation voice
- **HOW-IT-WORKS.md** — Deep technical walkthrough of the entire signal chain
- **README.md** — Public-facing project intro
