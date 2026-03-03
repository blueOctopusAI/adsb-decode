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

## Stage 8: The Web Application

Everything up to this point turns radio waves into structured data. Stage 8 is where that data becomes a usable application — a real-time air traffic dashboard that runs in any browser.

**Modules:** Rust `web/` (axum) / Python `web/` (Flask)

### Architecture

The web server runs as part of the same binary that does the tracking. When you start `adsb track --live --port 8080`, a single process handles radio capture, protocol decoding, database writes, and the web dashboard simultaneously. There's no separate frontend build step — the HTML templates are embedded into the Rust binary at compile time.

The server uses **dual-path position serving**: when a live tracker is attached, the `/api/positions` endpoint reads directly from in-memory aircraft state (the `Tracker` struct shared via `Arc<RwLock<Tracker>>`), giving sub-second latency. When serving from an existing database without a dongle, the same endpoint falls back to SQL queries. The dashboard code doesn't know the difference.

The frontend polls every 500ms for position updates and every 2 seconds in 3D mode. All state (map style, toggle preferences, trail duration) persists in `localStorage` so you don't lose your view on refresh.

### The Map (2D)

The primary view is a Leaflet.js map with a dark CARTO tile layer. Aircraft appear as silhouette icons — different shapes for jets, props, turboprops, helicopters, and military aircraft — rotated to match their actual heading. Each aircraft trails an altitude-colored line behind it: green at low altitude, fading through yellow to red at cruise altitude.

A stats overlay in the corner shows live counts: aircraft tracked, total positions, events detected, server uptime. Clicking any aircraft opens a popup with its callsign, altitude, speed, ICAO address, and country.

Six map styles are available — Dark, Satellite, Topo, Streets, Dark Matter, and Voyager — switchable from a control panel and persisted across sessions.

### The Globe (3D)

A toggle switches the entire view to a CesiumJS 3D globe. Aircraft appear at their actual altitudes above the Earth's surface, rendered as heading-rotated billboard entities. Thin vertical "stalks" connect each aircraft to the ground, making altitude visually obvious. Flight level labels float next to each aircraft.

