# Roadmap

A live snapshot of where adsb-decode is going. The [README](README.md) has the permanent overview; this doc tracks active priorities and updates as they shift.

*As of 2026-06-10 (v0.2.40 on main — Cesium 3D globe + perf overhaul shipped; phase-0 raw-sink/TLE-history + favicon on branch `brand/favicon-integration`, not yet merged).*

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
| Deployed version | v0.2.40 (2026-06-01, Cesium 3D cold-load perf — main branch tip, tagged) |
| AIS (vessel) ingester | Shipped to production 2026-04-28 — live alongside aircraft feed |

**Note on Cargo.toml workspace version:** The workspace `version` field in `rust/Cargo.toml` is stuck at `0.2.24` because `scripts/release.sh` was bypassed after v0.2.12 (tags applied directly without a Cargo bump). The binary running in production reports whatever version `Cargo.toml` shows at compile time. The git tag is the authoritative version reference.

---

## Branch state (as of 2026-06-10)

| Branch | Status | What it has that main doesn't |
|---|---|---|
| `main` | Current deploy base (v0.2.40, 2026-06-01) | — |
| `brand/favicon-integration` | Built, local only — **not merged, not pushed** | A2 raw_sink (phase-0 bleed-stop), A3 TLE-history archive, `/api/clientlog` error-logging endpoint, favicon |
| `raw-sink` | Built, local only — **not merged, not pushed** | A2 raw_sink, A3 TLE-history archive (subset of `brand/favicon-integration`) |
| `adsb-3d-glowup` | Superseded — all work merged to main through v0.2.40 | Nothing unique vs main |

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
| T3.1 | Production binary rebuild to v0.2.40 | v0.2.40 binary is current (cold-load perf + 3D overhaul). Next rebuild needed when branch-stranded work merges. |
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
| T1.5.6 | **WebSocket broadcast topology.** Single `tokio::sync::broadcast` channel for default 5-minute window. One server-side producer ticks every 2s, sends to all subscribers — N clients = 1 snapshot rebuild per tick. Non-default windows fall back to per-connection timer. Producer skips work when no receivers connected. | Shipped 2026-05-07; **disabled 2026-05-11 (v0.2.20)** after the prod producer stopped ticking and froze every dashboard tab on first paint. All clients now use per-connection timers; code retained behind `#[allow(dead_code)]` pending root-cause debug. |
| T1.5.7 | **Anomaly score on dashboard.** Map popups show an Anomaly line color-coded by tier (low/med/high); table rows get tinted backgrounds + "!" markers when anomalous. AircraftState carries `last_anomaly_score`; `/api/positions` exposes it on every row from the live tracker path. | Shipped 2026-05-07 |
| T1.5.8 | **`position_count_hourly` continuous aggregate.** TimescaleDB matview precomputes hourly position counts. `/api/stats` 24-hour count is `SUM(cnt)` over 24 rows instead of `COUNT(*)` over millions; real-time aggregates (`materialized_only = false`) cover the partial current hour. Addresses the Mar 14 connection-pool exhaustion incident class. Falls back to direct `COUNT(*)` when matview isn't installed. | Shipped 2026-05-07 |

### Tier 3.5 — UX / dashboard polish

Visible improvements that make the public site feel like a real product, not a demo.

| ID | Item | Status |
|---|---|---|
| T3.5.1 | **Weather radar overlay (B235).** RainViewer (free, key-less, public) tile layer on both Leaflet and Cesium. Toggle next to Vessels, 5-minute auto-refresh, persists in localStorage. | Shipped 2026-05-07 |
| T3.5.2 | **4D replay (B209).** Lazy-loaded CesiumJS on `/replay`. The same playback timeline (interpolation + speed multiplier + event markers) drives 3D entity positions; altitude in meters via `* 0.3048`. Default 2D path stays ~3MB lighter (no top-level Cesium script tag). | Shipped 2026-05-07 |
| T3.5.3 | **One-command receiver setup.** `deploy/receiver-setup.sh` rewritten as env-var-driven. `ADSB_API_KEY=xxx ADSB_NAME=my-pi curl ... \| sudo -E bash` populates env file + installs unit + auto-starts in one command. New `tests/setup/test_receiver_setup.sh` (31 bash smoke tests) wired into a `setup-scripts` CI job. | Shipped 2026-05-07 |
| T3.5.4 | **Cesium 3D globe overhaul (v0.2.25–v0.2.40).** Full 3D-first dashboard: Starlink satellite layer (Aircraft/Sats toggle), observation-dome circles around Splatlas pins, TLE disk persistence + `POST /api/v1/tle/:group` operator seed endpoint, CESIUM_ION_TOKEN env injection, real Ion World Terrain + banking aircraft models, atmosphere + altitude-trail glow, viewport satellite filter, Splatlas scene + receiver pins on the globe, cold-load perf 6.3s → 3s (prefetch engine, non-blocking terrain, progressive tiles). | Shipped to main — v0.2.25 (2026-05-14) through v0.2.40 (2026-06-01) |

