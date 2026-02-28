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

## Quick Start

```bash
# Install
pip install -e ".[dev]"

# Hardware setup (macOS)
bash scripts/setup-rtlsdr.sh

# Capture 60 seconds of aircraft data
adsb capture --duration 60

# Decode a capture file
adsb decode data/capture.bin

# Live tracking with web dashboard
adsb track --live --port 8080

# What military aircraft have we seen?
adsb stats --military

# Export flight paths to Google Earth
adsb export --format kml --output flights.kml
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

See [CLAUDE.md](CLAUDE.md) for the full module breakdown and [HOW-IT-WORKS.md](HOW-IT-WORKS.md) for the technical deep dive.

## License

MIT
