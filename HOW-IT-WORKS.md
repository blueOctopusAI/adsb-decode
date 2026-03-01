# HOW-IT-WORKS.md — adsb-decode

## The Signal Chain

This document traces an ADS-B message from the antenna to the screen. Every stage corresponds to a module in `src/`.

---

## Stage 0: The Physics

ADS-B (Automatic Dependent Surveillance-Broadcast) is a surveillance technology where aircraft broadcast their identity, position, altitude, and velocity on **1090 MHz**. Any receiver tuned to that frequency can hear every aircraft within line-of-sight range (~100-200 nm depending on altitude and antenna).

The signal uses **Pulse Position Modulation (PPM)** at 1 Megabit/second. Each bit occupies 1 microsecond. A '1' bit has energy in the first half-microsecond; a '0' bit has energy in the second half.

Messages are either **56 bits** (short, Mode S surveillance) or **112 bits** (long, ADS-B extended squitter). Every message begins with an 8-microsecond **preamble** — four pulses at positions 0, 1, 3.5, and 4.5 microseconds — that receivers use to detect the start of a frame.

---

## Stage 1: Capture (`capture.py`)

The RTL-SDR dongle is a software-defined radio that samples the 1090 MHz band and produces **IQ (In-phase/Quadrature) samples** — pairs of 8-bit unsigned integers representing the signal's amplitude and phase.

- **Sample rate:** 2 MHz (2 million sample pairs per second)
- **Sample format:** Interleaved uint8 pairs [I₀, Q₀, I₁, Q₁, ...]
- **Center frequency:** 1090 MHz

We also support pre-demodulated frame input — hex strings from tools like `rtl_adsb` or `dump1090 --raw` — for testing without raw IQ processing.

**Files:** `capture.py` provides `IQReader` (raw samples), `FrameReader` (hex frames), and `LiveCapture` (real-time from dongle). `LiveCapture` uses `pyrtlsdr` to read raw IQ bytes directly from the USB dongle and pipes them through our `demodulator.py` — no external demodulation tools in the signal path. An `rtl_adsb` fallback exists only for systems without `pyrtlsdr` installed.

---

## Stage 2: Demodulation (`demodulator.py`)

Raw IQ samples must be converted to magnitude and searched for ADS-B messages.

### IQ to Magnitude

For each sample pair (I, Q), compute magnitude:
```
magnitude = sqrt(I² + Q²)
```

In practice we use `I² + Q²` (squared magnitude) to avoid the sqrt — relative comparisons still work.

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

## Stage 3: Frame Parsing (`frame_parser.py`, `crc.py`)

### CRC-24 Validation

Every Mode S message includes a 24-bit CRC computed over the message body using the ICAO polynomial:

```
Generator: x²⁴ + x²³ + x²¹ + x²⁰ + x¹⁷ + x¹⁵ + x¹³ + x¹² + x¹⁰ + x⁸ + x⁵ + x⁴ + x³ + 1
Hex: 0xFFF409
```

For DF17 (ADS-B) messages, the last 24 bits are pure CRC — valid messages produce remainder 0x000000.

For DF11 (all-call) messages, the last 24 bits are XORed with the ICAO address — we recover the address by XORing the CRC remainder with the known polynomial result.

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

**Output:** `ModeFrame` dataclass with DF, ICAO address, raw message bytes, timestamp, signal level.

---

## Stage 4: Decoding (`decoder.py`, `cpr.py`)

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
- **100-ft mode**: M-bit = 0, Q-bit = 0 → Gillham gray code

---

## Stage 4a: CPR Decoding (`cpr.py`)

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

## Stage 5: Tracking (`tracker.py`, `icao.py`)

### ICAO Address Resolution (`icao.py`)

Every aircraft has a unique 24-bit ICAO address assigned by its country of registration:

- **Country lookup**: Address ranges are allocated by ICAO (e.g., 0xA00000-0xAFFFFF = USA)
- **Military blocks**: Some address ranges are reserved for military aircraft
- **N-number algorithm**: US civil aircraft addresses (0xA00001-0xADF7C7) can be converted back to the N-number registration using a base-conversion algorithm

### State Machine (`tracker.py`)