### Tier 4 — Backlog

Tracked in `intelligence-hub/portfolio/implementation-backlog.md` (private):

- B234 — mobile ADS-B station mounted in a vehicle for ridgeline coverage
- B239 — ADS-B + acoustics correlation (acoustic signature per aircraft type)
- B240 — power infrastructure overlay (EIA Form 860, HIFLD transmission lines)
- B322 — 3D building dataset (2.75 B buildings) for terrain + occlusion modeling
- B235 → SHIPPED 2026-05-07 (see Tier 3.5)
- B209 → SHIPPED 2026-05-07 (see Tier 3.5)

---

## Branch-stranded work (built but not yet on main)

These items are complete on `brand/favicon-integration` but not merged or pushed. They are **not live in production** as of 2026-06-10.

| Item | Branch | Commit | What it does |
|---|---|---|---|
| **A2 raw append-only sink** | `brand/favicon-integration` | `8dd0e0f` | `raw_sink.rs` — pure writer for phase-0 bleed-stop; captures raw frames to an append-only local store before any parse/decode step. |
| **A3 dated TLE-history archive** | `brand/favicon-integration` | `6e7b49f` | `scripts/seed-tle.sh` update — history accrues per-date instead of overwriting last-good; epochs survive seed runs. |
| **Client error logging endpoint** | `brand/favicon-integration` | `688b7c1` | `POST /api/clientlog` — browser JS errors forwarded to server log for observability. |
| **Favicon** | `brand/favicon-integration` | `14bbb59` | Phosphor-green cursor-fin favicon matching the product wordmark. |

**Next action:** merge `brand/favicon-integration` → `main`, push, cut v0.2.41 (or bump Cargo.toml workspace version first to catch it up — see note in Current state table).

---

## Recent

### 2026-06-01 — Cesium 3D cold-load perf + final heading fixes (v0.2.38–v0.2.40)

- **Cold-load perf (v0.2.40).** Prefetch engine pre-warms TLE + Splatlas data before the globe renders; terrain loading made non-blocking (progressive tile load); cold-start cut from ~6.3s → ~3s.
- **Plane heading fixes (v0.2.38–v0.2.39).** Two separate heading-offset bugs: first the model was 180° backward (de-blob), then the corrected offset was 90° off; both fixed.
- **v0.2.36 regression fix (v0.2.37).** Page-load was zeroing out all aircraft in the Cesium entity store (TDZ reset).

### 2026-06-01 — Cesium 3D terrain + banking aircraft (v0.2.33–v0.2.35)

- **Real Ion World Terrain (v0.2.35).** Token-gated via `CESIUM_ION_TOKEN` env var (injected server-side at serve time in v0.2.33; no hardcoded token). Banking aircraft models proportional to turn rate. Airplane-model 404 fixed (v0.2.35). Smooth-glide satellite positions.
- **Atmosphere + glow (v0.2.34).** skyAtmosphere enabled; glowing altitude-trails; render-crash on 3D entry fixed (skyAtmosphere boolean + drop enableLighting).

### 2026-05-14 to 2026-05-31 — Cesium 3D globe buildout (v0.2.25–v0.2.33)

- **Receiver pins (v0.2.31–v0.2.32).** Distinct cyan antenna glyph for receiver sites (no coverage rings); icon name collision fix.
- **Splatlas integration (v0.2.26–v0.2.30).** Scene perimeter polygons, observation-dome circles, Splatlas pins clickable into the 3DGS viewer. Receiver site pin (links to splat scan). Splatlas + receiver pins on the 3D globe (v0.2.36 cap + filters).
- **TLE + satellite layer (v0.2.25–v0.2.29).** Starlink satellite layer with Aircraft/Sats toggle; concentric dome visualization; TLE disk persistence + `POST /api/v1/tle/:group` operator seed endpoint; seed-tle.sh bash fixes.
- **Brand (v0.2.25 vicinity).** adsb-decode product wordmark + OG card; Octo character imagery on how-it-works + features pages; about-page cross-link.

