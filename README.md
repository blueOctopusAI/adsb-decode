# adsb-decode

ADS-B radio protocol decoder — from raw 1090 MHz radio signals to identified aircraft on a map.

Built from scratch. No dump1090 dependency, no borrowed decoders. Every byte of the protocol is decoded and documented, from the preamble pulse pattern to the Compact Position Reporting trigonometry.

Two implementations:
- **Python** — the original reference implementation (22 modules, 394 tests, ~7,300 lines)
- **Rust** — the primary/active implementation (3-crate workspace, 199 tests, ~10,700 lines)

Both decode the same protocol identically. Cross-validated: 296-frame capture file produces 100% matching output between Python and Rust.

## What It Does

Plugs into an RTL-SDR USB dongle, tunes to 1090 MHz, and decodes ADS-B broadcasts from every aircraft within ~150 nautical miles. Aircraft identity, position, altitude, speed, heading — all extracted from raw radio samples.

**What makes this different from dump1090:**

| dump1090 | adsb-decode |
|----------|-------------|
| Real-time only | Historical database — query what flew last week |
| No military detection | ICAO block analysis + callsign pattern filters |
| No anomaly detection | Emergency squawks, rapid descent, circling, holding patterns, proximity alerts, geofence |
| No export | CSV, JSON, KML (Google Earth flight paths), GeoJSON |
| Single receiver | Multi-receiver network (hub-and-spoke, feeder agents) |
| No enrichment | Aircraft type classification, 3,642 airports, operator lookup |
| C, hard to modify | Readable, documented, tested |

## The Signal Chain

```
RTL-SDR Dongle (1090 MHz)
    → Raw IQ Samples / rtl_adsb hex frames
    → Magnitude Signal (demod)
    → Mode S Frames (frame parser + CRC-24)
    → Typed Messages (decoder + CPR)
    → Aircraft State (tracker + ICAO lookup)
    → SQLite Database
    → Web Map + CLI
```

Each stage has a dedicated module with full documentation. See [HOW-IT-WORKS.md](HOW-IT-WORKS.md) for the complete signal chain deep dive.

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

5 aircraft fully resolved with N-number registrations. 41 total ICAO addresses heard in under a minute.

## Quick Start (Rust)

```bash
# Build
cd rust
cargo build --release

# Decode a capture file
cargo run --bin adsb -- decode data/live_capture.txt

# Track from file with persistence
cargo run --bin adsb -- track data/live_capture.txt --db-path data/flights.db

# Live tracking with web dashboard (RTL-SDR dongle required)
cargo run --bin adsb -- track --live --port 8080

# Serve dashboard from existing database (no dongle)
cargo run --bin adsb -- serve --db-path data/adsb.db --port 8080

# Database statistics
cargo run --bin adsb -- stats --db-path data/flights.db

# Export flight paths
cargo run --bin adsb -- export --db-path data/flights.db --format json
```

### Quick Start (Python reference)

```bash
pip install -e ".[dev]"
bash scripts/setup-rtlsdr.sh
adsb decode data/live_capture.txt
adsb track --live --port 8080
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
- **Circling/loitering detection** — Cumulative heading change analysis over 5-minute windows. Catches surveillance, search patterns, training flights.
- **Holding pattern detection** — Stable altitude + reciprocal headings identified via heading histogram analysis.
- **Proximity alerts** — Flags when two aircraft are within configurable distance (default 5nm horizontal, 1,000 ft vertical).
- **Unusual altitude** — Fast aircraft (>200 kts) at low altitude (<3,000 ft) far from airports.
- **Geofence alerts** — Configure a lat/lon/radius zone and get notified when aircraft enter it.
- **Aircraft type enrichment** — Speed/altitude profile classifies aircraft as jet, prop, turboprop, helicopter, military, or cargo. Airline operator lookup from callsign prefix.
- **Airport awareness** — 3,642 US airports bundled. Nearest airport lookup, flight phase classification (approaching, departing, overflying).
- **Historical queries** — SQLite database stores every position report. Query builder with preset and custom filters.
- **CRC error correction** — 1-2 bit error correction via syndrome table lookup. Recovers corrupted frames that would otherwise be dropped.

## Web Dashboard

Full-featured dark-themed dashboard at `http://127.0.0.1:8080`:

- **Live map** — Aircraft silhouette icons (jet/prop/turboprop/helicopter/military) with heading rotation, altitude-colored trail lines (green→yellow→red), stats overlay, altitude legend
- **Trail duration slider** — 5 minutes to 24 hours. Controls trail visibility AND which aircraft appear on the map/list.
- **Aircraft detail** — Split-screen view. Left: captured trail map, events, position history. Right: external intel from hexdb.io (manufacturer, type, owner), link cards to ADSBExchange/Planespotters/FlightAware/FlightRadar24/FAA Registry/OpenSky, altitude profile chart.
- **Airport overlay** — 3,642 US airports with Major/Medium/Small toggles. Click for details + AirNav/SkyVector links.
- **Heatmap** — Position density visualization toggle
- **Map styles** — Dark, Satellite, Topo, Streets, Dark Matter, Voyager (persisted in localStorage)
- **Events dashboard** — Color-coded events with type filters
- **Query builder** — Preset queries (military, low altitude, fast) + custom filters with map visualization
- **Historical replay** — Time slider with play/pause, adjustable speed (1x–10min)
- **Receiver management** — Connected feeders with coverage circles
- **Table view** — Sortable aircraft list with detail pages

## Multi-Receiver Network

Hub-and-spoke architecture for distributed coverage:

