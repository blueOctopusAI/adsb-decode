# Roadmap

A live snapshot of where adsb-decode is going. The [README](README.md) has the permanent overview; this doc tracks active priorities and updates as they shift.

*As of 2026-04-28.*

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
| T2.2 | **Snapshot cadence.** Lightsail "automatic snapshots" enabled with weekly retention. The Apr 25 → Apr 28 gap during the recent 4 GB restore was tolerable (data ages out in 14 days), but routine snapshots make future restores deterministic. | Pending console action |
| T2.3 | **Public-vs-gated dashboard decision.** The dashboard at `adsb.blueoctopustechnology.com` is fully public. For some downstream use cases, gating may matter; for others (live evaluator demos), public-by-default is the asset. Decide and document. | Pending decision |

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

### 2026-04-28
- Production VPS restored to 4 GB tier (was 1 GB, memory-pressured). Static IP swapped, services healthy, cert valid through Jun 2.
- Found correlator API contract mismatch via real round-trip; source + tests fixed; new `positions_in_window()` method added.
- Schema discipline doc shipped.
- Auto-recovery healthcheck unit shipped (deployed on the new VPS).
- **AIS ingester shipped to production.** Built on VPS, systemd unit + EnvironmentFile pattern, 4 ships/sec sustained ingest, hundreds of unique vessels in the first minutes. Maritime feed is now live alongside aircraft. `/api/vessels` returning real ship data (e.g. *FIRST DRAFT V*, *FOUNTAINHEAD*).
- **Bug found:** `/api/vessel_positions_latest` returns 200 with empty body even when `vessel_positions` has rows. `/api/vessels` works fine. Logged for follow-up — not blocking, since vessel + position data is queryable through other paths.

### 2026-04-26
- AIS ingester development complete: parser fixes (two AISStream doc-vs-wire bugs), live dry-run successful with hundreds of real ships.
- TimescaleDB compression policy fix (events 30 d → 1 d compression-after) — Apr 14 disk-pressure fully resolved.

### 2026-04-25
- Three new SITL avoidance scenarios consumed via `/api/positions` (downstream UtilTech repo).

### 2026-04-14
- adsb-decode disk pressure incident: events table 29 GB → 544 KB; compression policy realigned. Triggered the 4 GB upgrade decision.

---

*Updated when priorities shift or work lands. Living doc.*