### 2026-05-11 — Statistical baseline recentered to observation-weighted median (v0.2.24)
- **Verification.** After v0.2.23 actually spawned the refresh task, the prod distribution finally showed the baseline contributing — but the predicted pattern landed: a routine Delta cruise flight scored 4.98 out of MAX 5.0, every off-centerline cell was reading as red high-tier anomaly. The math was right but the calibration was wrong.
- **Fix.** `BaselineCache::replace` now computes the **observation-weighted median** of `-ln(p)` across cells and stores it as an `offset`. `score()` subtracts the offset before clamping, so the typical cell scores 0. Only cells genuinely rarer than typical traffic contribute positive score; unseen cells still flag at MAX_SCORE so the long-tail signal survives.
- **Why observation-weighted, not cell-weighted.** Most grid cells are empty (water, forest, no traffic). Cell-weighted median would put "typical" in the empty bucket. Walking cells by score and stopping when the cumulative *observation* count crosses `total / 2` weights cells by their actual traffic share, which is what "typical" means in any user-relevant sense.
- **Tests.** `typical_traffic_cell_scores_zero_after_recentering` seeds three centerline cells + two off-centerline cells, asserts the densest centerline scores exactly 0. `off_typical_cell_scores_above_zero_below_max` pins the middle case. `unseen_cell_scores_at_max_after_recentering` confirms recentering doesn't suppress the long-tail signal.
- 175/175 default tests green, clippy --workspace clean.

### 2026-05-11 — Statistical baseline scorer wired into the production code path (v0.2.22)
- **Investigation.** After v0.2.21 made anomaly_score visible, the prod distribution looked wrong: 27 of 957 positions over a 30-min window scored non-zero, and every one was exactly `2.0` or `3.0`. A real statistical baseline produces continuous values; integer-only output points at rules-only output, with the baseline contributing nothing.
- **Root cause.** `spawn_baseline_refresh` lived in `main.rs` and was only called from the three `cmd_track_live*` subcommands. Production runs `adsb serve` — and `web::serve` never spawned the refresh. So `BaselineCache.total` stayed `0` and `score()` short-circuited to `0.0` for every position. T1.5.5 ("statistical anomaly baseline") and T1.5.7 ("anomaly on dashboard") had been marked shipped 2026-05-07 and were dormant the entire time.
- **Fix.** Moved `spawn_baseline_refresh` from `main.rs` to `web::mod.rs` and made it `pub`. `web::serve` now spawns it.
- **Observability.** `/api/stats` exposes `baseline_last_refresh`, `baseline_cell_count`, `baseline_total`. Same problem class won't be invisible for 4 days next time.
- **Tests.** `api_stats_exposes_baseline_state` + `refresh_baseline_once_populates_cache_when_db_has_positions`. 175/175 tests green; clippy clean.

### 2026-05-11 — Anomaly visibility + stale-feed banner + map JS extracted (v0.2.21)
- **Anomaly score was dark in prod (T1.5.2 / T1.5.5 / T1.5.7).** Every SELECT producer omitted `p.anomaly_score` from the column list. Fix adds the column to every Postgres + SQLite SELECT.
- **Stale-feed banner on the dashboard.** Banner appears above the map when `feed_age_seconds > 60`, with a link to `/receivers`.
- **`map.html` inline JS extracted to `templates/map.js`.** Template was 2,644 lines with a 2,317-line `<script>` block. New `GET /assets/map.js` handler serves the file with `application/javascript` MIME + 5-minute cache, embedded at compile time via `include_str!`.
- 173/173 default tests green; clippy clean.

### 2026-05-11 — Dashboard live-refresh fix + WS streaming regression tests (v0.2.20)
- **Bug.** Public dashboard "not refreshing." `wss://.../ws/positions?minutes=5` sent the initial snapshot then no further frames. Every tab opened with `minutes=5`, so every tab froze on first paint.
- **Fix.** `handle_ws_positions` now routes every client through `handle_ws_via_per_connection_timer`. Broadcast topology left wired but idle.
- **New tests.** Three async tests boot the full router on a random port, connect a real `tokio-tungstenite` client, assert ≥2 Text frames within 5.5s. 168/168 green; clippy clean.

