# HOW-IT-WORKS.md — adsb-decode

## The Signal Chain

This document traces an ADS-B message from the antenna to the screen. Each stage corresponds to modules in both the Rust (`rust/adsb-core/src/`) and Python (`src/`) implementations. The protocol-level details are identical — only the implementation language differs.

---

## Stage 0: The Physics

ADS-B (Automatic Dependent Surveillance-Broadcast) is a surveillance technology where aircraft broadcast their identity, position, altitude, and velocity on **1090 MHz**. Any receiver tuned to that frequency can hear every aircraft within line-of-sight range (~100-200 nm depending on altitude and antenna).

The signal uses **Pulse Position Modulation (PPM)** at 1 Megabit/second. Each bit occupies 1 microsecond. A '1' bit has energy in the first half-microsecond; a '0' bit has energy in the second half.

Messages are either **56 bits** (short, Mode S surveillance) or **112 bits** (long, ADS-B extended squitter). Every message begins with an 8-microsecond **preamble** — four pulses at positions 0, 1, 3.5, and 4.5 microseconds — that receivers use to detect the start of a frame.

---

## Stage 1: Capture

The RTL-SDR dongle is a software-defined radio that samples the 1090 MHz band and produces **IQ (In-phase/Quadrature) samples** — pairs of 8-bit unsigned integers representing the signal's amplitude and phase.

- **Sample rate:** 2 MHz (2 million sample pairs per second)
- **Sample format:** Interleaved uint8 pairs [I₀, Q₀, I₁, Q₁, ...]
- **Center frequency:** 1090 MHz

We also support pre-demodulated frame input — hex strings from tools like `rtl_adsb` or `dump1090 --raw` — for testing without raw IQ processing.

**Current live capture:** The Rust implementation uses `rtl_adsb` subprocess for hex frame input. The Python implementation has native IQ demodulation via `pyrtlsdr` (reading raw IQ bytes directly from the USB dongle and piping through our own demodulator). Both also support file-based input (hex frame files and raw IQ files).

| | Rust | Python |
|---|---|---|
| Hex frame files | `capture.rs` FrameReader | `capture.py` FrameReader |
| Raw IQ files | `capture.rs` IQReader | `capture.py` IQReader |
| Live capture | `main.rs` spawns `rtl_adsb` | `capture.py` LiveDemodCapture (pyrtlsdr) |
| Demod module | `demod.rs` (implemented, not wired to live) | `demodulator.py` (active in live path) |

---

## Stage 2: Demodulation

Raw IQ samples must be converted to magnitude and searched for ADS-B messages.

**Modules:** Rust `demod.rs` / Python `demodulator.py`

### IQ to Magnitude

For each sample pair (I, Q), compute magnitude:
```
magnitude = sqrt(I² + Q²)
```

In practice we use `I² + Q²` (squared magnitude) to avoid the sqrt — relative comparisons still work. The Rust implementation uses a compile-time `const [[f32; 256]; 256]` lookup table for magnitude values.

### Preamble Detection

Slide a window across the magnitude signal looking for the preamble pattern:
- Pulses at sample positions 0, 2, 7, 9 (at 2 MHz sample rate)
- Gaps (low energy) at positions 1, 3-6, 8
- The pulse amplitudes should be roughly equal
- The gaps should be significantly lower than the pulses

When a valid preamble is found, extract the next 112 samples (56 µs) as the message data.

### Bit Recovery

Each bit occupies 2 samples (at 2 MHz). Compare the first sample to the second:
- First > Second → bit is '1'
- First ≤ Second → bit is '0'

This gives us a raw bitstream of 56 or 112 bits.

---

## Stage 3: Frame Parsing + CRC

**Modules:** Rust `frame.rs` + `crc.rs` / Python `frame_parser.py` + `crc.py`

### CRC-24 Validation

Every Mode S message includes a 24-bit CRC computed over the message body using the ICAO polynomial:

```
Generator: x²⁴ + x²³ + x²¹ + x²⁰ + x¹⁷ + x¹⁵ + x¹³ + x¹² + x¹⁰ + x⁸ + x⁵ + x⁴ + x³ + 1
Hex: 0xFFF409
```

For DF17 (ADS-B) messages, the last 24 bits are pure CRC — valid messages produce remainder 0x000000.

For DF11 (all-call) messages, the last 24 bits are XORed with the ICAO address — we recover the address by XORing the CRC remainder with the known polynomial result.

### CRC Error Correction

