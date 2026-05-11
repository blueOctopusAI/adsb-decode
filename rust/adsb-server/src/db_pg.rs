//! TimescaleDB (PostgreSQL) backend — production-scale time-series storage.
//!
//! Requires the `timescaledb` feature flag and a PostgreSQL server with
//! the TimescaleDB extension installed.
//!
//! Key differences from SQLite:
//! - `positions`, `events`, and `vessel_positions` are TimescaleDB hypertables
//! - Compression policies (positions/vessel_positions: 7d, events: 1d)
//! - Retention policies (positions/vessel_positions: 14d, events: 7d)
//! - Continuous aggregates provide downsampled views (30s, 5m)
//! - Connection pooling via sqlx::PgPool
//!
//! Invariant: for every hypertable, compression interval MUST be strictly less
//! than retention interval, or chunks are dropped before they can compress.
//! This was the 2026-04-14 events incident (29 GB hypertable). Enforced by
//! `tests/timescale_invariants.rs`.

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

use crate::db::*;

/// TimescaleDB schema — creates tables, hypertables, and policies.
const TIMESCALE_SCHEMA: &str = r#"
-- Core tables
CREATE TABLE IF NOT EXISTS receivers (
    id BIGSERIAL PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    email TEXT,
    lat DOUBLE PRECISION,
    lon DOUBLE PRECISION,
    altitude_ft DOUBLE PRECISION,
    description TEXT,
    api_key TEXT UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_receivers_api_key ON receivers(api_key);

CREATE TABLE IF NOT EXISTS aircraft (
    icao TEXT PRIMARY KEY,
    registration TEXT,
    country TEXT,
    is_military BOOLEAN DEFAULT FALSE,
    first_seen TIMESTAMPTZ NOT NULL,
    last_seen TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS sightings (
    id BIGSERIAL PRIMARY KEY,
    icao TEXT NOT NULL REFERENCES aircraft(icao),
    capture_id BIGINT,
    callsign TEXT,
    squawk TEXT,
    min_altitude_ft INTEGER,
    max_altitude_ft INTEGER,
    avg_signal DOUBLE PRECISION,
    message_count INTEGER DEFAULT 0,
    first_seen TIMESTAMPTZ NOT NULL,
    last_seen TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS positions (
    time TIMESTAMPTZ NOT NULL,
    icao TEXT NOT NULL,
    receiver_id BIGINT,
    lat DOUBLE PRECISION NOT NULL,
    lon DOUBLE PRECISION NOT NULL,
    altitude_ft INTEGER,
    speed_kts DOUBLE PRECISION,
    heading_deg DOUBLE PRECISION,
    vertical_rate_fpm INTEGER,
    anomaly_score DOUBLE PRECISION
);

CREATE TABLE IF NOT EXISTS captures (
    id BIGSERIAL PRIMARY KEY,
    receiver_id BIGINT,
    source TEXT,
    start_time TIMESTAMPTZ,
    end_time TIMESTAMPTZ,
    total_frames BIGINT DEFAULT 0,
    valid_frames BIGINT DEFAULT 0,
    aircraft_count BIGINT DEFAULT 0
);

CREATE TABLE IF NOT EXISTS events (
    time TIMESTAMPTZ NOT NULL,
    icao TEXT NOT NULL,
    event_type TEXT NOT NULL,
    description TEXT,
    lat DOUBLE PRECISION,
    lon DOUBLE PRECISION,
    altitude_ft INTEGER
);

-- Vessel (AIS) tables
CREATE TABLE IF NOT EXISTS vessels (
    mmsi TEXT PRIMARY KEY,
    name TEXT,
    vessel_type TEXT,
    flag TEXT,
    first_seen TIMESTAMPTZ NOT NULL,
    last_seen TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS vessel_positions (
    time TIMESTAMPTZ NOT NULL,
    mmsi TEXT NOT NULL,
    lat DOUBLE PRECISION NOT NULL,
    lon DOUBLE PRECISION NOT NULL,
    speed_kts DOUBLE PRECISION,
    course_deg DOUBLE PRECISION,
    heading_deg DOUBLE PRECISION
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_positions_icao ON positions(icao, time DESC);
CREATE INDEX IF NOT EXISTS idx_sightings_icao ON sightings(icao);
CREATE INDEX IF NOT EXISTS idx_sightings_icao_capture ON sightings(icao, capture_id);
CREATE INDEX IF NOT EXISTS idx_events_icao ON events(icao, time DESC);
CREATE INDEX IF NOT EXISTS idx_events_type ON events(event_type);
CREATE INDEX IF NOT EXISTS idx_aircraft_last_seen ON aircraft(last_seen DESC);
CREATE INDEX IF NOT EXISTS idx_vessel_positions_mmsi ON vessel_positions(mmsi, time DESC);
"#;

/// TimescaleDB-specific setup (hypertables, compression, retention).
///
/// These are idempotent — safe to run on every startup.
const TIMESCALE_SETUP: &str = r#"
-- Convert to hypertables (no-op if already hypertables)
SELECT create_hypertable('positions', 'time', if_not_exists => TRUE);
SELECT create_hypertable('events', 'time', if_not_exists => TRUE);
SELECT create_hypertable('vessel_positions', 'time', if_not_exists => TRUE);

-- Compression policy: compress chunks older than 7 days
-- segmentby icao for efficient per-aircraft queries
ALTER TABLE positions SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'icao',
    timescaledb.compress_orderby = 'time DESC'
);
SELECT add_compression_policy('positions', INTERVAL '7 days', if_not_exists => TRUE);

-- Retention policy: drop raw positions older than 14 days
-- (38GB Lightsail disk fills up at ~90 days of continuous feeding)
SELECT add_retention_policy('positions', INTERVAL '14 days', if_not_exists => TRUE);

-- Events: compress after 3 days, retain for 7 days
-- (event detection runs every 10s and generates high volume — 365 days filled 38GB disk twice)
ALTER TABLE events SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'icao',
    timescaledb.compress_orderby = 'time DESC'
);
-- NOTE: compression must fire well before retention, or chunks are dropped
-- before compression runs, and the table holds only uncompressed data.
-- Discovered 2026-04-14 when a 30-day compression / 7-day retention pair
-- grew the events hypertable to 29 GB of uncompressed data.
SELECT add_compression_policy('events', INTERVAL '1 day', if_not_exists => TRUE);
SELECT add_retention_policy('events', INTERVAL '7 days', if_not_exists => TRUE);

-- Vessel positions: compress after 7 days, retain for 14 days.
-- Same compression-before-retention ordering as positions/events (lesson from
-- the 2026-04-14 events incident — compress interval must be strictly less
-- than retention interval, or retention drops chunks before compression runs).
-- Steady-state at ~4 ships/sec ingest: ~250 MB on disk (uncompressed 0-7d
-- window + compressed 7-14d window), well under the 38 GB Lightsail budget.
ALTER TABLE vessel_positions SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'mmsi',
    timescaledb.compress_orderby = 'time DESC'
);
SELECT add_compression_policy('vessel_positions', INTERVAL '7 days', if_not_exists => TRUE);
SELECT add_retention_policy('vessel_positions', INTERVAL '14 days', if_not_exists => TRUE);
"#;

/// Continuous aggregate for 30-second downsampled positions.
const CAGG_30S: &str = r#"
CREATE MATERIALIZED VIEW IF NOT EXISTS positions_30s
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('30 seconds', time) AS bucket,
    icao,
    AVG(lat) AS lat,
    AVG(lon) AS lon,
    AVG(altitude_ft)::INTEGER AS altitude_ft,
    AVG(speed_kts) AS speed_kts,
    AVG(heading_deg) AS heading_deg,
    AVG(vertical_rate_fpm)::INTEGER AS vertical_rate_fpm,
    COUNT(*) AS sample_count
FROM positions
GROUP BY bucket, icao
WITH NO DATA;

SELECT add_continuous_aggregate_policy('positions_30s',
    start_offset => INTERVAL '1 hour',
    end_offset => INTERVAL '30 seconds',
    schedule_interval => INTERVAL '1 minute',
    if_not_exists => TRUE
);
"#;

/// Continuous aggregate for hourly position counts. Used by `/api/stats` to
/// get the "positions in last 24h" number without scanning every chunk in
/// the hypertable. The MAR 14 outage class twice — `COUNT(*) FROM positions
/// WHERE time > NOW() - 24h` was the slowest query on the system.
///
/// `materialized_only = false` enables real-time aggregates so the most
/// recent hour (which the policy hasn't materialized yet) still counts via
/// a union between the matview and the base hypertable.
const CAGG_POSITION_COUNT_HOURLY: &str = r#"
CREATE MATERIALIZED VIEW IF NOT EXISTS position_count_hourly
WITH (timescaledb.continuous, timescaledb.materialized_only = false) AS
SELECT
    time_bucket('1 hour', time) AS hour,
    COUNT(*) AS cnt
FROM positions
GROUP BY hour
WITH NO DATA;

SELECT add_continuous_aggregate_policy('position_count_hourly',
    start_offset => INTERVAL '7 days',
    end_offset => INTERVAL '5 minutes',
    schedule_interval => INTERVAL '15 minutes',
    if_not_exists => TRUE
);
"#;

/// Continuous aggregate for 5-minute downsampled positions (historical).
const CAGG_5M: &str = r#"
CREATE MATERIALIZED VIEW IF NOT EXISTS positions_5m
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('5 minutes', time) AS bucket,
    icao,
    AVG(lat) AS lat,
    AVG(lon) AS lon,
    AVG(altitude_ft)::INTEGER AS altitude_ft,
    AVG(speed_kts) AS speed_kts,
    AVG(heading_deg) AS heading_deg,
    AVG(vertical_rate_fpm)::INTEGER AS vertical_rate_fpm,
    COUNT(*) AS sample_count
FROM positions
GROUP BY bucket, icao
WITH NO DATA;

SELECT add_continuous_aggregate_policy('positions_5m',
    start_offset => INTERVAL '2 hours',
    end_offset => INTERVAL '5 minutes',
    schedule_interval => INTERVAL '5 minutes',
    if_not_exists => TRUE
);
"#;

/// PostgreSQL/TimescaleDB backend with connection pooling.
pub struct TimescaleDb {
    pool: PgPool,
}

impl TimescaleDb {
    /// Connect to PostgreSQL and run migrations.
    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(url)
            .await?;

        // Create base schema
        sqlx::raw_sql(TIMESCALE_SCHEMA).execute(&pool).await?;

        // Idempotent migrations for columns added after the initial schema.
        // PG's IF NOT EXISTS makes these safe on repeat runs.
        sqlx::raw_sql(
            "ALTER TABLE positions ADD COLUMN IF NOT EXISTS anomaly_score DOUBLE PRECISION;",
        )
        .execute(&pool)
        .await?;

        // Set up TimescaleDB hypertables and policies (may fail if extension not loaded)
        if let Err(e) = sqlx::raw_sql(TIMESCALE_SETUP).execute(&pool).await {
            eprintln!("Warning: TimescaleDB setup failed (extension may not be installed): {e}");
            eprintln!("Falling back to plain PostgreSQL (no compression/retention/aggregates)");
        }

        // Set up continuous aggregates (may fail without TimescaleDB)
        let _ = sqlx::raw_sql(CAGG_30S).execute(&pool).await;
        let _ = sqlx::raw_sql(CAGG_5M).execute(&pool).await;
        let _ = sqlx::raw_sql(CAGG_POSITION_COUNT_HOURLY)
            .execute(&pool)
            .await;

        Ok(TimescaleDb { pool })
    }
}

/// Helper: convert epoch seconds to PostgreSQL TIMESTAMPTZ.
fn epoch_to_pg(ts: f64) -> chrono::DateTime<chrono::Utc> {
    use chrono::{TimeZone, Utc};
    Utc.timestamp_opt(ts as i64, ((ts.fract()) * 1_000_000_000.0) as u32)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).unwrap())
}

