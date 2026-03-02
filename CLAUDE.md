# CLAUDE.md

This file provides guidance to Claude Code when working with code in this repository.

## What This Is

An ADS-B (Automatic Dependent Surveillance-Broadcast) radio protocol decoder that processes raw 1090 MHz radio signals into identified aircraft on a map. Built from scratch — no dump1090 dependency, no borrowed decoders. Every byte of the protocol is decoded and documented.

Two implementations exist side by side:
- **Rust** (primary, active development) — 3-crate workspace under `rust/`, 223 tests, ~12,000 lines
- **Python** (reference implementation) — 22 modules under `src/`, 394 tests, ~7,300 lines

The Rust implementation is feature-complete and is the active codebase. The Python implementation serves as the reference oracle — both were cross-validated against a 296-frame capture file with 100% field match.

Part of the Blue Octopus Technology portfolio. Demonstrates AI-accelerated reverse engineering of live radio protocols, complementing the ctf-lab binary analysis work.

## The Signal Chain

```
RTL-SDR Dongle (1090 MHz) → Raw IQ Samples → Magnitude Signal → Mode S Frames → Typed Messages → Aircraft State → Database + Map
```

See HOW-IT-WORKS.md for the full signal chain deep dive.

## Rust Project Structure (primary)

```
rust/
├── Cargo.toml                # Workspace root
├── adsb-core/                # Library: pure decode + tracking (no async, no I/O)
│   └── src/
│       ├── lib.rs            # Module exports
│       ├── types.rs          # Shared types, hex encode/decode, Icao = [u8; 3]
│       ├── crc.rs            # CRC-24 LUT (compile-time const fn), syndrome tables, try_fix
│       ├── frame.rs          # ModeFrame, parse_frame, IcaoCache
│       ├── decode.rs         # Identification, position, velocity, altitude, squawk, Gillham
│       ├── cpr.rs            # CPR global/local decode, NL table
│       ├── demod.rs          # IQ→magnitude LUT, preamble, PPM bit recovery
│       ├── tracker.rs        # AircraftState, CPR pairing, stale pruning, TrackEvent enum
│       ├── filter.rs         # FilterEngine with 8 detectors + dedup
│       ├── enrich.rs         # Aircraft classification, airline lookup, 3,642 airports (include_str!)
│       ├── icao.rs           # Country lookup, military blocks, N-number conversion
│       ├── config.rs         # YAML config load/save
│       └── airports.csv      # OurAirports US database (embedded at compile time)
│
├── adsb-feeder/              # Binary: edge device (Pi + RTL-SDR)
│   └── src/
│       ├── main.rs           # Capture → decode → batch POST to server
│       └── capture.rs        # FrameReader, IQReader, demodulate_stream, LiveCapture (native-sdr)
│
├── adsb-server/              # Binary: web server + CLI + database
│   ├── src/
│   │   ├── main.rs           # CLI: decode, track (--live --port), stats, history, export, serve, setup
│   │   ├── db.rs             # SQLite (6 tables, WAL, retention, downsample, phantom pruning)
│   │   ├── db_pg.rs          # TimescaleDB (feature-gated, not currently in use)
│   │   ├── notification.rs   # Webhook notifications (fire-and-forget POST JSON)
│   │   └── web/
│   │       ├── mod.rs        # Axum app builder, AppState, CORS, bearer token auth
│   │       ├── routes.rs     # REST API endpoints
│   │       ├── pages.rs      # HTML page handlers
│   │       └── ingest.rs     # Multi-receiver ingest + heartbeat + DB persistence
│   └── templates/            # 8 HTML pages with Leaflet.js maps
```

**Dependency graph:** `adsb-core` (pure, no async) ← `adsb-feeder` + `adsb-server`

## Python Project Structure (reference)