Each aircraft (by ICAO address) maintains a state object:
- Current position (lat, lon, altitude)
- Current velocity (ground speed, heading, vertical rate)
- Callsign and squawk
- CPR frame buffer (last even and odd frames for position decoding)
- Last update timestamp
- Signal history

The tracker:
1. Receives decoded messages from the decoder
2. Updates the appropriate aircraft state
3. Pairs even/odd CPR frames for position calculation
4. Writes updates to the database
5. Runs filters for anomaly detection

---

## Stage 6: Persistence (`database.py`)

SQLite database with WAL (Write-Ahead Logging) mode for concurrent read/write. Schema:

- **receivers**: Registered sensor nodes. Each receiver has a name, lat/lon, altitude, and description. Designed for distributed deployment — multiple receivers feeding a single database.
- **aircraft**: One row per unique ICAO address seen. Accumulates country, registration, military flag.
- **sightings**: One row per capture session per aircraft. Tracks callsign, squawk, min/max altitude, signal strength.
- **positions**: Time-series position data. Lat, lon, altitude, speed, heading, vertical rate. Tagged with `receiver_id` — which sensor heard this frame.
- **captures**: Metadata per capture session. Source file, start/end time, frame counts. Tagged with `receiver_id`.
- **events**: Anomalies detected by filters — emergency squawk, rapid descent, military aircraft, geofence breach.

### Multi-Receiver Architecture

The schema is receiver-aware from day one. Every position report and capture session records which receiver heard it. This enables:

- **Coverage mapping**: Which receivers see which aircraft? Where are the terrain shadows?
- **Signal comparison**: Same aircraft heard by multiple receivers at different signal strengths — crude triangulation.
- **MLAT readiness**: With 3+ receivers and precise timestamps, time-difference-of-arrival (TDOA) can independently verify or compute aircraft positions — including aircraft that broadcast Mode S but not ADS-B.
- **Reliability**: Receivers operate independently. One goes down, the network keeps collecting.

A single-receiver deployment works identically — it just has one row in the receivers table. Adding receivers is adding data sources, not refactoring the schema.

---

## Stage 7: Intelligence (`filters.py`, `enrichment.py`)

What makes this more than a radio scanner:

- **Military detection**: ICAO address in military allocation block, or callsign matches military patterns (e.g., RCH*, DOOM*, JAKE*)
- **Emergency squawks**: 7500 (hijack), 7600 (radio failure), 7700 (emergency)
- **Rapid descent**: Vertical rate exceeding -5,000 ft/min
- **Low altitude**: Aircraft below 500 ft AGL (excludes ground)
- **Circling/loitering**: Cumulative heading change >360° within 5 minutes, handles wraparound
- **Holding patterns**: Stable altitude (±500 ft) + reciprocal headings (180°±30°) detected via 10° heading bins
- **Proximity alerts**: Two aircraft within configurable distance (default 5nm horizontal, 1,000 ft vertical). Sorted pair key prevents duplicate alerts.
- **Unusual altitude**: Fast aircraft (>250 kts) below 3,000 ft with no airport within 15nm
- **Geofence alerts**: Aircraft entering a configured lat/lon/radius zone
- **Aircraft type enrichment**: Speed/altitude profile classification (jet, prop, turboprop, helicopter, military, cargo). Airline ICAO prefix → operator name lookup.
- **Airport awareness**: 3,642 US airports from OurAirports dataset. Nearest airport lookup, flight phase classification (approaching, departing, overflying).

---

## Stage 8: Display (`web/`, `cli.py`, `exporters.py`)

### Web Dashboard (`web/`)
- Flask app with Leaflet.js map (dark CARTO tiles)
- Aircraft icons with heading rotation, color-coded (green=civilian, red=military)
- Altitude-colored trail lines (green→yellow→red gradient, fading opacity for older segments)
- Click-to-detail popups with callsign, registration, country, altitude, speed, heading, vertical rate
- Stats overlay: aircraft count, positions, events, uptime
- Altitude legend (color bar with labeled scale 0–40,000 ft)
- Heatmap layer toggle (Leaflet.heat plugin for position density)
- Airport overlay: 3,642 US airports with Major/Medium/Small toggles, viewport-filtered rendering. Click for popup with elevation, coords, AirNav + SkyVector links.
- Dynamic map centering from receiver location via `/api/stats`
- Events dashboard with type filter buttons (military, emergency, anomaly)
- Query builder with preset filters (military, low altitude, fast, recent) and custom parameters
- Historical replay with time slider, play/pause, adjustable speed (1x–60x)
- Receiver management page with coverage circles
- Table view with sort/filter
- Single aircraft detail page with position history
- 1-second polling for real-time updates
- Dark theme (avionics tradition)