/// Helper: extract a row column as f64.
///
/// `EXTRACT(EPOCH FROM ...)` in PostgreSQL 14+ returns `numeric`, not
/// `double precision`.  sqlx 0.8 cannot decode `numeric` as `f64`,
/// so all EXTRACT calls in this file cast to `::double precision`.
fn row_f64(r: &sqlx::postgres::PgRow, col: &str) -> f64 {
    r.try_get::<f64, _>(col).unwrap_or(0.0)
}

#[async_trait::async_trait]
impl AdsbDatabase for TimescaleDb {
    async fn stats(&self) -> DbStats {
        let aircraft: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM aircraft")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);
        // 24-hour position count via the hourly continuous aggregate. Each
        // bucket is precomputed; SUM-of-24-rows beats COUNT-of-millions.
        // Real-time aggregates (materialized_only = false on the CAGG) cover
        // the most-recent partial hour by unioning with the base hypertable.
        // Falls back to direct COUNT when the CAGG isn't installed (e.g.
        // plain Postgres without TimescaleDB).
        let positions: i64 = match sqlx::query_scalar::<_, Option<i64>>(
            "SELECT COALESCE(SUM(cnt), 0)::BIGINT
             FROM position_count_hourly
             WHERE hour > NOW() - INTERVAL '24 hours'",
        )
        .fetch_one(&self.pool)
        .await
        {
            Ok(Some(n)) => n,
            _ => sqlx::query_scalar(
                "SELECT COUNT(*) FROM positions WHERE time > NOW() - INTERVAL '24 hours'",
            )
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0),
        };
        let events: i64 = sqlx::query_scalar("SELECT COALESCE(approximate_row_count('events'), 0)")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);
        let receivers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM receivers")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);
        let captures: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM captures")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);

        // MAX(time) returns NULL on empty positions; the EXTRACT therefore yields NULL
        // and we want that surfaced as None instead of swallowed as 0.
        let feed_age_seconds: Option<f64> = sqlx::query_scalar::<_, Option<f64>>(
            "SELECT EXTRACT(EPOCH FROM (NOW() - MAX(time)))::double precision FROM positions",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(None)
        .map(|s: f64| s.max(0.0));

        DbStats {
            aircraft,
            positions,
            events,
            receivers,
            captures,
            feed_age_seconds,
        }
    }

    async fn get_all_aircraft(&self) -> Vec<AircraftRow> {
        let rows = sqlx::query(
            "SELECT icao, registration, country, is_military,
                    EXTRACT(EPOCH FROM first_seen)::double precision as first_seen,
                    EXTRACT(EPOCH FROM last_seen)::double precision as last_seen
             FROM aircraft ORDER BY last_seen DESC",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.iter()
            .map(|r| AircraftRow {
                icao: r.get("icao"),
                registration: r.get("registration"),
                country: r.get("country"),
                is_military: r.get::<bool, _>("is_military"),
                first_seen: row_f64(r, "first_seen"),
                last_seen: row_f64(r, "last_seen"),
            })
            .collect()
    }

    async fn get_aircraft(&self, icao: &str) -> Option<AircraftRow> {
        sqlx::query(
            "SELECT icao, registration, country, is_military,
                    EXTRACT(EPOCH FROM first_seen)::double precision as first_seen,
                    EXTRACT(EPOCH FROM last_seen)::double precision as last_seen
             FROM aircraft WHERE icao = $1",
        )
        .bind(icao)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .map(|r| AircraftRow {
            icao: r.get("icao"),
            registration: r.get("registration"),
            country: r.get("country"),
            is_military: r.get::<bool, _>("is_military"),
            first_seen: row_f64(&r, "first_seen"),
            last_seen: row_f64(&r, "last_seen"),
        })
    }

    async fn get_positions(&self, icao: &str, limit: i64) -> Vec<PositionRow> {
        let rows = sqlx::query(
            "SELECT icao, lat, lon, altitude_ft, speed_kts, heading_deg,
                    vertical_rate_fpm, anomaly_score,
                    EXTRACT(EPOCH FROM time)::double precision as timestamp
             FROM positions WHERE icao = $1 ORDER BY time DESC LIMIT $2",
        )
        .bind(icao)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.iter().map(row_to_position).collect()
    }

    async fn get_recent_positions(&self, minutes: f64, limit: i64) -> Vec<PositionRow> {
        let interval = format!("{} minutes", minutes as i64);
        // DISTINCT ON deduplicates across multiple receivers — one row per aircraft
        let rows = sqlx::query(
            "SELECT DISTINCT ON (icao) icao, lat, lon, altitude_ft, speed_kts,
                    heading_deg, vertical_rate_fpm, anomaly_score,
                    EXTRACT(EPOCH FROM time)::double precision as timestamp
             FROM positions
             WHERE time >= NOW() - $1::INTERVAL
             ORDER BY icao, time DESC
             LIMIT $2",
        )
        .bind(&interval)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.iter().map(row_to_position).collect()
    }

    async fn get_events(
        &self,
        event_type: Option<&str>,
        icao: Option<&str>,
        limit: i64,
    ) -> Vec<EventRow> {
        let rows =
            match (event_type, icao) {
                (Some(et), Some(ic)) => sqlx::query(
                    "SELECT 0::BIGINT as id, icao, event_type, description, lat, lon, altitude_ft,
                            EXTRACT(EPOCH FROM time)::double precision as timestamp
                     FROM events WHERE event_type = $1 AND icao = $2
                     ORDER BY time DESC LIMIT $3",
                )
                .bind(et)
                .bind(ic)
                .bind(limit)
                .fetch_all(&self.pool)
                .await,
                (Some(et), None) => sqlx::query(
                    "SELECT 0::BIGINT as id, icao, event_type, description, lat, lon, altitude_ft,
                            EXTRACT(EPOCH FROM time)::double precision as timestamp
                     FROM events WHERE event_type = $1
                     ORDER BY time DESC LIMIT $2",
                )
                .bind(et)
                .bind(limit)
                .fetch_all(&self.pool)
                .await,
                (None, Some(ic)) => sqlx::query(
                    "SELECT 0::BIGINT as id, icao, event_type, description, lat, lon, altitude_ft,
                            EXTRACT(EPOCH FROM time)::double precision as timestamp
                     FROM events WHERE icao = $1
                     ORDER BY time DESC LIMIT $2",
                )
                .bind(ic)
                .bind(limit)
                .fetch_all(&self.pool)
                .await,
                (None, None) => sqlx::query(
                    "SELECT 0::BIGINT as id, icao, event_type, description, lat, lon, altitude_ft,
                            EXTRACT(EPOCH FROM time)::double precision as timestamp
                     FROM events ORDER BY time DESC LIMIT $1",
                )
                .bind(limit)
                .fetch_all(&self.pool)
                .await,
            };

        rows.unwrap_or_default()
            .iter()
            .map(|r| EventRow {
                id: r.get("id"),
                icao: r.get("icao"),
                event_type: r.get("event_type"),
                description: r
                    .get::<Option<String>, _>("description")
                    .unwrap_or_default(),
                lat: r.get("lat"),
                lon: r.get("lon"),
                altitude_ft: r.get("altitude_ft"),
                timestamp: row_f64(r, "timestamp"),
            })
            .collect()
    }

    async fn get_trails(&self, minutes: f64, limit_per_aircraft: i64) -> Vec<PositionRow> {
        let interval = format!("{} minutes", minutes as i64);

        // Use continuous aggregates for longer time windows to avoid full table scans.
        // <2h: raw positions (full resolution)
        // 2-6h: positions_30s (30-second buckets)
        // >6h: positions_5m (5-minute buckets)
        let source_table = if minutes > 360.0 {
            "positions_5m"
        } else if minutes > 120.0 {
            "positions_30s"
        } else {
            "positions"
        };

        let time_col = if source_table == "positions" {
            "time"
        } else {
            "bucket"
        };

        let sql = if source_table == "positions" {
            // Raw positions: deduplicate across receivers
            "WITH deduped AS (
                     SELECT DISTINCT ON (icao, time) icao, lat, lon, altitude_ft,
                            speed_kts, heading_deg, vertical_rate_fpm, time
                     FROM positions
                     WHERE time >= NOW() - $1::INTERVAL
                     ORDER BY icao, time, receiver_id
                 )
                 SELECT icao, lat, lon, altitude_ft, speed_kts, heading_deg,
                        vertical_rate_fpm, EXTRACT(EPOCH FROM time)::double precision as timestamp
                 FROM (
                     SELECT *, ROW_NUMBER() OVER (PARTITION BY icao ORDER BY time DESC) as rn
                     FROM deduped
                 ) sub WHERE rn <= $2
                 ORDER BY icao, time ASC"
                .to_string()
        } else {
            // Continuous aggregates: already deduplicated and bucketed
            format!(
                "SELECT icao, lat, lon, altitude_ft, speed_kts, heading_deg,
                        vertical_rate_fpm, EXTRACT(EPOCH FROM {time_col})::double precision as timestamp
                 FROM (
                     SELECT *, ROW_NUMBER() OVER (PARTITION BY icao ORDER BY {time_col} DESC) as rn
                     FROM {source_table}
                     WHERE {time_col} >= NOW() - $1::INTERVAL
                 ) sub WHERE rn <= $2
                 ORDER BY icao, {time_col} ASC"
            )
        };

        let rows = sqlx::query(&sql)
            .bind(&interval)
            .bind(limit_per_aircraft)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default();

        rows.iter().map(row_to_position).collect()
    }

    async fn get_heatmap_positions(
        &self,
        minutes: f64,
        limit: i64,
    ) -> Vec<(f64, f64, Option<i32>)> {
        let interval = format!("{} minutes", minutes as i64);
        let rows = sqlx::query(
            "SELECT lat, lon, altitude_ft FROM positions
             WHERE time >= NOW() - $1::INTERVAL
             ORDER BY RANDOM() LIMIT $2",
        )
        .bind(&interval)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.iter()
            .map(|r| {
                (
                    r.get::<f64, _>("lat"),
                    r.get::<f64, _>("lon"),
                    r.get::<Option<i32>, _>("altitude_ft"),
                )
            })
            .collect()
    }

    async fn get_heatmap_density(&self, minutes: f64, resolution: f64) -> Vec<HeatmapCell> {
        let interval = format!("{} minutes", minutes as i64);
        // Use 5-minute aggregate for heatmap (doesn't need raw resolution)
        // and cap at 5000 cells to prevent massive responses
        let source = if minutes > 120.0 {
            "positions_5m"
        } else {
            "positions"
        };
        let time_col = if source == "positions" {
            "time"
        } else {
            "bucket"
        };
        let sql = format!(
            "SELECT
                 ROUND(lat / $1) * $1 AS cell_lat,
                 ROUND(lon / $1) * $1 AS cell_lon,
                 COUNT(*) AS cnt,
                 AVG(altitude_ft)::INTEGER AS avg_alt
             FROM {source}
             WHERE {time_col} >= NOW() - $2::INTERVAL
             GROUP BY cell_lat, cell_lon
             ORDER BY cnt DESC
             LIMIT 5000"
        );
        let rows = sqlx::query(&sql)
            .bind(resolution)
            .bind(&interval)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default();

        rows.iter()
            .map(|r| HeatmapCell {
                lat: r.get::<f64, _>("cell_lat"),
                lon: r.get::<f64, _>("cell_lon"),
                count: r.get::<i64, _>("cnt"),
                avg_alt: r.get("avg_alt"),
            })
            .collect()
    }

    async fn query_positions(
        &self,
        min_alt: Option<i32>,
        max_alt: Option<i32>,
        icao: Option<&str>,
        military: bool,
        limit: i64,
    ) -> Vec<PositionRow> {
        let mut conditions = vec!["TRUE".to_string()];
        let mut idx = 1;

        if min_alt.is_some() {
            conditions.push(format!("p.altitude_ft >= ${idx}"));
            idx += 1;
        }
        if max_alt.is_some() {
            conditions.push(format!("p.altitude_ft <= ${idx}"));
            idx += 1;
        }
        if icao.is_some() {
            conditions.push(format!("p.icao = ${idx}"));
            idx += 1;
        }
        if military {
            conditions.push("a.is_military = TRUE".to_string());
        }

        let where_clause = conditions.join(" AND ");
        // Aircraft + latest-sighting enrichment for /api/query consumers.
        // DISTINCT ON is the Postgres idiom for "one row per ICAO, latest by last_seen".
        let sql = format!(
            "SELECT p.icao, p.lat, p.lon, p.altitude_ft, p.speed_kts, p.heading_deg,
                    p.vertical_rate_fpm, p.anomaly_score,
                    EXTRACT(EPOCH FROM p.time)::double precision as timestamp,
                    s_latest.callsign, a.registration, a.country, a.is_military
             FROM positions p
             LEFT JOIN aircraft a ON p.icao = a.icao
             LEFT JOIN (
                 SELECT DISTINCT ON (icao) icao, callsign
                 FROM sightings
                 ORDER BY icao, last_seen DESC
             ) s_latest ON p.icao = s_latest.icao
             WHERE {where_clause}
             ORDER BY p.time DESC LIMIT ${idx}"
        );

        let mut query = sqlx::query(&sql);
        if let Some(v) = min_alt {
            query = query.bind(v);
        }
        if let Some(v) = max_alt {
            query = query.bind(v);
        }
        if let Some(v) = icao {
            query = query.bind(v);
        }
        query = query.bind(limit);

        query
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default()
            .iter()
            .map(row_to_position)
            .collect()
    }

    async fn get_all_positions_ordered(
        &self,
        limit: i64,
        start: Option<f64>,
        end: Option<f64>,
    ) -> Vec<PositionRow> {
        let start_ts = start.map(epoch_to_pg);
        let end_ts = end.map(epoch_to_pg);

        // Aircraft + latest-sighting enrichment for /api/positions/all — the
        // post-flight replay correlation endpoint UtilTech's correlator hits.
        let rows = sqlx::query(
            "SELECT p.icao, p.lat, p.lon, p.altitude_ft, p.speed_kts, p.heading_deg,
                    p.vertical_rate_fpm, p.anomaly_score,
                    EXTRACT(EPOCH FROM p.time)::double precision as timestamp,
                    s_latest.callsign, a.registration, a.country, a.is_military
             FROM positions p
             LEFT JOIN aircraft a ON p.icao = a.icao
             LEFT JOIN (
                 SELECT DISTINCT ON (icao) icao, callsign
                 FROM sightings
                 ORDER BY icao, last_seen DESC
             ) s_latest ON p.icao = s_latest.icao
             WHERE ($2::TIMESTAMPTZ IS NULL OR p.time >= $2)
               AND ($3::TIMESTAMPTZ IS NULL OR p.time <= $3)
             ORDER BY p.time ASC LIMIT $1",
        )
        .bind(limit)
        .bind(start_ts)
        .bind(end_ts)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.iter().map(row_to_position).collect()
    }

    async fn get_receivers(&self) -> Vec<ReceiverRow> {
        let rows = sqlx::query(
            "SELECT id, name, email, lat, lon, description,
                    EXTRACT(EPOCH FROM created_at)::double precision as created_at
             FROM receivers ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.iter()
            .map(|r| ReceiverRow {
                id: r.get("id"),
                name: r.get("name"),
                email: r.get("email"),
                lat: r.get("lat"),
                lon: r.get("lon"),
                description: r.get("description"),
                created_at: row_f64(r, "created_at"),
            })
            .collect()
    }

    async fn get_aircraft_history(&self, hours: f64) -> Vec<HistoryRow> {
        let interval = format!("{} hours", hours as i64);
        let rows = sqlx::query(
            "SELECT a.icao, s.callsign, a.country, a.is_military,
                    s.min_altitude_ft, s.max_altitude_ft,
                    COALESCE(s.message_count, 0)::BIGINT as message_count,
                    EXTRACT(EPOCH FROM a.first_seen)::double precision as first_seen,
                    EXTRACT(EPOCH FROM a.last_seen)::double precision as last_seen
             FROM aircraft a
             LEFT JOIN sightings s ON a.icao = s.icao
             WHERE a.last_seen >= NOW() - $1::INTERVAL
             ORDER BY a.last_seen DESC",
        )
        .bind(&interval)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.iter()
            .map(|r| HistoryRow {
                icao: r.get("icao"),
                callsign: r.get("callsign"),
                country: r.get("country"),
                is_military: r.get::<bool, _>("is_military"),
                min_altitude_ft: r.get("min_altitude_ft"),
                max_altitude_ft: r.get("max_altitude_ft"),
                message_count: r.get("message_count"),
                first_seen: row_f64(r, "first_seen"),
                last_seen: row_f64(r, "last_seen"),
            })
            .collect()
    }

    async fn export_positions(
        &self,
        hours: Option<f64>,
        icao: Option<&str>,
        limit: i64,
    ) -> Vec<PositionRow> {
        let mut conditions = Vec::new();
        let mut idx = 1;

        if hours.is_some() {
            conditions.push(format!("p.time >= NOW() - ${}::INTERVAL", idx));
            idx += 1;
        }
        if icao.is_some() {
            conditions.push(format!("p.icao = ${}", idx));
            idx += 1;
        }

        let where_clause = if conditions.is_empty() {
            "TRUE".to_string()
        } else {
            conditions.join(" AND ")
        };

        // CSV/JSON export with full enrichment so external consumers don't need
        // a second round-trip per aircraft.
        let sql = format!(
            "SELECT p.icao, p.lat, p.lon, p.altitude_ft, p.speed_kts, p.heading_deg,
                    p.vertical_rate_fpm, p.anomaly_score,
                    EXTRACT(EPOCH FROM p.time)::double precision as timestamp,
                    s_latest.callsign, a.registration, a.country, a.is_military
             FROM positions p
             LEFT JOIN aircraft a ON p.icao = a.icao
             LEFT JOIN (
                 SELECT DISTINCT ON (icao) icao, callsign
                 FROM sightings
                 ORDER BY icao, last_seen DESC
             ) s_latest ON p.icao = s_latest.icao
             WHERE {where_clause}
             ORDER BY p.time ASC LIMIT ${idx}"
        );

        let mut query = sqlx::query(&sql);
        if let Some(h) = hours {
            let interval = format!("{} hours", h as i64);
            query = query.bind(interval);
        }
        if let Some(ic) = icao {
            query = query.bind(ic);
        }
        query = query.bind(limit);

        query
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default()
            .iter()
            .map(row_to_position)
            .collect()
    }

    // -----------------------------------------------------------------------
    // Write methods
    // -----------------------------------------------------------------------

    async fn upsert_aircraft(
        &self,
        icao: &str,
        country: Option<&str>,
        registration: Option<&str>,
        is_military: bool,
        timestamp: f64,
    ) {
        let ts = epoch_to_pg(timestamp);
        let _ = sqlx::query(
            "INSERT INTO aircraft (icao, country, registration, is_military, first_seen, last_seen)
             VALUES ($1, $2, $3, $4, $5, $5)
             ON CONFLICT (icao) DO UPDATE SET
                 country = COALESCE(EXCLUDED.country, aircraft.country),
                 registration = COALESCE(EXCLUDED.registration, aircraft.registration),
                 is_military = aircraft.is_military OR EXCLUDED.is_military,
                 last_seen = GREATEST(aircraft.last_seen, EXCLUDED.last_seen)",
        )
        .bind(icao)
        .bind(country)
        .bind(registration)
        .bind(is_military)
        .bind(ts)
        .execute(&self.pool)
        .await;
    }

    async fn upsert_sighting(
        &self,
        icao: &str,
        capture_id: Option<i64>,
        callsign: Option<&str>,
        squawk: Option<&str>,
        altitude_ft: Option<i32>,
        timestamp: f64,
    ) {
        let ts = epoch_to_pg(timestamp);
        // Use a single upsert: INSERT ON CONFLICT by (icao, capture_id).
        // Since sightings has no unique constraint on (icao, capture_id), we
        // try to update existing or insert new in two steps.
        let existing: Option<(i64, Option<i32>, Option<i32>)> = sqlx::query(
            "SELECT id, min_altitude_ft, max_altitude_ft FROM sightings
             WHERE icao = $1 AND capture_id IS NOT DISTINCT FROM $2
             ORDER BY last_seen DESC LIMIT 1",
        )
        .bind(icao)
        .bind(capture_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .map(|r| {
            (
                r.get("id"),
                r.get("min_altitude_ft"),
                r.get("max_altitude_ft"),
            )
        });

        if let Some((id, min_alt, max_alt)) = existing {
            let new_min = match (min_alt, altitude_ft) {
                (Some(m), Some(a)) => Some(m.min(a)),
                (None, Some(a)) => Some(a),
                (Some(m), None) => Some(m),
                (None, None) => None,
            };
            let new_max = match (max_alt, altitude_ft) {
                (Some(m), Some(a)) => Some(m.max(a)),
                (None, Some(a)) => Some(a),
                (Some(m), None) => Some(m),
                (None, None) => None,
            };
            let _ = sqlx::query(
                "UPDATE sightings SET
                     callsign = COALESCE($1, callsign),
                     squawk = COALESCE($2, squawk),
                     min_altitude_ft = $3,
                     max_altitude_ft = $4,
                     message_count = message_count + 1,
                     last_seen = $5
                 WHERE id = $6",
            )
            .bind(callsign)
            .bind(squawk)
            .bind(new_min)
            .bind(new_max)
            .bind(ts)
            .bind(id)
            .execute(&self.pool)
            .await;
        } else {
            let _ = sqlx::query(
                "INSERT INTO sightings
                 (icao, capture_id, callsign, squawk, min_altitude_ft, max_altitude_ft,
                  message_count, first_seen, last_seen)
                 VALUES ($1, $2, $3, $4, $5, $5, 1, $6, $6)",
            )
            .bind(icao)
            .bind(capture_id)
            .bind(callsign)
            .bind(squawk)
            .bind(altitude_ft)
            .bind(ts)
            .execute(&self.pool)
            .await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn add_position(
        &self,
        icao: &str,
        lat: f64,
        lon: f64,
        altitude_ft: Option<i32>,
        speed_kts: Option<f64>,
        heading_deg: Option<f64>,
        vertical_rate_fpm: Option<i32>,
        anomaly_score: Option<f64>,
        receiver_id: Option<i64>,
        timestamp: f64,
    ) {
        let ts = epoch_to_pg(timestamp);
        let _ = sqlx::query(
            "INSERT INTO positions (time, icao, receiver_id, lat, lon, altitude_ft,
                                    speed_kts, heading_deg, vertical_rate_fpm, anomaly_score)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(ts)
        .bind(icao)
        .bind(receiver_id)
        .bind(lat)
        .bind(lon)
        .bind(altitude_ft)
        .bind(speed_kts)
        .bind(heading_deg)
        .bind(vertical_rate_fpm)
        .bind(anomaly_score)
        .execute(&self.pool)
        .await;
    }

    async fn add_event(
        &self,
        icao: &str,
        event_type: &str,
        description: &str,
        lat: Option<f64>,
        lon: Option<f64>,
        altitude_ft: Option<i32>,
        timestamp: f64,
    ) {
        let ts = epoch_to_pg(timestamp);
        let _ = sqlx::query(
            "INSERT INTO events (time, icao, event_type, description, lat, lon, altitude_ft)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(ts)
        .bind(icao)
        .bind(event_type)
        .bind(description)
        .bind(lat)
        .bind(lon)
        .bind(altitude_ft)
        .execute(&self.pool)
        .await;
    }

    /// Spatial density baseline. Postgres has a real `floor()` so we use it
    /// directly. Cutoff is computed as a TIMESTAMPTZ from `NOW() - INTERVAL`
    /// rather than an epoch param to keep the query plan simple.
    async fn position_density_grid(
        &self,
        hours_back: f64,
        grid_size_deg: f64,
    ) -> Vec<(i32, i32, u64)> {
        let rows = sqlx::query(
            "SELECT
                FLOOR(lat / $1)::INT AS lat_b,
                FLOOR(lon / $1)::INT AS lon_b,
                COUNT(*) AS cnt
             FROM positions
             WHERE time >= NOW() - ($2 * INTERVAL '1 hour')
             GROUP BY lat_b, lon_b",
        )
        .bind(grid_size_deg)
        .bind(hours_back)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .map(|r| {
                use sqlx::Row;
                let lat_b: i32 = r.get("lat_b");
                let lon_b: i32 = r.get("lon_b");
                let cnt: i64 = r.get("cnt");
                (lat_b, lon_b, cnt as u64)
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Vessel (AIS) methods
    // -----------------------------------------------------------------------

    async fn get_vessels(&self, limit: i64) -> Vec<VesselRow> {
        let rows = sqlx::query(
            "SELECT mmsi, name, vessel_type, flag,
                    EXTRACT(EPOCH FROM first_seen)::double precision as first_seen,
                    EXTRACT(EPOCH FROM last_seen)::double precision as last_seen
             FROM vessels ORDER BY last_seen DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.iter()
            .map(|r| VesselRow {
                mmsi: r.get("mmsi"),
                name: r.get("name"),
                vessel_type: r.get("vessel_type"),
                flag: r.get("flag"),
                first_seen: row_f64(r, "first_seen"),
                last_seen: row_f64(r, "last_seen"),
            })
            .collect()
    }

    async fn get_vessel_positions(&self, minutes: f64, limit: i64) -> Vec<VesselPositionRow> {
        let interval = format!("{} minutes", minutes as i64);
        let rows = sqlx::query(
            "SELECT mmsi, lat, lon, speed_kts, course_deg, heading_deg,
                    EXTRACT(EPOCH FROM time)::double precision as timestamp
             FROM vessel_positions
             WHERE time >= NOW() - $1::INTERVAL
             ORDER BY time DESC LIMIT $2",
        )
        .bind(&interval)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.iter().map(row_to_vessel_position).collect()
    }

    async fn get_recent_vessel_positions(&self, limit: i64) -> Vec<VesselPositionRow> {
        let rows = sqlx::query(
            "SELECT DISTINCT ON (mmsi) mmsi, lat, lon, speed_kts, course_deg, heading_deg,
                    EXTRACT(EPOCH FROM time)::double precision as timestamp
             FROM vessel_positions
             ORDER BY mmsi, time DESC
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.iter().map(row_to_vessel_position).collect()
    }

    // -----------------------------------------------------------------------
    // Registration
    // -----------------------------------------------------------------------

    async fn register_receiver(
        &self,
        name: &str,
        email: Option<&str>,
        lat: Option<f64>,
        lon: Option<f64>,
        description: Option<&str>,
    ) -> Option<(i64, String)> {
        let api_key = uuid::Uuid::new_v4().to_string();
        let row = sqlx::query(
            "INSERT INTO receivers (name, email, lat, lon, description, api_key)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id",
        )
        .bind(name)
        .bind(email)
        .bind(lat)
        .bind(lon)
        .bind(description)
        .bind(&api_key)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        row.map(|r| (r.get::<i64, _>("id"), api_key))
    }

    async fn lookup_receiver_by_api_key(&self, key: &str) -> Option<(i64, String)> {
        sqlx::query("SELECT id, name FROM receivers WHERE api_key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .map(|r| (r.get::<i64, _>("id"), r.get::<String, _>("name")))
    }
}

// ---------------------------------------------------------------------------
// Row mapping helpers
// ---------------------------------------------------------------------------

fn row_to_position(r: &sqlx::postgres::PgRow) -> PositionRow {
    // Enrichment fields are optional — try_get returns Err on missing columns,
    // which is fine for the unenriched producers (`get_recent_positions`,
    // `get_trails`, `get_positions`) that don't SELECT them.
    PositionRow {
        icao: r.get("icao"),
        lat: r.get("lat"),
        lon: r.get("lon"),
        altitude_ft: r.get("altitude_ft"),
        speed_kts: r.get("speed_kts"),
        heading_deg: r.get("heading_deg"),
        vertical_rate_fpm: r.get("vertical_rate_fpm"),
        timestamp: row_f64(r, "timestamp"),
        callsign: r.try_get("callsign").ok().flatten(),
        registration: r.try_get("registration").ok().flatten(),
        country: r.try_get("country").ok().flatten(),
        is_military: r.try_get("is_military").ok(),
        anomaly_score: r.try_get("anomaly_score").ok().flatten(),
    }
}

fn row_to_vessel_position(r: &sqlx::postgres::PgRow) -> VesselPositionRow {
    VesselPositionRow {
        mmsi: r.get("mmsi"),
        lat: r.get("lat"),
        lon: r.get("lon"),
        speed_kts: r.get("speed_kts"),
        course_deg: r.get("course_deg"),
        heading_deg: r.get("heading_deg"),
        timestamp: row_f64(r, "timestamp"),
    }
}

// ----- Vessel writers (Daniel-requested maritime feed) ----------------------
// Inherent (non-trait) methods — the AdsbDatabase trait is ADS-B-only.
// Used by the ais-ingester binary that consumes AISStream.io WebSocket feed.
// SqliteDb has equivalent sync writers in db.rs; these are the async
// counterparts for production (TimescaleDB/PostgreSQL).
//
// dead_code is allowed because each binary in this crate gets its own
// compilation: the `adsb` binary doesn't use vessel writers (only the
// `ais-ingester` binary does). too_many_arguments matches the existing
// add_position pattern.
#[allow(dead_code, clippy::too_many_arguments)]
impl TimescaleDb {
    /// Insert a `vessel_positions` row for one AIS PositionReport.
    pub async fn add_vessel_position(
        &self,
        mmsi: &str,
        lat: f64,
        lon: f64,
        speed_kts: Option<f64>,
        course_deg: Option<f64>,
        heading_deg: Option<f64>,
        timestamp: f64,
    ) -> Result<(), sqlx::Error> {
        let ts = epoch_to_pg(timestamp);
        sqlx::query(
            "INSERT INTO vessel_positions
                 (time, mmsi, lat, lon, speed_kts, course_deg, heading_deg)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(ts)
        .bind(mmsi)
        .bind(lat)
        .bind(lon)
        .bind(speed_kts)
        .bind(course_deg)
        .bind(heading_deg)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Upsert a `vessels` row from one AIS ShipStaticData (or position-only
    /// MMSI heard for the first time). Idempotent — preserves existing
    /// metadata when the new row has `None` for a field.
    pub async fn upsert_vessel(
        &self,
        mmsi: &str,
        name: Option<&str>,
        vessel_type: Option<&str>,
        flag: Option<&str>,
        timestamp: f64,
    ) -> Result<(), sqlx::Error> {
        let ts = epoch_to_pg(timestamp);
        sqlx::query(
            "INSERT INTO vessels (mmsi, name, vessel_type, flag, first_seen, last_seen)
             VALUES ($1, $2, $3, $4, $5, $5)
             ON CONFLICT (mmsi) DO UPDATE SET
                 name = COALESCE(EXCLUDED.name, vessels.name),
                 vessel_type = COALESCE(EXCLUDED.vessel_type, vessels.vessel_type),
                 flag = COALESCE(EXCLUDED.flag, vessels.flag),
                 last_seen = EXCLUDED.last_seen",
        )
        .bind(mmsi)
        .bind(name)
        .bind(vessel_type)
        .bind(flag)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Postgres + TimescaleDB integration tests
//
// db_pg.rs is the production backend. Without these tests, the only signal we
// had on Postgres-specific SQL correctness was "the live VPS hasn't broken
// yet" — which already failed us once on the 2026-04-28 envelope-vs-bare-array
// contract bug and again on the historical-replay enrichment gap discovered
// 2026-05-05.
//
// These tests are #[ignore]'d because they need a live Postgres + TimescaleDB
// instance, and per `feedback_no-docker-on-mac.md` we don't pull testcontainers
// into the local Mac dev loop. Opt-in:
//
//   export DATABASE_URL="postgres://adsb:CHANGEME@localhost:5432/adsb_test"
//   cargo test -p adsb-server --features timescaledb -- --ignored
//
// Each test uses a unique ICAO in the Algeria block (DDA000-DDAFFF) and a
// far-future timestamp (year 2099) so parallel runs and stale data can't
// interfere with each other.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod pg_integration {
    use super::*;
    use crate::db::AdsbDatabase;
    use std::sync::Arc;

    /// 2099-01-01 UTC — well outside any real ADS-B traffic, so retention won't
    /// drop it and time-window queries from concurrent tests can't see ours.
    const FAR_FUTURE_BASE: f64 = 4_070_908_800.0;

    async fn connect_or_skip() -> Option<Arc<TimescaleDb>> {
        let url = match std::env::var("DATABASE_URL") {
            Ok(u) => u,
            Err(_) => {
                eprintln!("DATABASE_URL not set, skipping Postgres integration test");
                return None;
            }
        };
        match TimescaleDb::connect(&url).await {
            Ok(db) => Some(Arc::new(db)),
            Err(e) => {
                eprintln!("Could not connect to Postgres at DATABASE_URL: {e}. Skipping.");
                None
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at TimescaleDB"]
    async fn pg_connect_runs_schema_migrations() {
        // If the SQL constants regress (invalid hypertable, broken policy SQL),
        // this fires before any other test gets a chance to run.
        let _db = connect_or_skip().await.expect("DATABASE_URL not set");
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at TimescaleDB"]
    async fn pg_position_roundtrip() {
        // Catches column rename / serde drift between PositionRow and the SELECT clause.
        let Some(db) = connect_or_skip().await else {
            return;
        };

        let ts = FAR_FUTURE_BASE + 100.0;
        db.upsert_aircraft("DDA001", Some("US"), Some("N99001"), false, ts)
            .await;
        db.add_position(
            "DDA001",
            35.18,
            -83.33,
            Some(20000),
            Some(300.0),
            Some(90.0),
            None,
            None,
            ts,
        )
        .await;

        let positions = db.get_positions("DDA001", 10).await;
        assert!(
            !positions.is_empty(),
            "round-trip insert→query found nothing"
        );
        let p = &positions[0];
        assert_eq!(p.icao, "DDA001");
        assert!((p.lat - 35.18).abs() < 1e-6);
        assert!((p.lon - (-83.33)).abs() < 1e-6);
        assert_eq!(p.altitude_ft, Some(20000));
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at TimescaleDB"]
    async fn pg_get_all_positions_ordered_enrichment_populates() {
        // The 2026-05-05 enrichment fix on the Postgres path uses DISTINCT ON,
        // which has no SQLite equivalent. SQLite-side tests can't catch a
        // typo in this Postgres-specific syntax. This test does.
        let Some(db) = connect_or_skip().await else {
            return;
        };

        let ts = FAR_FUTURE_BASE + 200.0;
        db.upsert_aircraft("DDA002", Some("US"), Some("N99002"), true, ts)
            .await;
        db.upsert_sighting("DDA002", None, Some("PG_TEST"), None, Some(35000), ts)
            .await;
        db.add_position(
            "DDA002",
            36.0,
            -84.0,
            Some(35000),
            Some(450.0),
            Some(180.0),
            None,
            None,
            ts,
        )
        .await;

        let positions = db
            .get_all_positions_ordered(10000, Some(ts - 1.0), Some(ts + 1.0))
            .await;

        let row = positions
            .iter()
            .find(|p| p.icao == "DDA002")
            .expect("seeded position not in /api/positions/all result — JOIN broken");

        assert_eq!(
            row.is_military,
            Some(true),
            "Postgres get_all_positions_ordered did not surface is_military. \
             Check the LEFT JOIN aircraft a ON p.icao = a.icao SQL."
        );
        assert_eq!(row.registration.as_deref(), Some("N99002"));
        assert_eq!(
            row.callsign.as_deref(),
            Some("PG_TEST"),
            "Postgres DISTINCT ON sightings JOIN dropped the callsign"
        );
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at TimescaleDB"]
    async fn pg_query_positions_enrichment_populates() {
        let Some(db) = connect_or_skip().await else {
            return;
        };

        let ts = FAR_FUTURE_BASE + 300.0;
        db.upsert_aircraft("DDA003", Some("UK"), Some("G99003"), false, ts)
            .await;
        db.upsert_sighting("DDA003", None, Some("BAW123"), None, Some(28000), ts)
            .await;
        db.add_position(
            "DDA003",
            37.0,
            -85.0,
            Some(28000),
            Some(420.0),
            Some(45.0),
            None,
            None,
            ts,
        )
        .await;

        let positions = db
            .query_positions(Some(20000), Some(40000), Some("DDA003"), false, 100)
            .await;

        let row = positions
            .iter()
            .find(|p| p.icao == "DDA003")
            .expect("seeded position not in /api/query result");

        assert_eq!(row.registration.as_deref(), Some("G99003"));
        assert_eq!(row.country.as_deref(), Some("UK"));
        assert_eq!(row.callsign.as_deref(), Some("BAW123"));
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at TimescaleDB"]
    async fn pg_query_positions_military_filter_works() {
        // sqlx parameter index drift on the Postgres side has bitten us before.
        // Pin the military-filter behavior.
        let Some(db) = connect_or_skip().await else {
            return;
        };

        let ts = FAR_FUTURE_BASE + 400.0;
        db.upsert_aircraft("DDA004", Some("US"), Some("N99004"), false, ts)
            .await;
        db.upsert_aircraft("DDA005", Some("US"), Some("N99005"), true, ts)
            .await;
        db.add_position(
            "DDA004",
            38.0,
            -86.0,
            Some(30000),
            Some(400.0),
            Some(0.0),
            None,
            None,
            ts,
        )
        .await;
        db.add_position(
            "DDA005",
            38.1,
            -86.1,
            Some(30000),
            Some(400.0),
            Some(0.0),
            None,
            None,
            ts,
        )
        .await;

        let mil_only = db.query_positions(None, None, None, true, 1000).await;
        let saw_mil = mil_only.iter().any(|p| p.icao == "DDA005");
        let saw_civ = mil_only.iter().any(|p| p.icao == "DDA004");
        assert!(saw_mil, "military filter dropped a known military aircraft");
        assert!(!saw_civ, "military filter let a civilian aircraft through");
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at TimescaleDB"]
    async fn pg_stats_exposes_feed_age_seconds() {
        // /api/stats healthcheck contract on the Postgres path. The
        // feed_age_seconds field is what catches "API up but feeder dead" —
        // the failure that took 27 h to surface on 2026-05-04.
        let Some(db) = connect_or_skip().await else {
            return;
        };

        let ts = FAR_FUTURE_BASE + 500.0;
        db.upsert_aircraft("DDA006", Some("US"), Some("N99006"), false, ts)
            .await;
        db.add_position(
            "DDA006",
            39.0,
            -87.0,
            Some(10000),
            Some(150.0),
            Some(0.0),
            None,
            None,
            ts,
        )
        .await;

        let stats = db.stats().await;
        assert!(stats.positions >= 1, "seeded position not visible in stats");
        assert!(
            stats.feed_age_seconds.is_some(),
            "stats.feed_age_seconds must be populated when positions exist"
        );
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at TimescaleDB"]
    async fn pg_vessel_position_roundtrip() {
        // AIS path roundtrip. vessel_positions is the newest hypertable (Apr 28)
        // and least-tested.
        let Some(db) = connect_or_skip().await else {
            return;
        };

        let ts = FAR_FUTURE_BASE + 600.0;
        db.upsert_vessel(
            "999000001",
            Some("PG TEST SHIP"),
            Some("Cargo"),
            Some("US"),
            ts,
        )
        .await
        .expect("upsert_vessel failed");
        db.add_vessel_position(
            "999000001",
            32.0,
            -79.0,
            Some(15.0),
            Some(180.0),
            Some(175.0),
            ts,
        )
        .await
        .expect("add_vessel_position failed");

        let recent = db.get_recent_vessel_positions(100).await;
        let v = recent
            .iter()
            .find(|p| p.mmsi == "999000001")
            .expect("seeded vessel position not in get_recent_vessel_positions");
        assert!((v.lat - 32.0).abs() < 1e-6);
        assert_eq!(v.speed_kts, Some(15.0));
        assert_eq!(v.heading_deg, Some(175.0));
    }
}
