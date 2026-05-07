# Roadmap

A live snapshot of where adsb-decode is going. The [README](README.md) has the permanent overview; this doc tracks active priorities and updates as they shift.

*As of 2026-05-07 (evening — v0.2.19 deployed, full session 176 chain live).*

---

## Three roles

adsb-decode started as a from-scratch ADS-B decoder. It now plays three roles in parallel — and each shapes priorities differently.

**1. Live data plane.** Pi 5 ridgeline feeder → AWS Lightsail → public REST API at `adsb.blueoctopustechnology.com`. The dashboard is a public CesiumJS globe. Production runs on a 4 GB Lightsail instance with TimescaleDB (positions retain 14 days, events 7 days, vessels 14 days).

**2. Library source.** The `adsb-core` workspace crate is consumed as a git dependency by sister projects — notably the [`UtilitarianTechnology`](https://github.com/blueOctopusAI/UtilitarianTechnology) repo's `rust/adsb-poc/` and `rust/adsb-adapter/` crates, which use it for on-vehicle ADS-B integration in an aerospace context. Schema changes to types like `AircraftState` propagate downstream. Cross-repo schema discipline matters; see [`docs/schema-discipline.md`](docs/schema-discipline.md).

**3. Ground-truth source for visual identification.** A correlator script (in the UtilTech repo) queries the live REST API to validate visual-language-model identifications against ADS-B-confirmed callsigns and registrations. The metric — *agreement rate between visual identification and ADS-B ground truth* — is the strongest defensible claim available for the downstream identification system.

These three roles share one production deployment. A regression in role 1 (live API) breaks role 3 (correlation) and propagates a stale dependency to role 2 (library). Treat the production VPS accordingly.

---

## Current state

| Area | Status |
|---|---|
| Production VPS | 4 GB Lightsail (us-east-1), upgraded from 1 GB on 2026-04-28 (snapshot-restore + static IP swap) |
| Pi 5 feeder | Live, ridgeline mounted, pushing positions continuously |
| Database | TimescaleDB on Postgres; positions retain 14 d, events 7 d, vessels 14 d |
| Services | `adsb-decode.service` (Rust binary, port 8000), `caddy.service` (TLS termination), `postgresql` |
| Public API | `https://adsb.blueoctopustechnology.com/api/...` — see `rust/adsb-server/src/web/routes.rs` |
| Latest server binary | Mar 15 build (no behavioral changes pending; recent retention fixes are server-side SQL) |
| AIS (vessel) ingester | Mac-side complete + tested with live AISStream traffic; production deploy queued |

---

## Active work

Tiered by impact on downstream consumers (live API users, library consumers, correlation use case).

### Tier 1 — Maritime + correlation

These directly enable use cases not possible today.

| ID | Item | Status |
|---|---|---|
| T1.1 | **AIS ingester production deploy.** Ground-side maritime feed (AISStream WebSocket) writing into the same TimescaleDB. Adds ship positions to the existing aircraft positions in one query plane. Runbook: [`docs/ais-ingester-runbook.md`](docs/ais-ingester-runbook.md). | Shipped 2026-04-28 |
| T1.2 | **Correlator API contract fix.** The downstream correlator was written against an assumed `{"positions": [...]}` envelope, but `/api/positions` and `/api/query` return bare arrays. Tests mocked the wrong shape; real round-trip caught it 2026-04-28. | Fix shipped 2026-04-28 |
| T1.3 | **Time-window query for post-flight correlation.** `/api/positions/all?start=X&end=Y` already supports time bounds — that's the right endpoint for replay correlation, not the live `?minutes=N` lookback. Correlator client method added to use it. | Fix shipped 2026-04-28 |
| T1.4 | **Cross-repo schema discipline doc.** Documents how `adsb-core` types propagate to consumers and what changes require coordination. [`docs/schema-discipline.md`](docs/schema-discipline.md). | Shipped 2026-04-28 |

### Tier 2 — Resilience

Service quality for an API that downstream consumers (and live demos) depend on.

| ID | Item | Status |
|---|---|---|
| T2.1 | **Auto-recovery healthcheck.** `adsb-decode-healthcheck.timer` modeled on the BluePages pattern (60 s interval, curl `/api/stats`, restart on non-200). Until installed, manual recovery is needed if `adsb-decode.service` wedges. Source in [`deploy/adsb-decode-healthcheck.{sh,service,timer}`](deploy/). | Shipped 2026-04-28 |
| T2.2 | **Snapshot cadence.** Manual snapshot taken 2026-04-28 after the cutover (known-good baseline). Automatic-snapshots feature deferred — current traffic doesn't warrant the cadence overhead. Revisit if downstream consumers (correlator, evaluator demos) start depending on uptime SLOs. | Manual baseline 2026-04-28; auto deferred |
| T2.3 | **Public-vs-gated dashboard decision.** Deferred 2026-04-28. Current traffic is effectively zero, so gating doesn't matter at this stage. Revisit when (a) someone starts citing the dashboard in proposal materials, (b) traffic appears that we'd rather not have, or (c) ITAR conversation reaches a decision point. | Deferred — no current pressure |
| T2.4 | **Consumer contract regression tests.** A `consumer_contract_tests` module in `rust/adsb-server/src/web/routes.rs` pins the JSON shape (bare array vs envelope, key set, enrichment values populated) for every endpoint a UtilTech consumer hits. Module-level doc names each consumer file. Catches the 2026-04-28 envelope-vs-bare-array failure class statically. | Shipped 2026-05-05 |
| T2.5 | **TimescaleDB invariant tests.** `tests/timescale_invariants.rs` parses the SQL constants in `db_pg.rs` and asserts compression interval < retention interval for every hypertable. The 2026-04-14 disk-pressure incident (compression 30d / retention 7d) cannot recur silently. | Shipped 2026-05-05 |
| T2.6 | **Postgres integration tests.** Seven `#[ignore]`'d tests in `db_pg.rs::pg_integration` exercise the production backend's SQL paths that SQLite tests can't reach — DISTINCT ON enrichment, military filter, vessel position roundtrip. Opt-in via `DATABASE_URL` + `--features timescaledb -- --ignored`. | Shipped 2026-05-05 |
| T2.7 | **CLI dispatch tests.** `tests/cli_dispatch.rs` covers `adsb decode/stats/history/export` end-to-end via `env!("CARGO_BIN_EXE_adsb")`. Pins the user-facing arg surface (e.g. `--db-path`) that deploy scripts and cron jobs rely on. | Shipped 2026-05-05 |

### Tier 3 — Defer

Not blocking anything; revisit when one of the above concerns escalates.

| ID | Item | Reason to defer |
|---|---|---|
| T3.1 | Production binary rebuild | Mar 15 build is current; recent compression + retention fixes were SQL-level. No behavioral diff to ship. |
| T3.2 | On-drone AIS receiver | Current AIS path is ground-side via WebSocket; an actual flying AIS receiver is a hardware path, not a server-side change. |
| T3.3 | API CORS / API-key story | All consumers today are first-party. Revisit if third-party clients appear. |

### Tier 1.5 — Foundations for ML, real-time, and protocol depth

Engineering-shaped pieces that turn one-off polish into compounding capability.

| ID | Item | Status |
|---|---|---|
| T1.5.1 | **BDS 4,0 / 5,0 / 6,0 decoding for DF20/DF21 long Comm-B** (Enhanced Mode S). Recovers data ADS-B never carries: autopilot setting (selected altitude, FMS, baro), true airspeed + roll angle + ground speed (track and turn report), magnetic heading + IAS + Mach + barometric/inertial vertical rate (heading and speed report). Plausibility-based register identification — winning register must beat runner-up by ≥4 points or frame is dropped. New module `adsb-core::comm_b` (12 unit tests). | Shipped 2026-05-07 |
| T1.5.2 | **Per-position anomaly score + DB column.** Foundation for ML; today the scorer is hand-tuned thresholds (extreme speed, extreme vertical rate, position teleport, altitude jump, stuck position, nonmonotonic timestamps) but the persistence shape is what later ML work will replace, not the column. New module `adsb-core::anomaly`. Schema gets `anomaly_score REAL` on positions; idempotent migration in both SQLite and TimescaleDB backends. Tracker computes the score per position update and includes it on the TrackEvent. 11 anomaly unit tests + 3 DB migration/roundtrip tests. | Shipped 2026-05-07 |
| T1.5.3 | **WebSocket position stream (`/ws/positions`).** Replaces the per-tab 30 HTTP requests/min poll with one persistent socket that pushes the same JSON snapshot every 2 seconds. Polling becomes fallback-only — fires when WS isn't available (proxy strips upgrade, network blocks, etc.) or while reconnecting with backoff. Trail-duration slider triggers WS reconnect so live pushes match the user-selected window. | Shipped 2026-05-07 |
| T1.5.4 | **BDS Comm-B data on aircraft detail page.** Tracker absorbs decoded BDS register payloads onto `AircraftState` per-register slots. `/api/aircraft/<icao>` adds a `comm_b` block when the live tracker has data; detail page renders fields (Selected Alt MCP, True Airspeed, Magnetic Heading, Mach, Roll Angle) as info-grid items. | Shipped 2026-05-07 |
| T1.5.5 | **Statistical anomaly baseline scorer.** Spatial position-density grid (0.1° cells); hourly background refresh queries `position_density_grid()` over last 7 days; `score(lat, lon)` returns Laplace-smoothed log-probability clamped to [0, 5]. Combined additively with the rules-based scorer in the ingest path before persisting `anomaly_score`. | Shipped 2026-05-07 |
| T1.5.6 | **WebSocket broadcast topology.** Single `tokio::sync::broadcast` channel for default 5-minute window. One server-side producer ticks every 2s, sends to all subscribers — N clients = 1 snapshot rebuild per tick. Non-default windows fall back to per-connection timer. Producer skips work when no receivers connected. | Shipped 2026-05-07 |
| T1.5.7 | **Anomaly score on dashboard.** Map popups show an Anomaly line color-coded by tier (low/med/high); table rows get tinted backgrounds + "!" markers when anomalous. AircraftState carries `last_anomaly_score`; `/api/positions` exposes it on every row from the live tracker path. | Shipped 2026-05-07 |
| T1.5.8 | **`position_count_hourly` continuous aggregate.** TimescaleDB matview precomputes hourly position counts. `/api/stats` 24-hour count is `SUM(cnt)` over 24 rows instead of `COUNT(*)` over millions; real-time aggregates (`materialized_only = false`) cover the partial current hour. Addresses the Mar 14 connection-pool exhaustion incident class. Falls back to direct `COUNT(*)` when matview isn't installed. | Shipped 2026-05-07 |

### Tier 3.5 — UX / dashboard polish

Visible improvements that make the public site feel like a real product, not a demo.

| ID | Item | Status |
|---|---|---|
| T3.5.1 | **Weather radar overlay (B235).** RainViewer (free, key-less, public) tile layer on both Leaflet and Cesium. Toggle next to Vessels, 5-minute auto-refresh, persists in localStorage. | Shipped 2026-05-07 |
| T3.5.2 | **4D replay (B209).** Lazy-loaded CesiumJS on `/replay`. The same playback timeline (interpolation + speed multiplier + event markers) drives 3D entity positions; altitude in meters via `* 0.3048`. Default 2D path stays ~3MB lighter (no top-level Cesium script tag). | Shipped 2026-05-07 |
| T3.5.3 | **One-command receiver setup.** `deploy/receiver-setup.sh` rewritten as env-var-driven. `ADSB_API_KEY=xxx ADSB_NAME=my-pi curl ... \| sudo -E bash` populates env file + installs unit + auto-starts in one command. New `tests/setup/test_receiver_setup.sh` (31 bash smoke tests) wired into a `setup-scripts` CI job. | Shipped 2026-05-07 |

### Tier 4 — Backlog

Tracked in `intelligence-hub/portfolio/implementation-backlog.md` (private):

- B234 — mobile ADS-B station mounted in a vehicle for ridgeline coverage
- B239 — ADS-B + acoustics correlation (acoustic signature per aircraft type)
- B240 — power infrastructure overlay (EIA Form 860, HIFLD transmission lines)
- B322 — 3D building dataset (2.75 B buildings) for terrain + occlusion modeling
- B235 → SHIPPED 2026-05-07 (see Tier 3.5)
- B209 → SHIPPED 2026-05-07 (see Tier 3.5)

---

## Recent

### 2026-05-07 (evening) — Surfacing + perf: anomaly UI + stats CAGG (v0.2.18 + v0.2.19)
- **Anomaly score visible on map (v0.2.18).** AircraftState gains `last_anomaly_score`; tracker stores it when emitting PositionUpdate. `/api/positions` live-tracker path includes `anomaly_score` per row. Dashboard `anomalyClass()` helper maps to 3 tiers (low ≥ 0.5, med ≥ 2.0, high ≥ 4.0). Popup adds an Anomaly line color-coded by tier; table rows get tinted backgrounds + "!" marker.
- **`position_count_hourly` continuous aggregate (v0.2.19).** TimescaleDB matview precomputes hourly position counts; `/api/stats` 24-hour count goes from `COUNT(*)` over the full positions hypertable to `SUM(cnt)` over 24 precomputed rows. Real-time aggregates cover the partial current hour. Match-fallback to direct COUNT when matview isn't installed (plain Postgres / fresh DB pre-materialization). Verified live: matview registered alongside existing `positions_30s` and `positions_5m`.

### 2026-05-07 (mid-day Cesium fixes) — Two imageryProvider regressions caught (v0.2.13 + v0.2.14)
- Cesium 1.107+ removed the `imageryProvider` constructor option. Passing it to `new Viewer({...})` is silently ignored — globe renders black until something else attaches imagery via `imageryLayers.addImageryProvider()`. The existing `setCesiumMapStyle()` did it the right way, which is why "switch styles and back, then it works" was the user-hit workaround.
- Fix on map page (v0.2.13): build viewer with no initial imagery, then await `setCesiumMapStyle(style)`.
- Same bug in today's new `/replay` 4D toggle (v0.2.14) — caught and fixed before any user hit it.
- Drop the camera-framing tweak from v0.2.12; that was a misdiagnosis of the imagery bug.

### 2026-05-07 (BDS surfacing + statistical scorer + broadcast topology) — v0.2.15 + v0.2.16 + v0.2.17
- **BDS surfacing on detail page (v0.2.15).** Today's BDS decoder was extracting Comm-B data and dropping it on the floor; now the tracker absorbs each register's most-recent payload onto `AircraftState` per-register slots so a fresh BDS 5,0 doesn't clobber the last BDS 4,0. `/api/aircraft/<icao>` adds a `comm_b` block when present; detail page renders the fields. 12 + 3 new tests.
- **Statistical anomaly baseline (v0.2.16).** New `adsb-server::baseline::BaselineCache` holds a 0.1° spatial grid populated from a hourly `position_density_grid()` SQL aggregation over the last 7 days. Score function is Laplace-smoothed log-probability, clamped [0, 5]. Combined additively with the rules-based score on every position write. New SQL trait method on both backends; SQLite path uses a `CASE WHEN lat >= 0 ...` expression to get true floor semantics on negative longitudes (cast-to-int truncates toward zero, wrong bucket). 5 baseline tests + 1 SQL roundtrip test.
- **WS broadcast topology (v0.2.17).** Replaced N-clients × 1-snapshot-rebuild-per-tick with 1-snapshot-rebuild-per-tick fanned out via `tokio::sync::broadcast`. Default 5-minute window only; non-default windows fall back to per-connection timer. Producer skips work when no receivers connected (broadcast::send returns Err with no receivers).

### 2026-05-07 (afternoon) — Foundations: BDS / anomaly score / WebSocket
- **BDS decoding (T1.5.1).** DF20/DF21 long Comm-B replies were silently dropping their 56-bit MB field on every aircraft we heard. New `adsb-core::comm_b` module decodes BDS 4,0 (Selected Vertical Intention), BDS 5,0 (Track and Turn Report), BDS 6,0 (Heading and Speed Report). Identifies registers via plausibility scoring — each register's decoded values must pass physical sanity bounds and the winning register must beat the runner-up by ≥4 points. Ambiguous frames return None; conservative default. Register data now rides on the existing `AltitudeMsg` / `SquawkMsg` types as `comm_b: Option<CommBRegister>`, JSON-serialized only when populated. 12 new tests; net `adsb-core` 155 → 167 tests.
- **Anomaly score (T1.5.2).** Schema gets `anomaly_score REAL` on positions in both SQLite and TimescaleDB; new `migrate_add_column_if_missing` helper checks `pragma_table_info` and runs `ALTER TABLE ADD COLUMN` only when the column is missing. PG uses `IF NOT EXISTS`. Existing prod DBs upgrade transparently. Tracker computes the score on every position update via the new `adsb-core::anomaly` module — six rules at first light (extreme speed, extreme vertical rate, position teleport, altitude jump, stuck position, nonmonotonic timestamps). Each rule pushes a stable text flag so consumers can see *why* without re-deriving. PositionRow gains the field with skip-serializing-when-None. 11 anomaly tests + 3 DB tests (roundtrip via raw SQL, migration idempotent, migration adds column to legacy schema).
- **WebSocket position stream (T1.5.3).** New `/ws/positions` endpoint pushes the same JSON shape `/api/positions` returned. `collect_positions_snapshot` extracted from `api_positions` so both paths build the identical payload. The dashboard's 2-second polling becomes fallback-only — fires when WS isn't available, with exponential reconnect backoff capped at 30s. Trail-duration slider triggers WS reconnect so live pushes match the user's window. axum 0.7 `ws` feature enabled.
- **Tests:** 178 adsb-core + 144 adsb-server + 9 cli + 3 timescale tests pass; clippy clean both with and without the timescaledb feature.

### 2026-05-07 — UX session: setup script + weather + 4D replay
- **One-command receiver setup is one command for real.** Previously `curl | sudo bash` ran but still left the user editing /etc/adsb-receiver.env and running `systemctl enable --now`. Now `ADSB_API_KEY=xxx curl ... | sudo -E bash` finishes the install in one shell line. Added `--dry-run` (touches nothing, announces every action) and `--help` (reads usage out of the script header). New bash test suite (`tests/setup/test_receiver_setup.sh`) with 31 smoke tests covers arg parsing, env-var injection, file-path overrides, auto-start gating, root-check rejection. Wired into CI as a separate `setup-scripts` job.
- **Weather radar overlay (RainViewer).** Free, key-less, public radar tile service. Tile URL works as both a Leaflet `TileLayer` and a Cesium `UrlTemplateImageryProvider` — same overlay on both 2D and 3D. Toggle next to Vessels in map controls, 5-minute auto-refresh, persists in localStorage. Mode-switch handling: if user toggles 3D while weather is on, the layer follows.
- **4D replay (3D + time).** `/replay` page gains a 3D toggle that lazy-loads CesiumJS and renders the existing playback timeline in 3D. Same `interpolateAt` path drives Cesium entity positions; altitude in meters via `* 0.3048`. Per-icao point + polyline trail entities. Default 2D path stays ~3MB lighter (pinned by test — no top-level `<script src=cesium>` allowed).
- **Inline JS contract pins.** Two new `pages_tests` patterns added: balanced-`<script>`-tag count check (catches accidental tag breakage in 2,400-line embedded-JS templates) + per-page hook-name asserts (`enableWeather`, `disableWeather`, `loadCesiumJS`, `updateDisplay3D`, etc.). Substring-only checks were missing the parse-correctness gap; this pair gets closer to "the JS at least loads."
- **Stale catches:** `features.html` had a hardcoded "269 Tests" count — replaced with descriptive language. The "4D Replay" feature card had a 2D-only description (this was the V1 honest framing; now V1 ships 4D so the description got rewritten to match).

### 2026-05-05 — v0.2.9 release
- **v0.2.9 cut + deployed to prod.** Tag `v0.2.9` on commit `a472847` (post-`cargo fmt` on top of `e058341`). CI matrix passed on the second attempt — first attempt failed at fmt check on the new pg_integration / cli_dispatch / consumer_contract_tests blocks. GitHub release published with `adsb-server-x86_64-unknown-linux-gnu-timescaledb.tar.gz`. Binary swapped on Lightsail VPS via `deploy.sh` pattern; old binary backed up to `/opt/adsb-decode/adsb.v0.2.8.bak`. Service active.
- **Live-prod smoke test confirms enrichment landed.** Sample of 5000 positions from `/api/positions/all`: 99.5% callsign coverage, 93.1% registration, 98.7% country, 0.42% flagged military (e.g. PAT860 = US Army Priority Air Transport, ICAO ADFD7B). The historical-replay JOIN is populating real signal — Python correlator's `is_military`-based class discrimination now works on `/api/positions/all` queries.

### 2026-05-05
- **Consumer contract regression tests** landed (`web/routes.rs::consumer_contract_tests`). 10 tests pinning bare-array vs envelope shape and enrichment-populated invariants for every endpoint UtilTech consumers hit. Module doc names each consumer file so a future shape change names what to coordinate against.
- **`PositionRow` enrichment fix.** Added `callsign`, `registration`, `country`, `is_military` fields populated via JOIN against `aircraft` + latest `sightings`. The historical-replay correlator's military-discrimination logic now actually works — was silently defaulting to false. SQLite uses `ROW_NUMBER` window function, Postgres uses `DISTINCT ON`. Both backends covered.
- **TimescaleDB invariant tests** (`tests/timescale_invariants.rs`). Parses SQL constants and asserts compression < retention for every hypertable. Encodes the 2026-04-14 lesson as a regression test.
- **Postgres integration tests** for `db_pg.rs` (7 ignored tests, opt-in via `DATABASE_URL` + `--features timescaledb`). Covers schema migration, position roundtrip, enrichment JOINs, military filter, stats, vessel positions.
- **CLI dispatch tests** (`tests/cli_dispatch.rs`). 9 tests covering decode/stats/history/export/help/version + error paths.
- **Per-page deep tests** (`web/routes.rs::pages_tests`). 8 tests pinning robots.txt content-type, sitemap completeness, llms.txt API endpoint references, register form fields, og-image mime, /aircraft detail external links, /api/airports bare-array shape.
- **Pi `adsb-receiver` crash-on-capture-exit.** A USB transient on May 4 11:19 EDT killed the `rtl_adsb` subprocess silently for 27 h before user noticed empty map. Now `exit(1)` on subprocess death → systemd `Restart=always` respawns within 10 s. Verified by SIGKILL test.
- **Server-side: register coord validation + `feed_age_seconds` on `/api/stats`.** External monitors can detect "API up but feeder dead" without operator intervention.
- **Doc cleanup.** CLAUDE.md DB-backend section now reflects Postgres+TimescaleDB production / SQLite local; added `adsb-receiver` crate to project structure (was claiming 3 crates, actually 4); dropped stale hardcoded test counts. db_pg.rs module doc updated with current retention numbers.
- Total test count: 317 → 347 default + 7 opt-in Postgres tests.

### 2026-04-28
- Production VPS restored to 4 GB tier (was 1 GB, memory-pressured). Static IP swapped, services healthy, cert valid through Jun 2.
- Found correlator API contract mismatch via real round-trip; source + tests fixed; new `positions_in_window()` method added.
- Schema discipline doc shipped.
- Auto-recovery healthcheck unit shipped (deployed on the new VPS).
- **AIS ingester shipped to production.** Built on VPS, systemd unit + EnvironmentFile pattern, 4 ships/sec sustained ingest, hundreds of unique vessels in the first minutes. Maritime feed is now live alongside aircraft. `/api/vessels` returning real ship data (e.g. *FIRST DRAFT V*, *FOUNTAINHEAD*); `/api/vessel-positions` and `/api/vessel-positions/latest` (DISTINCT-ON-mmsi for one position per ship) both healthy.
- **Doc correction:** runbook references to `/api/vessel_positions_latest` (underscore form) corrected to actual route paths `/api/vessel-positions/latest` (hyphen form). Code was always right; only the docs were stale.

### 2026-04-26
- AIS ingester development complete: parser fixes (two AISStream doc-vs-wire bugs), live dry-run successful with hundreds of real ships.
- TimescaleDB compression policy fix (events 30 d → 1 d compression-after) — Apr 14 disk-pressure fully resolved.

### 2026-04-25
- Three new SITL avoidance scenarios consumed via `/api/positions` (downstream UtilTech repo).

### 2026-04-14
- adsb-decode disk pressure incident: events table 29 GB → 544 KB; compression policy realigned. Triggered the 4 GB upgrade decision.

---

*Updated when priorities shift or work lands. Living doc.*