The 3D view has full feature parity with 2D:
- **Heatmap** renders as colored density rectangles on the globe surface (instead of Leaflet's heat layer)
- **Airports** render as billboard entities with labels (instead of Leaflet markers)
- **Trails** render as polylines at altitude (instead of flat lines on a 2D map)
- **Toggle states** (heatmap on/off, airports on/off, trails on/off) sync when you switch between 2D and 3D — your preferences carry over

Satellite imagery comes from ArcGIS; all other tile layers use `UrlTemplateImageryProvider`.

### Historical Aircraft (Ghost Markers)

At longer trail durations (1 hour or more), the dashboard shows aircraft that were recently active but have stopped transmitting. These appear as faded, semi-transparent markers — "ghost" aircraft — so you can see traffic patterns over time, not just what's flying right now.

Ghost aircraft headings are computed from their trail geometry (the bearing between their last two known positions), since they're no longer transmitting heading data directly. At shorter trail settings (5 minutes, 15 minutes, 30 minutes), only live aircraft appear — these function as a pure real-time view.

This is a key difference from other ADS-B tools: most show you a snapshot of the sky right now. This one remembers what flew earlier and shows you the pattern.

### Toggleable Overlay Layers

Several layers can be turned on or off from the map controls:

- **Heatmap** — Position density visualization with a time window slider (15 minutes to 7 days). Uses server-side grid aggregation with zoom-aware resolution. Normalized density values so the color scale adapts to the data.
- **Airports** — 3,642 US airports from the OurAirports dataset, embedded at compile time. Filterable by class: Major, Medium, Small. Each airport marker is clickable with links to AirNav and SkyVector. Works in both 2D and 3D.
- **Event markers** — Detected events (military sightings, emergency squawks, circling aircraft, etc.) appear as color-coded circle markers on the map with tooltips and popups.
- **Military highlights** — Pulsing red rings behind confirmed military aircraft, making them immediately visible on a crowded map.

### Aircraft Detail Page

Clicking through to an individual aircraft opens a split-screen detail view:

**Left panel:**
- Trail map showing every captured position for that aircraft
- Event history (any anomalies detected)
- Position table with timestamps, altitude, speed, heading

**Right panel:**
- External data from hexdb.io — manufacturer, aircraft type, registered owner
- Quick-link cards to ADSBExchange, Planespotters, FlightAware, FlightRadar24, FAA Registry, and OpenSky
- Altitude profile chart showing the aircraft's vertical history

The hexdb.io lookup is proxied through the server (`/api/lookup/:icao`) so the browser doesn't need to make cross-origin requests. Results are cached per ICAO address.

### Additional Dashboard Pages

Beyond the map, the dashboard includes seven more pages:

- **Table** — Sortable aircraft list with search box, military/live/country filters, 500-row cap with pagination hint. Each row links to the detail page.
- **Events** — Color-coded event log with type filter buttons (military, emergency, circling, etc.). Events are auto-enriched with aircraft type and owner data from hexdb.io.
- **Query Builder** — Preset filters (military aircraft, low altitude, fast movers) plus custom parameters (min/max altitude, ICAO address, time range). Results render on a map.
- **Historical Replay** — Time slider with play/pause and adjustable speed (1x through 10-minute jumps). Replays position history from the database.
- **Receivers** — Connected feeder nodes with online/offline status and coverage circles on a map.
- **Stats** — Database summary: aircraft count, position count, event count, capture history, receiver health.

---

## Stage 9: Intelligence Layer

What makes this more than a radio scanner. The intelligence layer runs alongside tracking and generates events in real time.

**Modules:** Rust `filter.rs` + `enrich.rs` / Python `filters.py` + `enrichment.py`

### Detection Filters

The `FilterEngine` checks every decoded frame against eight filter types:

- **Military detection** — ICAO address in military allocation block, or callsign matches known military patterns (RCH = C-17 Globemaster, DUKE = Army, REACH = Air Mobility Command, DOOM, JAKE, etc.)
- **Emergency squawks** — 7500 (hijack), 7600 (radio failure), 7700 (general emergency) trigger immediate alerts
- **Rapid descent** — Vertical rate exceeding -5,000 ft/min
- **Unusual altitude** — Fast aircraft (>200 kts) below 3,000 ft with no airport within 15nm
- **Circling/loitering** — Cumulative heading change exceeding 360 degrees within a 5-minute window. Handles compass wraparound correctly.
- **Holding patterns** — Stable altitude (within 500 ft) combined with reciprocal headings (180 degrees apart, within 30-degree tolerance). Detected via a 10-degree heading histogram.
- **Proximity alerts** — Two aircraft within configurable distance (default 5nm horizontal, 1,000 ft vertical). Sorted pair key prevents duplicate alerts for the same pair.
- **Geofence alerts** — User-configurable lat/lon/radius zones. Aircraft entering a geofence trigger an event.

All filters use `emit()` deduplication — once an alert fires for a specific aircraft, it won't fire again for the same condition until the condition clears.

### Enrichment

When a filter event fires, the system automatically enriches it:
- **Aircraft type classification** — Speed and altitude profile classifies the aircraft as jet, prop, turboprop, helicopter, military, or cargo
- **Operator lookup** — Airline ICAO prefix maps to operator name (26 carriers covered)
- **hexdb.io enrichment** — Automatic lookup of registration, manufacturer, type, and owner. Cached per ICAO address. Fire-and-forget via `tokio::spawn` so it never blocks tracking.
- **Airport awareness** — 3,642 US airports. Nearest airport lookup, flight phase classification (approaching, departing, overflying).

---

## Multi-Receiver Network

The system supports distributed coverage through a hub-and-spoke architecture:

```
[Pi + Dongle] --HTTP POST--> [Central Server] <--Browser-- [Dashboard]
[Pi + Dongle] --HTTP POST-->      |
[Mac + Dongle] --HTTP POST-->     |
                            Axum API + SQLite
```

The **feeder agent** (`adsb-feeder` binary) runs on each receiver node — a Raspberry Pi with an RTL-SDR dongle, a laptop, any machine with a radio. It captures frames, decodes them locally, and batches position data into HTTP POST requests to the central server.

The **ingest API** (`POST /api/v1/frames`) accepts frame batches with bearer token authentication. A heartbeat endpoint (`POST /api/v1/heartbeat`) tracks whether each feeder is online. The receiver management page shows connection status and coverage circles.

Each receiver node costs about $60 (Pi + dongle + antenna). The central server aggregates data from all feeders into one database and one dashboard.

---

## The CLI

The `adsb` binary provides seven commands:

| Command | What it does |
|---------|-------------|
| `decode` | Parse a file of hex frames and print an aircraft table. Quick way to inspect a capture. |
| `track` | The main command. Process a capture file or start live tracking from an RTL-SDR dongle. Writes to SQLite. Add `--port 8080` to launch the web dashboard alongside tracking. |
| `serve` | Start the web dashboard from an existing database, without a dongle. View historical data. |
| `stats` | Print database summary: aircraft count, position count, event count, capture history. |
| `history` | Show aircraft sighting history — when each aircraft was first and last seen. |
| `export` | Export position data to CSV or JSON. |
| `setup` | Interactive setup wizard — detect hardware, configure receiver, database, and server settings. |

Live tracking supports three capture modes:
- `--live` — Spawn `rtl_adsb` as a subprocess (simplest, most compatible)
- `--native-demod` — Spawn `rtl_sdr` and pipe raw IQ through our own demodulator
- `--native-usb` — Direct USB access via `rtlsdr_mt` (requires `native-sdr` feature flag)

---

## API Endpoints

The REST API serves both the dashboard frontend and external consumers.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/aircraft` | All tracked aircraft (optional `?military=true`) |
| GET | `/api/aircraft/<icao>` | Single aircraft detail + positions + events |
| GET | `/api/positions` | Most recent position per aircraft (optional `?minutes=` time filter) |
| GET | `/api/positions/all` | All positions ordered by time (for replay) |
| GET | `/api/trails` | Position trails per aircraft (`?minutes=` time window) |
| GET | `/api/lookup/<icao>` | External aircraft metadata via hexdb.io (cached) |
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

---

## Export Formats

- **CSV** — Flat position data: timestamp, ICAO, lat, lon, altitude, speed, heading
- **JSON** — Structured aircraft objects with nested position arrays
- **KML** — Google Earth flight paths (Python implementation only)
- **GeoJSON** — Map-ready feature collections (Python implementation only)

---

## The Python-to-Rust Story

The project started as a Python reference implementation — 22 modules, 394 tests, ~7,300 lines. Every stage of the protocol was built and validated in Python first: the demodulator with numpy, the CRC with bitwise operations, the CPR math with floating point, the tracker, the database, the Flask dashboard.

Then the entire system was rewritten in Rust — a language the developer had never used before. The rewrite took a weekend, pair-programming with Claude Code. The Rust implementation is a 3-crate workspace (~12,000 lines) with 223 tests across `adsb-core` (pure library, no async), `adsb-feeder` (edge device binary), and `adsb-server` (web server + CLI + database).

Cross-validation confirmed correctness: the same 296-frame capture file produces 100% matching output between Python and Rust. Every decoded field — ICAO address, callsign, position, altitude, speed, heading — matches exactly.

The Rust version is now the primary codebase. It's faster, compiles to a single binary, and the type system catches entire categories of bugs at compile time. The Python version remains as the reference oracle.

---

## Implemented but Not Active

These features are fully coded but not currently in use:

- **TimescaleDB backend** (`db_pg.rs`) — A complete PostgreSQL backend with hypertables for time-series data, automatic compression (positions older than 7 days), retention policies (90-day positions, 365-day events), and continuous aggregates for 30-second and 5-minute rollups. Behind a `timescaledb` feature flag. The system currently uses SQLite, which handles the current data volume fine. TimescaleDB is there for when the database grows beyond what SQLite handles gracefully.

- **Native RTL-SDR USB access** (`LiveCapture` in capture.rs) — Direct dongle access via the `rtlsdr_mt` crate, bypassing `rtl_adsb` entirely. Requires `librtlsdr` on the system and the `native-sdr` feature flag. Built and tested, but the subprocess approach works reliably and is easier to set up.

- **Cross-compilation CI** — GitHub Actions configured for ARM targets (aarch64 for Pi 4/5, armv7 for Pi 3) using `cross`. The CI runs but release binaries haven't been published to GitHub Releases yet.

---

## Future Ideas

These are concepts that have been discussed but do not exist in code yet:

- **Airspace narrator** — An LLM integration that watches the event stream and produces natural-language summaries of what's happening in the sky. "Three military aircraft circling over the mountains west of Asheville. A Delta flight just declared an emergency squawk at FL350." The intelligence layer already generates the structured events — the narrator would translate them into plain English.

- **Pattern analysis over time** — With weeks or months of historical data, identify recurring patterns: regular military training routes, airline schedule adherence, seasonal traffic changes, unusual activity baselines. The database schema already supports long-term storage; the analysis layer doesn't exist yet.

- **Alerting and notifications** — Push notifications (email, SMS, webhook) when specific conditions are met: a military aircraft enters your area, an emergency squawk is detected, a geofence is breached. The webhook infrastructure exists for filter events, but there's no user-facing notification configuration.

- **Fleet tracking** — Track specific aircraft by ICAO address or callsign pattern over time. Build profiles of individual aircraft: where they go, how often, typical routes. The database stores per-aircraft history; the fleet analysis layer doesn't exist yet.

- **Collaborative network** — Multiple adsb-decode users sharing data to build broader coverage. The multi-receiver architecture is built for one operator with multiple dongles. Extending it to multiple operators would require authentication, data sharing agreements, and a hosted central server.