```
src/
├── cli.py           # Click CLI — setup, capture, decode, track, stats, export, serve
├── capture.py       # IQ file readers, frame readers, live capture (native demod + fallback)
├── demodulator.py   # IQ → bits (PPM demod, preamble detection, numpy DSP)
├── frame_parser.py  # Raw bits → ModeFrame objects (DF classification)
├── decoder.py       # Frames → typed messages (callsign, position, velocity)
├── cpr.py           # Compact Position Reporting math (global + local decode)
├── crc.py           # CRC-24 validation (ICAO standard polynomial)
├── icao.py          # ICAO address → country, military block detection, N-number
├── tracker.py       # Per-aircraft state machine with CPR pairing, ingest downsampling
├── database.py      # SQLite persistence (WAL mode, 6 tables, tiered retention, VACUUM)
├── filters.py       # Military, emergency, circling, holding, proximity, unusual altitude, geofence
├── enrichment.py    # Aircraft type classification, operator lookup, 3,642 airports
├── notifications.py # Webhook dispatch for events
├── config.py        # Config file management (~/.adsb-decode/config.yaml)
├── hardware.py      # RTL-SDR dongle detection, driver checks, test capture
├── exporters.py     # CSV, JSON, KML (Google Earth), GeoJSON output
├── feeder.py        # Remote receiver agent
└── web/
    ├── app.py       # Flask app factory
    ├── ingest.py    # Frame ingestion API for remote feeders
    ├── routes.py    # REST API + page routes
    └── templates/   # Jinja2 (9 pages)
```

## Commands

### Rust (primary)

```bash
cd rust
cargo build --release            # Build all crates
cargo test --workspace           # Run all 199 tests
cargo test -p adsb-core          # Test core library only
cargo clippy --workspace         # Lint

# CLI
cargo run --bin adsb -- decode data/live_capture.txt
cargo run --bin adsb -- track --live --port 8080     # Live RTL-SDR + web dashboard
cargo run --bin adsb -- track --live --native-demod   # Native IQ demod via rtl_sdr pipe
cargo run --bin adsb -- track --live --native-usb     # Direct USB via rtlsdr_mt (requires native-sdr feature)
cargo run --bin adsb -- track --live --webhook https://example.com/hook  # With notifications
cargo run --bin adsb -- track --live --auth-token SECRET  # With ingest API auth
cargo run --bin adsb -- serve --db-path data/adsb.db  # Serve dashboard from existing DB
cargo run --bin adsb -- stats --db-path data/adsb.db
cargo run --bin adsb -- history --db-path data/adsb.db
cargo run --bin adsb -- export --db-path data/adsb.db --format json
```

### Python (reference)

```bash
pip install -e ".[dev]"          # Install in dev mode
pytest                           # Run all 394 tests

adsb setup                       # Check RTL-SDR hardware
adsb decode data/capture.bin     # Decode a capture file
adsb track --live --port 8080    # Live tracking with web dashboard
```

## Database Schema (6 tables)

- **receivers** — sensor nodes (name, lat, lon, altitude, description)
- **aircraft** — ICAO address, registration, country, military flag (sticky via MAX), first/last seen
- **sightings** — per-session appearance (callsign, squawk, min/max altitude, message count)
- **positions** — lat, lon, alt, speed, heading, vertical rate, timestamp, receiver_id
- **captures** — metadata per capture session (source, duration, frame counts, receiver_id)
- **events** — detected anomalies (emergency squawk, military, circling, unusual altitude)

SQLite with WAL mode, foreign keys enabled, indexed queries.

Key SQL patterns:
- `is_military = MAX(is_military, excluded.is_military)` — military flag is sticky, never reverts
- `country = COALESCE(excluded.country, country)` — preserves existing country on NULL re-upsert
- `downsample_positions(older_than_hours, keep_interval_sec)` — bucket-based thinning

## Current State