### Multi-Receiver Network (`feeder.py`, `web/ingest.py`)
- **Feeder agent**: Runs on remote Pi/machine with dongle. Captures frames via native demodulator (pyrtlsdr) or rtl_adsb fallback. Batches and POSTs hex frames to central server every N seconds.
- **Ingest API**: `POST /api/v1/frames` accepts batched frames with receiver metadata. Bearer token auth. Heartbeat endpoint for status. `GET /api/v1/receivers` lists all connected receivers with online/offline status.
- **Architecture**: Hub-and-spoke. Multiple feeders → one central server → one dashboard.

### CLI (`cli.py`)
- Rich-formatted tables in the terminal
- Real-time scrolling display with live proximity checks
- Stats summary (aircraft count, position count, military detections)

### Export (`exporters.py`)
- **CSV**: Flat position data for spreadsheet analysis
- **JSON**: Structured aircraft + position data
- **KML**: Google Earth flight paths with altitude
- **GeoJSON**: Map-ready feature collections

### Deployment (`deploy/`)
- Caddy (auto-HTTPS reverse proxy) + Gunicorn + Flask + SQLite
- systemd service with auto-restart
- Server provisioning script (Ubuntu: UFW, fail2ban, Python, unattended-upgrades)
- One-command deploy: `bash deploy/deploy.sh`

---

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/aircraft` | All tracked aircraft (optional `?military=true`) |
| GET | `/api/aircraft/<icao>` | Single aircraft detail + positions + events |
| GET | `/api/positions` | Most recent position per aircraft (optional `?minutes=` time filter) |
| GET | `/api/positions/all` | All positions ordered by time (for replay) |
| GET | `/api/trails` | Position trails per aircraft (`?minutes=` time window, default 60) |
| GET | `/api/lookup/<icao>` | External aircraft metadata via hexdb.io (manufacturer, type, owner) |
| GET | `/api/query` | Filtered positions (min/max alt, ICAO, military, limit) |
| GET | `/api/airports` | 3,642 US airports with type classification |
| GET | `/api/events` | Recent events (optional `?type=` filter) |
| GET | `/api/stats` | Database stats, receiver info, capture start time |
| POST | `/api/v1/frames` | Ingest frames from remote feeder (auth required) |
| POST | `/api/v1/heartbeat` | Feeder status heartbeat |
| GET | `/api/v1/receivers` | List connected receivers |

---

## File Map

| File | Lines | Purpose |
|------|-------|---------|
| `src/crc.py` | ~40 | CRC-24 polynomial validation |
| `src/capture.py` | ~360 | IQ file reader, frame reader, native demod + fallback live capture |
| `src/demodulator.py` | ~200 | IQ→magnitude, preamble detection, bit recovery |
| `src/frame_parser.py` | ~150 | Bit→ModeFrame, DF classification |
| `src/decoder.py` | ~400 | Frame→typed messages, all DF/TC types |
| `src/cpr.py` | ~180 | Compact Position Reporting math |
| `src/icao.py` | ~200 | Country lookup, military blocks, N-number |
| `src/tracker.py` | ~330 | Per-aircraft state machine, heading/position history |
| `src/database.py` | ~250 | SQLite schema, queries, WAL mode |
| `src/filters.py` | ~405 | Military, emergency, circling, holding, proximity, unusual alt, geofence |
| `src/enrichment.py` | ~310 | Aircraft type classification, operator lookup, 3,642 airports |
| `src/exporters.py` | ~150 | CSV, JSON, KML, GeoJSON output |
| `src/feeder.py` | ~190 | Remote receiver agent |
| `src/cli.py` | ~280 | Click CLI entry points |
| `src/web/app.py` | ~50 | Flask app factory |
| `src/web/ingest.py` | ~185 | Frame ingestion API for remote feeders |
| `src/web/routes.py` | ~400 | REST API + page routes (15 endpoints, 9 pages) |
| `data/airports.csv` | 3,643 | OurAirports US airport database |
