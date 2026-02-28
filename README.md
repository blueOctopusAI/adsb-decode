# adsb-decode

ADS-B radio protocol decoder — from raw 1090 MHz radio signals to identified aircraft on a map.

Built from scratch in Python. No dump1090 dependency, no borrowed decoders. Every byte of the protocol is decoded and documented, from the preamble pulse pattern to the Compact Position Reporting trigonometry.

## What It Does

Plugs into an RTL-SDR USB dongle, tunes to 1090 MHz, and decodes ADS-B broadcasts from every aircraft within ~150 nautical miles. Aircraft identity, position, altitude, speed, heading — all extracted from raw radio samples.

**What makes this different from dump1090:**

| dump1090 | adsb-decode |
|----------|-------------|
| Real-time only | Historical database — query what flew last week |
| No military detection | ICAO block analysis + callsign pattern filters |
| No anomaly detection | Emergency squawks, rapid descent, circling, geofence |
| No export | CSV, JSON, KML (Google Earth flight paths), GeoJSON |
| C, hard to modify | Python, readable, documented |
| "Just works" | "Decoded blind, explained every step" |

## The Signal Chain

```
RTL-SDR Dongle (1090 MHz)
    → Raw IQ Samples (capture.py)
    → Magnitude Signal (demodulator.py)
    → Mode S Frames (frame_parser.py + crc.py)
    → Typed Messages (decoder.py + cpr.py)
    → Aircraft State (tracker.py + icao.py)
    → SQLite Database (database.py)
    → Web Map + CLI (web/ + cli.py)
```

Each stage has a dedicated module with full documentation. See [HOW-IT-WORKS.md](HOW-IT-WORKS.md) for the complete signal chain deep dive — from the physics of pulse position modulation to the trigonometry of Compact Position Reporting.

## Real Demo

First capture session — RTL-SDR dongle on a desk in Franklin, NC. 45 seconds, stock whip antenna indoors:

```
┏━━━━━━━━━━━━┳━━━━━━━━━━━┳━━━━━━━━━━━━━━━┳━━━━━━━━┳━━━━━━━━━━┳━━━━━━━━━┳━━━━━━━━━━━┓
┃ ICAO       ┃ Callsign  ┃ Country       ┃ Reg    ┃ Alt (ft) ┃ Speed   ┃ Msgs      ┃
┡━━━━━━━━━━━━╇━━━━━━━━━━━╇━━━━━━━━━━━━━━━╇━━━━━━━━╇━━━━━━━━━━╇━━━━━━━━━╇━━━━━━━━━━━┩
│ A4CA5F     │ AAL2127   │ United States │ N364BW │ 24000    │ 361     │ 7         │
│ AB7E1C     │ -         │ United States │ N7101C │ 15450    │ 117     │ 3         │
│ AA1BB0     │ N6539K    │ United States │ N6539K │ 37000    │ 443     │ 4         │
│ A53432     │ -         │ United States │ N27XQ  │ 35000    │ 438     │ 2         │
│ A4E2D0     │ -         │ United States │ N372JV │ 29825    │ -       │ 2         │
│            │ ... 36 more aircraft identified                                     │
└──────────────────────────────────────────────────────────────────────────────────┘

Summary:
  Total frames:     296
  Valid frames:     52
  Position decodes: 8
  Aircraft seen:    41
```

5 aircraft fully resolved with N-number registrations. 41 total ICAO addresses heard in under a minute. Range estimated at 80-120 nm from aircraft altitudes and distances.

## Quick Start

```bash
# Install
pip install -e ".[dev]"

# Hardware setup (macOS)
bash scripts/setup-rtlsdr.sh

# Decode a capture file
adsb decode data/live_capture.txt

# Track from file with persistence
adsb track data/live_capture.txt --db-path data/flights.db

# Live tracking with web dashboard
adsb track --live --port 8080

# Database statistics
adsb stats --db-path data/flights.db

# Export flight paths to Google Earth
adsb export --format kml -o flights.kml
```

## Hardware

- **RTL-SDR USB dongle** ($25-35) — any RTL2832U-based receiver
- **1090 MHz antenna** — the included whip antenna works; a tuned antenna improves range
- **Line of sight** — place near a window or outside. ADS-B is line-of-sight at 1090 MHz.

Expected range: 50-150 nautical miles depending on antenna placement and aircraft altitude.

## Intelligence Features

This isn't just a radio scanner. It's an intelligence tool.

- **Military aircraft detection** — ICAO address block analysis identifies military-registered aircraft. Callsign pattern matching catches military flights (RCH = C-17 Globemaster, DUKE = Army, REACH = Air Mobility Command).
- **Emergency monitoring** — Squawk 7500 (hijack), 7600 (radio failure), 7700 (general emergency) trigger immediate alerts.
- **Anomaly detection** — Rapid descent (>5000 ft/min), unusual circling patterns, low-altitude operations.
- **Geofence alerts** — Configure a lat/lon/radius zone and get notified when aircraft enter it.
- **Historical queries** — SQLite database stores every position report. Ask questions about past traffic patterns.

## Why This Exists

Two reasons:

1. **AI-accelerated protocol reverse engineering.** The ADS-B protocol spec is ~200 pages of ICAO documentation. The CPR position encoding involves zone-based trigonometry that takes humans days to implement correctly. AI compressed the decode implementation from weeks to hours — same methodology demonstrated in [ctf-lab](https://github.com/blueOctopusAI/ctf-lab), different domain.

2. **Historical air traffic intelligence.** Every other ADS-B tool shows you what's flying *right now*. This one remembers what flew *last month*. Pattern analysis, not just surveillance.

## Project Structure

```
src/
├── capture.py       # IQ file reader, hex frame reader, live RTL-SDR capture
├── demodulator.py   # Raw IQ → magnitude → preamble detection → PPM bit recovery
├── frame_parser.py  # Bitstream → ModeFrame, downlink format classification
├── crc.py           # CRC-24 validation (ICAO polynomial)
├── decoder.py       # ModeFrame → typed messages (identification, position, velocity)
├── cpr.py           # Compact Position Reporting — global + local decode
├── icao.py          # Country lookup, military detection, N-number conversion
├── tracker.py       # Per-aircraft state machine with CPR frame pairing
├── database.py      # SQLite with WAL mode, multi-receiver schema
├── filters.py       # Military, emergency, rapid descent, low altitude, geofence
├── exporters.py     # CSV, JSON, KML (Google Earth), GeoJSON
├── cli.py           # Click CLI — decode, track, stats, history, export, serve
└── web/             # Flask + Leaflet.js dashboard with 2-second polling
```

**278 tests** covering every module. See [HOW-IT-WORKS.md](HOW-IT-WORKS.md) for the complete signal chain deep dive.

## License

MIT