### What works now:
- Full decode pipeline (CRC-24, all downlink formats, all ADS-B type codes, CPR, Gillham altitude)
- Live tracking via `rtl_adsb` subprocess, native IQ demod via `rtl_sdr` pipe, or direct USB via `rtlsdr_mt`
- FilterEngine wired into live tracking — 8 filter types: military, emergency squawk, circling, holding, proximity, unusual altitude, geofence, rapid descent
- Webhook notifications for filter events (fire-and-forget POST JSON)
- Aircraft enrichment: speed/altitude classification, 26 airline prefixes, 3,642 embedded airports
- Web dashboard with 8 pages (map, table, detail, events, query, replay, receivers, stats)
- Dual-path positions: live tracker serves from memory; DB fallback when no tracker attached
- Multi-receiver ingest API with bearer token auth, heartbeat, and DB persistence
- Graceful shutdown (SIGTERM/SIGINT handling, DB flush, clean exit)
- Configurable data retention: `--retention-hours`, `--downsample-hours`, `--downsample-interval`
- CLI: decode, track, stats, history, export, serve, setup
- CRC error correction: 1-2 bit errors via syndrome table lookup
- 223 Rust tests + 394 Python tests
- Sample config: `config.example.yaml`

### What's implemented but not active:
- **TimescaleDB backend** (`db_pg.rs`) — fully implemented with hypertables, compression, retention, continuous aggregates. Requires a PostgreSQL instance to use. Currently using SQLite.
- **Native RTL-SDR USB** (`LiveCapture` in capture.rs) — direct dongle access via `rtlsdr_mt`. Requires `native-sdr` feature and `librtlsdr` on system. Build with `cargo build --features native-sdr`.
- **Cross-compilation CI** — GitHub Actions configured for Pi 3/4/5 ARM targets. No release binaries published yet.

### Not implemented:
- LLM integration / airspace narrator (discussed as future idea, no code exists)

## Key Technical Details

### CRC-24
- Generator polynomial: 0xFFF409 (ICAO standard)
- LUT built at compile time via `const fn build_crc_table()`
- Syndrome tables for 56-bit and 112-bit messages via `LazyLock<HashMap>`
- `try_fix()` corrects 1-2 bit errors, refuses to touch DF field (bits 0-4)

### Compact Position Reporting (CPR)
- Encodes lat/lon into 17-bit values using Nb=17 zones
- Even and odd frames use different zone counts (NZ=15 base)
- Global decode requires even+odd pair within 10 seconds
- Local decode uses reference position (receiver or last known)

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

### Live Tracking Architecture (Rust)
- Three capture modes: `rtl_adsb` subprocess (hex), `rtl_sdr` subprocess (IQ demod), `rtlsdr_mt` direct USB
- Blocking read loop runs in `tokio::task::spawn_blocking`
- `Tracker` shared via `Arc<RwLock<Tracker>>` with web server
- `FilterEngine` shared via `Arc<Mutex<FilterEngine>>` — checks every frame + proximity every 10s
- `Database` (concrete SQLite) handles writes from main thread
- `SqliteDb` (opens fresh connections per request) handles web reads via `AdsbDatabase` trait
- Web server runs in a tokio task, sees DB writes immediately
- Graceful shutdown via `AtomicBool` + `tokio::sync::Notify` (handles SIGTERM + Ctrl+C)
- Hourly background task prunes old positions, downsamples, removes phantom aircraft

## Division of Labor

**Human does:** Plug in RTL-SDR, position antenna, validate against FlightAware.

**Claude does:** All code, tests, documentation, CLI, web dashboard, DSP, protocol decode, CPR math.

## Testing Strategy

- Unit tests for every module with known test vectors from published ADS-B frames
- CRC validated against ICAO standard polynomial (including error correction)
- CPR math validated against published even/odd pairs
- Gillham gray code altitude decoder tested with range sweep (all 8192 codes)
- Database tests use in-memory SQLite (`:memory:`)
- Cross-validation: same capture file through Python + Rust, every decoded field compared

## Sensitive Data

- **Never commit** capture files (.iq, .bin) — they contain real aircraft data
- **Never commit** database files (.db) — contain position histories
- Receiver coordinates (lat/lon) should use placeholder values in committed docs
- ICAO addresses are public (broadcast over radio) — safe to reference in tests

## Companion Docs

- **SOUL.md** — Agent personality for this project
- **STYLE.md** — Output and documentation voice
- **HOW-IT-WORKS.md** — Deep technical walkthrough of the entire signal chain
- **README.md** — Public-facing project intro