Both implementations include syndrome-table-based error correction for 1-2 bit errors. Pre-built lookup tables map CRC syndromes (the non-zero remainder of a corrupted message) to the bit position(s) that are flipped. Safety: never corrects bits 0-4 (the DF field) to avoid turning one message type into another.

The Rust CRC LUT is built at compile time via `const fn build_crc_table()`. Syndrome tables use `LazyLock<HashMap<u32, Vec<usize>>>` initialized on first access.

### Downlink Format Classification

The first 5 bits of every frame encode the **Downlink Format (DF)**:

| DF | Bits | Name | Content |
|----|------|------|---------|
| 0 | 56 | Short air-air surveillance | Altitude |
| 4 | 56 | Surveillance altitude reply | Altitude, flight status |
| 5 | 56 | Surveillance identity reply | Squawk code |
| 11 | 56 | All-call reply | ICAO address acquisition |
| 16 | 112 | Long air-air surveillance | Altitude + extended |
| 17 | 112 | ADS-B extended squitter | **The main event** |
| 18 | 112 | TIS-B / ADS-R | Ground station relayed |
| 20 | 112 | Comm-B altitude reply | Altitude + BDS data |
| 21 | 112 | Comm-B identity reply | Squawk + BDS data |

**Output:** `ModeFrame` struct with DF, ICAO address (`[u8; 3]`), raw message bytes, timestamp, signal level.

---

## Stage 4: Decoding

**Modules:** Rust `decode.rs` + `cpr.rs` / Python `decoder.py` + `cpr.py`

### DF17 Extended Squitter — ADS-B Messages

The 56-bit ME (Message Extended) field in DF17 frames carries the ADS-B payload. The first 5 bits are the **Type Code (TC)**, which determines the message structure:

### TC 1-4: Aircraft Identification

Encodes the callsign (flight number) as 8 characters from a 64-character alphabet:
```
#ABCDEFGHIJKLMNOPQRSTUVWXYZ##### 0123456789######
```
Each character is 6 bits. 8 characters × 6 bits = 48 bits.

### TC 9-18: Airborne Position

Contains:
- **Surveillance status** (2 bits): alert, SPI, temporary alert
- **Single antenna flag** (1 bit)
- **Altitude** (12 bits): Gillham-coded, 25-ft or 100-ft increments
- **CPR format** (1 bit): even (0) or odd (1)
- **CPR latitude** (17 bits): encoded latitude
- **CPR longitude** (17 bits): encoded longitude

The position is encoded using **Compact Position Reporting (CPR)** — see below.

### TC 19: Airborne Velocity

Two sub-types:
- **Subtype 1-2:** Ground speed — separate East-West and North-South velocity components
- **Subtype 3-4:** Airspeed — heading + airspeed (IAS or TAS)

Also includes vertical rate (barometric or GNSS) in 64 ft/min increments.

### Squawk Code (DF5, DF21)

The 13-bit Identity field encodes a 4-digit octal squawk code using Gillham coding. Special codes:
- **7500**: Hijack
- **7600**: Radio failure
- **7700**: Emergency

### Altitude (DF0, DF4, DF20)

The 13-bit Altitude Code uses either:
- **25-ft mode**: M-bit = 0, Q-bit = 1 → altitude = (code × 25) - 1000
- **100-ft mode**: M-bit = 0, Q-bit = 0 → Gillham gray code (interleaved bit extraction → octal digit construction → dual gray code transformation → altitude in 100-ft increments)

---

## Stage 4a: CPR Decoding

**Modules:** Rust `cpr.rs` / Python `cpr.py`

CPR is the trickiest part of ADS-B. It compresses latitude and longitude into 17-bit values using a zone system.

### The Problem

Latitude ranges from -90° to +90° (180°), longitude from -180° to +180° (360°). With 17 bits (131,072 values), raw encoding gives ~0.001° resolution latitude (~110m) but only ~0.003° longitude (~330m at equator). CPR does better.

### The Solution: Zone Pairs

CPR divides the Earth into **NZ = 15** latitude zones per hemisphere (60 total). Aircraft alternate between **even** and **odd** frames, which use slightly different zone counts:

- Even frame: NZ zones
- Odd frame: NZ - 1 zones (14)

By combining one even and one odd frame, the receiver can determine which zone the aircraft is in and compute a precise position.

### Global Decode (two-frame)

Given an even frame (lat_even, lon_even) and odd frame (lat_odd, lon_odd):