```
[Pi + Dongle] --HTTP POST--> [Central Server] <--Browser-- [Dashboard]
[Pi + Dongle] --HTTP POST-->      ↑
[Mac + Dongle] --HTTP POST-->     |
                            Axum API + SQLite
```

- **Feeder agent** (`adsb-feeder` binary) runs on each receiver node
- Bearer token authentication for frame ingestion
- Heartbeat monitoring with online/offline status
- ~$60/node (Pi + dongle + antenna)

## Why This Exists

Two reasons:

1. **AI-accelerated protocol reverse engineering.** The ADS-B protocol spec is ~200 pages of ICAO documentation. The CPR position encoding involves zone-based trigonometry that takes humans days to implement correctly. AI compressed the decode implementation from weeks to hours — same methodology demonstrated in [ctf-lab](https://github.com/blueOctopusAI/ctf-lab), different domain.

2. **Historical air traffic intelligence.** Every other ADS-B tool shows you what's flying *right now*. This one remembers what flew *last month*. Pattern analysis, not just surveillance.

## Project Structure

### Rust (primary)

```
rust/
├── Cargo.toml                # Workspace root
├── adsb-core/                # Library: pure decode + tracking (no async, no I/O)
│   └── src/
│       ├── lib.rs            # Module exports
│       ├── types.rs          # Shared types, hex encode/decode, Icao type
│       ├── crc.rs            # CRC-24 LUT, syndrome tables, 1-2 bit error correction
│       ├── frame.rs          # ModeFrame, parse_frame, IcaoCache
│       ├── decode.rs         # Identification, position, velocity, altitude, squawk
│       ├── cpr.rs            # CPR global/local decode, NL table
│       ├── demod.rs          # IQ→magnitude LUT, preamble, PPM bit recovery
│       ├── tracker.rs        # AircraftState, CPR pairing, stale pruning, TrackEvent
│       ├── filter.rs         # Event detection (military, emergency, circling, geofence, unusual alt)
│       ├── enrich.rs         # Aircraft classification, airline lookup, 3,642 airports
│       ├── icao.rs           # Country lookup, military detect, N-number conversion
│       ├── config.rs         # YAML config load/save
│       └── airports.csv      # OurAirports US airport database (embedded at compile time)
│
├── adsb-feeder/              # Binary: edge device (Pi + RTL-SDR)
│   └── src/
│       ├── main.rs           # Capture → decode → batch POST
│       └── capture.rs        # FrameReader, IQReader, LiveDemodCapture
│
├── adsb-server/              # Binary: web server + CLI + database
│   ├── src/
│   │   ├── main.rs           # CLI dispatch (decode, track, stats, history, export, serve, setup)
│   │   ├── db.rs             # SQLite database (6 tables, WAL mode, retention, downsample)
│   │   ├── db_pg.rs          # TimescaleDB backend (behind feature flag)
│   │   └── web/
│   │       ├── mod.rs        # Axum app, shared state, CORS
│   │       ├── routes.rs     # REST API endpoints
│   │       ├── pages.rs      # HTML page handlers
│   │       └── ingest.rs     # Multi-receiver ingest + heartbeat
│   └── templates/            # 8 HTML pages (map, table, detail, events, query, replay, receivers, stats)
```

### Python (reference)

```
src/
├── capture.py       # IQ file reader, hex frame reader, native demod + fallback live capture
├── demodulator.py   # Raw IQ → magnitude → preamble detection → PPM bit recovery
├── frame_parser.py  # Bitstream → ModeFrame, downlink format classification
├── crc.py           # CRC-24 validation (ICAO polynomial)
├── decoder.py       # ModeFrame → typed messages (identification, position, velocity)
├── cpr.py           # Compact Position Reporting — global + local decode
├── icao.py          # Country lookup, military detection, N-number conversion
├── tracker.py       # Per-aircraft state machine with CPR frame pairing
├── database.py      # SQLite with WAL mode, multi-receiver schema, tiered retention
├── filters.py       # Military, emergency, circling, holding, proximity, unusual altitude, geofence
├── enrichment.py    # Aircraft type classification, operator lookup, 3,642 airports
├── notifications.py # Webhook dispatch for events
├── config.py        # Config file management (~/.adsb-decode/config.yaml)
├── hardware.py      # RTL-SDR dongle detection, driver checks
├── exporters.py     # CSV, JSON, KML (Google Earth), GeoJSON
├── feeder.py        # Remote receiver agent
├── cli.py           # Click CLI
└── web/
    ├── app.py       # Flask app factory
    ├── ingest.py    # Frame ingestion API for remote feeders
    ├── routes.py    # REST API + page routes
    └── templates/   # Jinja2 templates (9 pages)
```

**Rust:** 199 tests, ~10,700 lines across 3 crates. **Python:** 394 tests, ~7,300 lines across 22 modules. See [HOW-IT-WORKS.md](HOW-IT-WORKS.md) for the complete signal chain deep dive.

## Future

- **Native IQ demodulation in Rust** — The `demod.rs` module exists with magnitude LUT, preamble detection, and PPM bit recovery. Currently live capture uses `rtl_adsb` subprocess for hex frame input. Wiring the native demod into the live capture path would eliminate the external dependency.
- **TimescaleDB production backend** — Implemented behind a feature flag (`db_pg.rs`). Includes hypertables, compression policies, retention policies, and continuous aggregates. Requires a PostgreSQL instance to activate.
- **Cross-compiled Pi binaries** — GitHub Actions CI configured for aarch64 (Pi 4/5) and armv7 (Pi 3) targets via `cross`. Release binaries not yet published.

## License

MIT