### 2026-05-07 (evening) — Surfacing + perf: anomaly UI + stats CAGG (v0.2.18 + v0.2.19)
- **Anomaly score visible on map (v0.2.18).** AircraftState gains `last_anomaly_score`; tracker stores it when emitting PositionUpdate. Dashboard `anomalyClass()` helper maps to 3 tiers. Popup adds an Anomaly line color-coded by tier; table rows get tinted backgrounds + "!" marker.
- **`position_count_hourly` continuous aggregate (v0.2.19).** TimescaleDB matview precomputes hourly position counts; `/api/stats` 24-hour count from `SUM(cnt)` over 24 rows. Real-time aggregates cover the partial current hour.

### 2026-05-07 (mid-day Cesium fixes) — Two imageryProvider regressions caught (v0.2.13 + v0.2.14)
- Cesium 1.107+ removed the `imageryProvider` constructor option. Fix: build viewer with no initial imagery, then await `setCesiumMapStyle(style)`. Same bug in `/replay` 4D toggle caught and fixed before any user hit it.

### 2026-05-07 (BDS surfacing + statistical scorer + broadcast topology) — v0.2.15 + v0.2.16 + v0.2.17
- **BDS surfacing on detail page (v0.2.15).** Tracker absorbs each register's most-recent payload onto `AircraftState` per-register slots. `/api/aircraft/<icao>` adds a `comm_b` block when present.
- **Statistical anomaly baseline (v0.2.16).** New `adsb-server::baseline::BaselineCache` with 0.1° spatial grid. Score function is Laplace-smoothed log-probability, clamped [0, 5].
- **WS broadcast topology (v0.2.17).** Replaced N-clients × 1-snapshot-rebuild-per-tick with 1 rebuild fanned out via `tokio::sync::broadcast`.

### 2026-05-07 (afternoon) — Foundations: BDS / anomaly score / WebSocket
- **BDS decoding (T1.5.1).** `adsb-core::comm_b` decodes BDS 4,0 / 5,0 / 6,0. 12 new tests.
- **Anomaly score (T1.5.2).** Schema gets `anomaly_score REAL`; new `adsb-core::anomaly` module. 11 anomaly tests + 3 DB tests.
- **WebSocket position stream (T1.5.3).** New `/ws/positions` endpoint; polling becomes fallback-only.

### 2026-05-07 — UX session: setup script + weather + 4D replay
- **One-command receiver setup.** `ADSB_API_KEY=xxx curl ... | sudo -E bash` installs in one shell line. 31 bash smoke tests in CI.
- **Weather radar overlay (RainViewer).** Free, key-less, public. Works on both 2D and 3D.
- **4D replay (3D + time).** `/replay` gains a 3D toggle; altitude in meters via `* 0.3048`. 2D path stays ~3MB lighter.

### 2026-05-05 — v0.2.9 release
- **v0.2.9 cut + deployed to prod.** Tag `v0.2.9` on commit `a472847`. CI matrix passed; GitHub release published with `adsb-server-x86_64-unknown-linux-gnu-timescaledb.tar.gz`. Binary swapped on Lightsail VPS via `deploy.sh`.
- **Live-prod smoke test confirms enrichment landed.** Sample of 5000 positions: 99.5% callsign coverage, 93.1% registration, 98.7% country, 0.42% flagged military.

### 2026-05-05
- **Consumer contract regression tests, `PositionRow` enrichment fix, TimescaleDB invariant tests, Postgres integration tests, CLI dispatch tests, per-page deep tests, Pi crash-on-capture-exit fix, server-side register coord validation + `feed_age_seconds`.** See Tier 2 entries for detail.

### 2026-04-28
- Production VPS restored to 4 GB tier. Static IP swapped, services healthy.
- Correlator API contract mismatch found + fixed. Schema discipline doc shipped.
- Auto-recovery healthcheck shipped and deployed.
- **AIS ingester shipped to production.** Built on VPS, systemd unit + EnvironmentFile pattern, 4 ships/sec sustained ingest, hundreds of unique vessels in the first minutes. `/api/vessels`, `/api/vessel-positions`, and `/api/vessel-positions/latest` all returning live ship data.

### 2026-04-26
- AIS ingester development complete: parser fixes (two AISStream doc-vs-wire bugs), live dry-run successful with hundreds of real ships.
- TimescaleDB compression policy fix (events 30 d → 1 d compression-after) — Apr 14 disk-pressure fully resolved.

### 2026-04-25
- Three new SITL avoidance scenarios consumed via `/api/positions` (downstream UtilTech repo).

### 2026-04-14
- adsb-decode disk pressure incident: events table 29 GB → 544 KB; compression policy realigned. Triggered the 4 GB upgrade decision.

---

*Updated when priorities shift or work lands. Living doc.*