1. Compute latitude zone index j from both encoded latitudes
2. Compute candidate latitudes for even and odd
3. Verify both latitudes fall in the same longitude zone count (NL)
4. If NL matches, compute longitude from the most recent frame
5. Result: latitude and longitude to ~5.1m precision

### Local Decode (single-frame + reference)

If we have a known reference position (receiver location or last decoded position) within 180 nm:

1. Use the reference to determine the latitude zone
2. Decode latitude from the single frame
3. Use reference to determine longitude zone
4. Decode longitude
5. Result: valid if aircraft hasn't moved more than ~180 nm from reference

### Edge Cases

- **Zone boundaries**: Even/odd frames may indicate different NL values → discard the pair and wait for a new one
- **Polar regions**: Zone counts change near the poles
- **Antimeridian**: Longitude wrapping at ±180°
- **Timestamp check**: Even/odd pair must be within 10 seconds of each other

---

## Stage 5: Tracking

**Modules:** Rust `tracker.rs` + `icao.rs` / Python `tracker.py` + `icao.py`

### ICAO Address Resolution

Every aircraft has a unique 24-bit ICAO address assigned by its country of registration:

- **Country lookup**: Address ranges are allocated by ICAO (e.g., 0xA00000-0xAFFFFF = USA)
- **Military blocks**: Some address ranges are reserved for military aircraft
- **N-number algorithm**: US civil aircraft addresses (0xA00001-0xADF7C7) can be converted back to the N-number registration using a base-conversion algorithm

### State Machine

Each aircraft (by ICAO address) maintains a state object:
- Current position (lat, lon, altitude)
- Current velocity (ground speed, heading, vertical rate)
- Callsign and squawk
- CPR frame buffer (last even and odd frames for position decoding)
- Last update timestamp
- Heading history (for circling/holding detection)

The tracker produces `TrackEvent` enum outputs:
- `NewAircraft` — first time seeing this ICAO address
- `AircraftUpdate` — subsequent message from known aircraft
- `PositionUpdate` — new decoded position
- `SightingUpdate` — callsign/squawk/altitude change

This separates pure decode/track logic from I/O — the database layer consumes events.

---

## Stage 6: Persistence

**Modules:** Rust `db.rs` / Python `database.py`

SQLite database with WAL (Write-Ahead Logging) mode for concurrent read/write. Schema:

- **receivers**: Registered sensor nodes. Each receiver has a name, lat/lon, altitude, and description. Designed for distributed deployment — multiple receivers feeding a single database.
- **aircraft**: One row per unique ICAO address seen. Accumulates country, registration, military flag (sticky via MAX — once set to true, never reverts).
- **sightings**: One row per capture session per aircraft. Tracks callsign, squawk, min/max altitude, message count.
- **positions**: Time-series position data. Lat, lon, altitude, speed, heading, vertical rate. Tagged with `receiver_id`.
- **captures**: Metadata per capture session. Source file, start/end time, frame counts. Tagged with `receiver_id`.
- **events**: Anomalies detected by filters — emergency squawk, military, circling, unusual altitude, geofence.

### Data Retention

Both implementations have retention/pruning functions:

- **`prune_positions(max_age_hours)`** — deletes old positions
- **`prune_events(max_age_hours)`** — deletes old events
- **`downsample_positions(older_than_hours, keep_interval_sec)`** — bucket-based thinning (keeps one position per bucket per aircraft)
- **`prune_phantom_aircraft(min_age_hours)`** — removes aircraft with no positions (CRC-residual extraction artifacts)

The Python implementation runs these automatically every 10 minutes during live tracking with tiered policies (24h→30s, 7d→60s, 30d→delete). The Rust implementation has the functions but does not yet schedule them automatically.

### Database Trait (Rust)

The Rust implementation defines an `AdsbDatabase` async trait with 13 methods, enabling backend swapping:
- **`SqliteDb`** — stateless wrapper, opens fresh connections per request (used by web server)
- **`TimescaleDb`** — PostgreSQL with hypertables, compression (>7d), retention (90d positions, 365d events), continuous aggregates (30s + 5m). Behind `timescaledb` feature flag. Not currently in use.

---

## Stage 7: Intelligence

**Modules:** Rust `filter.rs` + `enrich.rs` / Python `filters.py` + `enrichment.py`

What makes this more than a radio scanner:

