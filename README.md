# adsb-decode

ADS-B radio protocol decoder — from raw 1090 MHz radio signals to identified aircraft on a map.

Built from scratch. No dump1090 dependency, no borrowed decoders. Every byte of the protocol is decoded and documented, from the preamble pulse pattern to the Compact Position Reporting trigonometry.

**Rust is the primary implementation.** Python exists as the original reference decoder used to prove the protocol math. Both decode identically — cross-validated on a 296-frame capture file with 100% field-level match.

---

## Table of Contents

- [What It Does](#what-it-does)
- [Quick Start](#quick-start)
  - [Decode a capture file](#decode-a-capture-file)
  - [Live tracking with a dongle](#live-tracking-with-a-dongle)
  - [Set up a remote receiver (Pi)](#set-up-a-remote-receiver-pi)
- [The Three Binaries](#the-three-binaries)
- [Verify It Works](#verify-it-works)
- [The Signal Chain](#the-signal-chain)
- [Hardware](#hardware)
- [Intelligence Features](#intelligence-features)
- [Web Dashboard](#web-dashboard)
- [Multi-Receiver Network](#multi-receiver-network)
- [Why This Exists](#why-this-exists)
- [Project Structure](#project-structure)
- [Python Reference Implementation](#python-reference-implementation)
- [Future](#future)

---

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

## Quick Start

> **Prerequisite:** All live-capture paths require RTL-SDR drivers. Install them first:
> ```bash
> sudo apt install rtl-sdr
> ```
> Then blacklist the DVB kernel module so it doesn't conflict:
> ```bash
> echo 'blacklist dvb_usb_rtl28xxu' | sudo tee /etc/modprobe.d/blacklist-rtlsdr.conf
> sudo modprobe -r dvb_usb_rtl28xxu
> ```

### Decode a capture file

No hardware needed. Good for testing or analyzing recorded data.

```bash
cd rust
cargo build --release

# Decode and display
cargo run --bin adsb -- decode data/live_capture.txt

# Decode with tracking + database
cargo run --bin adsb -- track data/live_capture.txt --db-path data/flights.db

# Export flight paths from database
cargo run --bin adsb -- export --db-path data/flights.db --format json

# Database statistics
cargo run --bin adsb -- stats --db-path data/flights.db
```

### Live tracking with a dongle

Plug in an RTL-SDR dongle and track aircraft in real time with a web dashboard.

```bash
cd rust
cargo build --release

# Live tracking with web dashboard (opens on port 8080)
cargo run --bin adsb -- track --live --port 8080

# Serve dashboard from existing database (no dongle needed)
cargo run --bin adsb -- serve --db-path data/adsb.db --port 8080
```

Then open `http://127.0.0.1:8080` in your browser.

### Set up a remote receiver (Pi)

Deploy a dedicated receiver that feeds data to a central server. One command does everything — installs drivers, downloads the binary, configures systemd:

```bash
curl -sL https://raw.githubusercontent.com/blueOctopusAI/adsb-decode/main/deploy/receiver-setup.sh | sudo bash
```

Or set it up manually — see the [Receiver Setup](#multi-receiver-network) section and [`deploy/receiver-setup.sh`](deploy/receiver-setup.sh) for details.

## The Three Binaries

The Rust workspace produces three binaries:

| Binary | Crate | Purpose |
|--------|-------|---------|
| `adsb` | `adsb-server` | The main tool. Decode files, track live, serve the web dashboard, query the database, export data. This is what most users want. |
| `adsb-receiver` | `adsb-receiver` | Headless receiver daemon. Captures frames from an RTL-SDR dongle and feeds them to a central `adsb` server over HTTP. Designed for Pis and remote stations. |
| `adsb-feeder` | `adsb-feeder` | Offline demodulation tool. Reads raw IQ sample files and decodes them. For signal processing work, not typical use. |

Build all three:
```bash
cd rust
cargo build --release
# Binaries at target/release/adsb, target/release/adsb-receiver, target/release/adsb-feeder
```

## Verify It Works

After building, confirm everything is working:

```bash
# Run the test suite
cd rust
cargo test --workspace

# Decode the included capture file — you should see aircraft
cargo run --bin adsb -- decode data/live_capture.txt

# Expected output: a table of aircraft with ICAO addresses, callsigns, altitudes
# If you see "Total frames: 296" and "Aircraft seen: 41" — it's working.
```

If you're running a receiver, check that frames are flowing:
```bash
sudo journalctl -u adsb-receiver -f
# You should see "[sender] POST 200" lines every few seconds
```

Then check the [receivers dashboard](https://adsb.blueoctopustechnology.com/receivers) to see your station online.

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
- **3D globe view** — CesiumJS toggle shows aircraft at real altitudes with heading-rotated billboards, altitude stalks, flight level labels, and live updates. Full feature parity with 2D: heatmap, airports, trails, and toggle states all carry over between modes.
- **Historical aircraft** — At trail durations >= 1h, aircraft that stopped transmitting appear as faded ghost markers with computed headings. Works in both 2D and 3D.
- **Event markers** — Toggle to overlay detected events (military, emergency, circling, etc.) as color-coded markers directly on the map
- **Military highlight** — Toggle to add pulsing red rings behind military aircraft
- **Trail duration slider** — 5 minutes to 24 hours. Short windows (5m-30m) show live traffic only. Longer windows (1h+) include historical aircraft as faded markers.
- **Aircraft detail** — Split-screen view. Left: captured trail map, events, position history. Right: external intel from hexdb.io (manufacturer, type, owner), link cards to ADSBExchange/Planespotters/FlightAware/FlightRadar24/FAA Registry/OpenSky, altitude profile chart.
- **Airport overlay** — 3,642 US airports with Major/Medium/Small toggles. Click for details + AirNav/SkyVector links. Works in both 2D (Leaflet markers) and 3D (Cesium billboards).
- **Heatmap** — Position density visualization. 2D uses Leaflet heat layer; 3D renders colored density rectangles on the globe.
- **Map styles** — Dark, Satellite, Topo, Streets, Dark Matter, Voyager (persisted in localStorage)
- **Events dashboard** — Color-coded events with type filters, auto-enriched with aircraft type/owner from hexdb.io
- **Query builder** — Preset queries (military, low altitude, fast) + custom filters with map visualization
- **Historical replay** — Time slider with play/pause, adjustable speed (1x-10min)
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

- **Receiver agent** (`adsb-receiver` binary) runs on each receiver node — single binary, no Python
- Bearer token authentication for frame ingestion
- Heartbeat monitoring with online/offline status
- Environment variable + CLI flag configuration for systemd deployment
- ~$60/node (Pi + dongle + antenna)

### Receiver Setup

Set up a remote receiver to feed data to a central server:

```bash
# One-command automated setup (installs drivers, binary, systemd service):
curl -sL https://raw.githubusercontent.com/blueOctopusAI/adsb-decode/main/deploy/receiver-setup.sh | sudo bash

# Or manually:
# 1. Install RTL-SDR drivers: sudo apt install rtl-sdr
# 2. Download the binary for your architecture (see deploy/receiver-setup.sh)
# 3. Run directly:
adsb-receiver --server https://adsb.blueoctopustechnology.com --name my-pi --api-key YOUR_KEY

# Or configure via environment variables for systemd:
# See deploy/receiver-setup.sh for the full automated path
```

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
├── adsb-feeder/              # Binary: offline demodulation tool
│   └── src/
│       ├── main.rs           # Capture → decode (file-based)
│       └── capture.rs        # FrameReader, IQReader, LiveDemodCapture
│
├── adsb-receiver/            # Binary: networked receiver daemon
│   └── src/
│       ├── main.rs           # CLI (clap + env vars), startup, Ctrl+C shutdown
│       ├── capture.rs        # rtl_adsb subprocess management
│       ├── sender.rs         # HTTP client, batch POST, heartbeat
│       └── stats.rs          # Atomic counters (frames, uptime)
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

## Python Reference Implementation

The Python implementation (`src/`) is the original reference decoder — 22 modules, 394 tests, ~7,300 lines. It was used to prove the protocol math before the Rust rewrite. Both decode identically and are cross-validated in CI.

```bash
pip install -e ".[dev]"
bash scripts/setup-rtlsdr.sh
adsb decode data/live_capture.txt
adsb track --live --port 8080
```

## Future

- **TimescaleDB production backend** — Implemented behind a feature flag (`db_pg.rs`). Includes hypertables, compression policies, retention policies, and continuous aggregates. Requires a PostgreSQL instance to activate.
- **Cross-compiled Pi binaries** — GitHub Actions CI configured for aarch64 (Pi 4/5) and armv7 (Pi 3) targets via `cross` for all three binaries (feeder, receiver, server).

## License

MIT
