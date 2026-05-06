# Roadmap

A live snapshot of where adsb-decode is going. The [README](README.md) has the permanent overview; this doc tracks active priorities and updates as they shift.

*As of 2026-05-05.*

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

### Tier 4 — Backlog

Tracked in `intelligence-hub/portfolio/implementation-backlog.md` (private):

- B209 — 4D replay mode on a CesiumJS timeline
- B234 — mobile ADS-B station mounted in a vehicle for ridgeline coverage
- B239 — ADS-B + acoustics correlation (acoustic signature per aircraft type)
- B240 — power infrastructure overlay (EIA Form 860, HIFLD transmission lines)
- B322 — 3D building dataset (2.75 B buildings) for terrain + occlusion modeling

---

## Recent

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
