//! TimescaleDB (PostgreSQL) backend — production-scale time-series storage.
//!
//! Requires the `timescaledb` feature flag and a PostgreSQL server with
//! the TimescaleDB extension installed.
//!
//! Key differences from SQLite:
//! - `positions` and `events` are TimescaleDB hypertables
//! - Automatic compression on chunks older than 7 days
//! - Retention policy drops raw positions older than 90 days
//! - Continuous aggregates provide downsampled views (30s, 5m)
//! - Connection pooling via sqlx::PgPool

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
    vertical_rate_fpm INTEGER
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

-- Retention policy: drop raw positions older than 90 days
SELECT add_retention_policy('positions', INTERVAL '90 days', if_not_exists => TRUE);

-- Events: compress after 30 days, retain for 365 days
ALTER TABLE events SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'icao',
    timescaledb.compress_orderby = 'time DESC'
);
SELECT add_compression_policy('events', INTERVAL '30 days', if_not_exists => TRUE);
SELECT add_retention_policy('events', INTERVAL '365 days', if_not_exists => TRUE);

-- Vessel positions: compress after 7 days, retain for 90 days
ALTER TABLE vessel_positions SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'mmsi',
    timescaledb.compress_orderby = 'time DESC'
);
SELECT add_compression_policy('vessel_positions', INTERVAL '7 days', if_not_exists => TRUE);
SELECT add_retention_policy('vessel_positions', INTERVAL '90 days', if_not_exists => TRUE);
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

        // Set up TimescaleDB hypertables and policies (may fail if extension not loaded)
        if let Err(e) = sqlx::raw_sql(TIMESCALE_SETUP).execute(&pool).await {
            eprintln!("Warning: TimescaleDB setup failed (extension may not be installed): {e}");
            eprintln!("Falling back to plain PostgreSQL (no compression/retention/aggregates)");
        }

        // Set up continuous aggregates (may fail without TimescaleDB)
        let _ = sqlx::raw_sql(CAGG_30S).execute(&pool).await;
        let _ = sqlx::raw_sql(CAGG_5M).execute(&pool).await;

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
        let positions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM positions WHERE time > NOW() - INTERVAL '24 hours'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
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

        DbStats {
            aircraft,
            positions,
            events,
            receivers,
            captures,
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
                    vertical_rate_fpm, EXTRACT(EPOCH FROM time)::double precision as timestamp
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
                    heading_deg, vertical_rate_fpm,
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
        let sql = format!(
            "SELECT p.icao, p.lat, p.lon, p.altitude_ft, p.speed_kts, p.heading_deg,
                    p.vertical_rate_fpm, EXTRACT(EPOCH FROM p.time)::double precision as timestamp
             FROM positions p
             LEFT JOIN aircraft a ON p.icao = a.icao
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

        let rows = sqlx::query(
            "SELECT icao, lat, lon, altitude_ft, speed_kts, heading_deg,
                    vertical_rate_fpm, EXTRACT(EPOCH FROM time)::double precision as timestamp
             FROM positions
             WHERE ($2::TIMESTAMPTZ IS NULL OR time >= $2)
               AND ($3::TIMESTAMPTZ IS NULL OR time <= $3)
             ORDER BY time ASC LIMIT $1",
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
            conditions.push(format!("time >= NOW() - ${}::INTERVAL", idx));
            idx += 1;
        }
        if icao.is_some() {
            conditions.push(format!("icao = ${}", idx));
            idx += 1;
        }

        let where_clause = if conditions.is_empty() {
            "TRUE".to_string()
        } else {
            conditions.join(" AND ")
        };

        let sql = format!(
            "SELECT icao, lat, lon, altitude_ft, speed_kts, heading_deg,
                    vertical_rate_fpm, EXTRACT(EPOCH FROM time)::double precision as timestamp
             FROM positions WHERE {where_clause}
             ORDER BY time ASC LIMIT ${idx}"
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
        receiver_id: Option<i64>,
        timestamp: f64,
    ) {
        let ts = epoch_to_pg(timestamp);
        let _ = sqlx::query(
            "INSERT INTO positions (time, icao, receiver_id, lat, lon, altitude_ft,
                                    speed_kts, heading_deg, vertical_rate_fpm)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
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
        .execute(&self.pool)
        .await;
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
    PositionRow {
        icao: r.get("icao"),
        lat: r.get("lat"),
        lon: r.get("lon"),
        altitude_ft: r.get("altitude_ft"),
        speed_kts: r.get("speed_kts"),
        heading_deg: r.get("heading_deg"),
        vertical_rate_fpm: r.get("vertical_rate_fpm"),
        timestamp: row_f64(r, "timestamp"),
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