- **Military detection**: ICAO address in military allocation block, or callsign matches military patterns (e.g., RCH*, DOOM*, JAKE*)
- **Emergency squawks**: 7500 (hijack), 7600 (radio failure), 7700 (emergency)
- **Rapid descent**: Vertical rate exceeding -5,000 ft/min
- **Low altitude**: Aircraft below 500 ft AGL (excludes ground)
- **Circling/loitering**: Cumulative heading change >360° within 5 minutes, handles wraparound
- **Holding patterns**: Stable altitude (±500 ft) + reciprocal headings (180°±30°) detected via 10° heading bins
- **Proximity alerts**: Two aircraft within configurable distance (default 5nm horizontal, 1,000 ft vertical). Sorted pair key prevents duplicate alerts.
- **Unusual altitude**: Fast aircraft (>200 kts) below 3,000 ft with no airport within 15nm
- **Geofence alerts**: Aircraft entering a configured lat/lon/radius zone
- **Aircraft type enrichment**: Speed/altitude profile classification (jet, prop, turboprop, helicopter, military, cargo). Airline ICAO prefix → operator name lookup (26 carriers).
- **Airport awareness**: 3,642 US airports from OurAirports dataset, embedded at compile time. Nearest airport lookup, flight phase classification (approaching, departing, overflying).

All filters use `emit()` deduplication to prevent repeated alerts for the same aircraft.

---

## Stage 8: Display

### Web Dashboard

**Modules:** Rust `web/` (axum) / Python `web/` (Flask)

Both implementations serve the same dashboard functionality:
- Leaflet.js map with dark CARTO tiles
- Aircraft silhouette icons (jet/prop/turboprop/helicopter/military) with heading rotation
- Altitude-colored trail lines (green→yellow→red gradient)
- Click-to-detail popups
- Stats overlay: aircraft count, positions, events, uptime
- Heatmap layer toggle
- 6 switchable map styles: Dark, Satellite, Topo, Streets, Dark Matter, Voyager
- Airport overlay: 3,642 US airports with Major/Medium/Small toggles
- Events dashboard with type filter buttons
- Query builder with preset filters and custom parameters
- Historical replay with time slider, play/pause, adjustable speed
- Receiver management page with coverage circles
- Aircraft detail page with position history and external intel (hexdb.io)
- 500ms polling for real-time updates
- **Dual-path position serving**: When a live tracker is attached, `/api/positions` reads from in-memory aircraft state for sub-second latency. Otherwise falls back to DB queries.

### Multi-Receiver Network

- **Feeder agent**: Runs on remote Pi/machine with dongle. Captures frames, batches, and POSTs to central server.
- **Ingest API**: `POST /api/v1/frames` with bearer token auth. Heartbeat monitoring.
- **Architecture**: Hub-and-spoke. Multiple feeders → one central server → one dashboard.

### CLI

- Decode: parse hex frames, print aircraft table
- Track: process capture file or live dongle, write to DB
- Stats: database summary
- History: aircraft sighting history
- Export: CSV/JSON output
- Serve: web dashboard from existing DB
- Setup: hardware detection (Python only currently)

### Export

- **CSV**: Flat position data
- **JSON**: Structured aircraft + position data
- **KML**: Google Earth flight paths (Python only currently)
- **GeoJSON**: Map-ready feature collections (Python only currently)

---

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/aircraft` | All tracked aircraft (optional `?military=true`) |
| GET | `/api/aircraft/<icao>` | Single aircraft detail + positions + events |
| GET | `/api/positions` | Most recent position per aircraft (optional `?minutes=` time filter) |
| GET | `/api/positions/all` | All positions ordered by time (for replay) |
| GET | `/api/trails` | Position trails per aircraft (`?minutes=` time window) |
| GET | `/api/lookup/<icao>` | External aircraft metadata via hexdb.io |
| GET | `/api/query` | Filtered positions (min/max alt, ICAO, military, limit) |
| GET | `/api/airports` | 3,642 US airports with type classification |
| GET | `/api/heatmap` | Position density data for heatmap layer |
| GET | `/api/geofences` | List configured geofence zones |
| POST | `/api/geofences` | Create a geofence zone |
| DELETE | `/api/geofences/<id>` | Delete a geofence zone |
| GET | `/api/events` | Recent events (optional `?type=` filter) |
| GET | `/api/stats` | Database stats, receiver info |
| POST | `/api/v1/frames` | Ingest frames from remote feeder (auth required) |
| POST | `/api/v1/heartbeat` | Feeder status heartbeat |
| GET | `/api/v1/receivers` | List connected receivers |
