//! SQLite persistence — WAL mode, 6 tables, indexed queries.
//!
//! Schema: receivers, aircraft, sightings, positions, captures, events.
//! Every position and capture records which receiver heard it.

use rusqlite::{params, Connection, Result as SqlResult};
use serde::Serialize;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use adsb_core::tracker::TrackEvent;
use adsb_core::types::{icao_to_string, Icao};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS receivers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,
    lat REAL,
    lon REAL,
    altitude_ft REAL,
    description TEXT,
    created_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS aircraft (
    icao TEXT PRIMARY KEY,
    registration TEXT,
    country TEXT,
    is_military INTEGER DEFAULT 0,
    first_seen REAL NOT NULL,
    last_seen REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS sightings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    icao TEXT NOT NULL REFERENCES aircraft(icao),
    capture_id INTEGER REFERENCES captures(id),
    callsign TEXT,
    squawk TEXT,
    min_altitude_ft INTEGER,
    max_altitude_ft INTEGER,
    avg_signal REAL,
    message_count INTEGER DEFAULT 0,
    first_seen REAL NOT NULL,
    last_seen REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS positions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    icao TEXT NOT NULL REFERENCES aircraft(icao),
    receiver_id INTEGER REFERENCES receivers(id),
    lat REAL NOT NULL,
    lon REAL NOT NULL,
    altitude_ft INTEGER,
    speed_kts REAL,
    heading_deg REAL,
    vertical_rate_fpm INTEGER,
    timestamp REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS captures (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    receiver_id INTEGER REFERENCES receivers(id),
    source TEXT,
    start_time REAL,
    end_time REAL,
    total_frames INTEGER DEFAULT 0,
    valid_frames INTEGER DEFAULT 0,
    aircraft_count INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    icao TEXT NOT NULL REFERENCES aircraft(icao),
    event_type TEXT NOT NULL,
    description TEXT,
    lat REAL,
    lon REAL,
    altitude_ft INTEGER,
    timestamp REAL NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_positions_icao ON positions(icao);
CREATE INDEX IF NOT EXISTS idx_positions_timestamp ON positions(timestamp);
CREATE INDEX IF NOT EXISTS idx_positions_receiver ON positions(receiver_id);
CREATE INDEX IF NOT EXISTS idx_sightings_icao ON sightings(icao);
CREATE INDEX IF NOT EXISTS idx_sightings_icao_capture ON sightings(icao, capture_id);
CREATE INDEX IF NOT EXISTS idx_events_icao ON events(icao);
CREATE INDEX IF NOT EXISTS idx_events_type ON events(event_type);
CREATE INDEX IF NOT EXISTS idx_aircraft_last_seen ON aircraft(last_seen);
"#;

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

/// SQLite database for ADS-B aircraft tracking data.
pub struct Database {
    conn: Connection,
    autocommit: bool,
    pending: u32,
}

impl Database {
    /// Open or create a database at the given path.
    pub fn open(path: &str) -> SqlResult<Self> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()?
        } else {
            // Ensure parent directory exists
            if let Some(parent) = Path::new(path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            Connection::open(path)?
        };

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;

        Ok(Database {
            conn,
            autocommit: true,
            pending: 0,
        })
    }

    /// Open in-memory database (for testing).
    pub fn open_memory() -> SqlResult<Self> {
        Self::open(":memory:")
    }

    /// Set batch mode (disable autocommit for throughput).
    pub fn set_autocommit(&mut self, autocommit: bool) {
        self.autocommit = autocommit;
    }

    fn maybe_commit(&mut self) {
        self.pending += 1;
        if self.autocommit {
            let _ = self.conn.execute_batch("COMMIT; BEGIN;");
            self.pending = 0;
        }
    }

    /// Commit any pending writes.
    pub fn flush(&mut self) {
        if self.pending > 0 {
            let _ = self.conn.execute_batch("COMMIT; BEGIN;");
            self.pending = 0;
        }
    }

    // -----------------------------------------------------------------------
    // Apply track events
    // -----------------------------------------------------------------------

    /// Process a batch of TrackEvents from the tracker.
    pub fn apply_events(&mut self, events: &[TrackEvent]) {
        for event in events {
            match event {
                TrackEvent::NewAircraft {
                    icao,
                    country,
                    registration,
                    is_military,
                    timestamp,
                } => {
                    self.upsert_aircraft(
                        icao,
                        *country,
                        registration.as_deref(),
                        *is_military,
                        *timestamp,
                    );
                }
                TrackEvent::AircraftUpdate { icao, timestamp } => {
                    self.upsert_aircraft(icao, None, None, false, *timestamp);
                }
                TrackEvent::SightingUpdate {
                    icao,
                    capture_id,
                    callsign,
                    squawk,
                    altitude_ft,
                    timestamp,
                } => {
                    self.upsert_sighting(
                        icao,
                        *capture_id,
                        callsign.as_deref(),
                        squawk.as_deref(),
                        *altitude_ft,
                        *timestamp,
                    );
                }
                TrackEvent::PositionUpdate {
                    icao,
                    lat,
                    lon,
                    altitude_ft,
                    speed_kts,
                    heading_deg,
                    vertical_rate_fpm,
                    receiver_id,
                    timestamp,
                } => {
                    self.add_position(
                        icao,
                        *lat,
                        *lon,
                        *altitude_ft,
                        *speed_kts,
                        *heading_deg,
                        *vertical_rate_fpm,
                        *receiver_id,
                        *timestamp,
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Receivers
    // -----------------------------------------------------------------------

    /// Register a receiver. Returns receiver_id.
    pub fn add_receiver(
        &mut self,
        name: &str,
        lat: Option<f64>,
        lon: Option<f64>,
        altitude_ft: Option<f64>,
        description: &str,
    ) -> i64 {
        let _ = self.conn.execute(
            "INSERT OR IGNORE INTO receivers (name, lat, lon, altitude_ft, description, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![name, lat, lon, altitude_ft, description, now()],
        );
        self.maybe_commit();

        self.conn
            .query_row("SELECT id FROM receivers WHERE name = ?1", params![name], |r| {
                r.get(0)
            })
            .unwrap_or(0)
    }

    // -----------------------------------------------------------------------
    // Aircraft
    // -----------------------------------------------------------------------

    /// Insert or update aircraft record.
    pub fn upsert_aircraft(
        &mut self,
        icao: &Icao,
        country: Option<&str>,
        registration: Option<&str>,
        is_military: bool,
        timestamp: f64,
    ) {
        let icao_str = icao_to_string(icao);
        let _ = self.conn.execute(
            "INSERT INTO aircraft (icao, country, registration, is_military, first_seen, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(icao) DO UPDATE SET
                 country = COALESCE(excluded.country, country),
                 registration = COALESCE(excluded.registration, registration),
                 is_military = MAX(is_military, excluded.is_military),
                 last_seen = MAX(last_seen, excluded.last_seen)",
            params![icao_str, country, registration, is_military as i32, timestamp],
        );
        self.maybe_commit();
    }

    pub fn get_aircraft(&self, icao_hex: &str) -> Option<AircraftRow> {
        self.conn
            .query_row(
                "SELECT icao, registration, country, is_military, first_seen, last_seen
                 FROM aircraft WHERE icao = ?1",
                params![icao_hex],
                |r| {
                    Ok(AircraftRow {
                        icao: r.get(0)?,
                        registration: r.get(1)?,
                        country: r.get(2)?,
                        is_military: r.get::<_, i32>(3)? != 0,
                        first_seen: r.get(4)?,
                        last_seen: r.get(5)?,
                    })
                },
            )
            .ok()
    }

    pub fn count_aircraft(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM aircraft", [], |r| r.get(0))
            .unwrap_or(0)
    }

    // -----------------------------------------------------------------------
    // Positions
    // -----------------------------------------------------------------------

    pub fn add_position(
        &mut self,
        icao: &Icao,
        lat: f64,
        lon: f64,
        altitude_ft: Option<i32>,
        speed_kts: Option<f64>,
        heading_deg: Option<f64>,
        vertical_rate_fpm: Option<i32>,
        receiver_id: Option<i64>,
        timestamp: f64,
    ) {
        let icao_str = icao_to_string(icao);
        let _ = self.conn.execute(
            "INSERT INTO positions (icao, receiver_id, lat, lon, altitude_ft, speed_kts, heading_deg, vertical_rate_fpm, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![icao_str, receiver_id, lat, lon, altitude_ft, speed_kts, heading_deg, vertical_rate_fpm, timestamp],
        );
        self.maybe_commit();
    }

    pub fn get_positions(&self, icao_hex: &str, limit: i64) -> Vec<PositionRow> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT icao, lat, lon, altitude_ft, speed_kts, heading_deg, vertical_rate_fpm, timestamp
                 FROM positions WHERE icao = ?1 ORDER BY timestamp DESC LIMIT ?2",
            )
            .unwrap();

        stmt.query_map(params![icao_hex, limit], |r| {
            Ok(PositionRow {
                icao: r.get(0)?,
                lat: r.get(1)?,
                lon: r.get(2)?,
                altitude_ft: r.get(3)?,
                speed_kts: r.get(4)?,
                heading_deg: r.get(5)?,
                vertical_rate_fpm: r.get(6)?,
                timestamp: r.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn count_positions(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM positions", [], |r| r.get(0))
            .unwrap_or(0)
    }

    // -----------------------------------------------------------------------
    // Captures
    // -----------------------------------------------------------------------

    /// Start a new capture session. Returns capture_id.
    pub fn start_capture(&mut self, source: &str, receiver_id: Option<i64>) -> i64 {
        self.conn
            .execute(
                "INSERT INTO captures (receiver_id, source, start_time, total_frames, valid_frames, aircraft_count)
                 VALUES (?1, ?2, ?3, 0, 0, 0)",
                params![receiver_id, source, now()],
            )
            .unwrap();
        self.maybe_commit();
        self.conn.last_insert_rowid()
    }

    pub fn end_capture(
        &mut self,
        capture_id: i64,
        total_frames: u64,
        valid_frames: u64,
        aircraft_count: u64,
    ) {
        let _ = self.conn.execute(
            "UPDATE captures SET end_time = ?1, total_frames = ?2, valid_frames = ?3, aircraft_count = ?4
             WHERE id = ?5",
            params![now(), total_frames, valid_frames, aircraft_count, capture_id],
        );
        self.maybe_commit();
    }

    // -----------------------------------------------------------------------
    // Events
    // -----------------------------------------------------------------------

    pub fn add_event(
        &mut self,
        icao: &Icao,
        event_type: &str,
        description: &str,
        lat: Option<f64>,
        lon: Option<f64>,
        altitude_ft: Option<i32>,
        timestamp: f64,
    ) {
        let icao_str = icao_to_string(icao);
        let _ = self.conn.execute(
            "INSERT INTO events (icao, event_type, description, lat, lon, altitude_ft, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![icao_str, event_type, description, lat, lon, altitude_ft, timestamp],
        );
        self.maybe_commit();
    }

    pub fn count_events(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap_or(0)
    }

    // -----------------------------------------------------------------------
    // Sightings
    // -----------------------------------------------------------------------

    pub fn upsert_sighting(
        &mut self,
        icao: &Icao,
        capture_id: Option<i64>,
        callsign: Option<&str>,
        squawk: Option<&str>,
        altitude_ft: Option<i32>,
        timestamp: f64,
    ) {
        let icao_str = icao_to_string(icao);

        // Check for existing sighting
        let existing: Option<(i64, Option<i32>, Option<i32>)> = self
            .conn
            .query_row(
                "SELECT id, min_altitude_ft, max_altitude_ft FROM sightings
                 WHERE icao = ?1 AND capture_id IS ?2",
                params![icao_str, capture_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();

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
            let _ = self.conn.execute(
                "UPDATE sightings SET
                     callsign = COALESCE(?1, callsign),
                     squawk = COALESCE(?2, squawk),
                     min_altitude_ft = ?3,
                     max_altitude_ft = ?4,
                     message_count = message_count + 1,
                     last_seen = ?5
                 WHERE id = ?6",
                params![callsign, squawk, new_min, new_max, timestamp, id],
            );
        } else {
            let _ = self.conn.execute(
                "INSERT INTO sightings
                 (icao, capture_id, callsign, squawk, min_altitude_ft, max_altitude_ft, message_count, first_seen, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, 1, ?6, ?6)",
                params![icao_str, capture_id, callsign, squawk, altitude_ft, timestamp],
            );
        }
        self.maybe_commit();
    }

    // -----------------------------------------------------------------------
    // Maintenance
    // -----------------------------------------------------------------------

    /// Delete positions older than max_age_hours. Returns rows deleted.
    pub fn prune_positions(&mut self, max_age_hours: i64) -> usize {
        let cutoff = now() - (max_age_hours as f64 * 3600.0);
        let count = self
            .conn
            .execute("DELETE FROM positions WHERE timestamp < ?1", params![cutoff])
            .unwrap_or(0);
        let _ = self.conn.execute_batch("COMMIT; BEGIN;");
        count
    }

    /// Delete events older than max_age_hours. Returns rows deleted.
    pub fn prune_events(&mut self, max_age_hours: i64) -> usize {
        let cutoff = now() - (max_age_hours as f64 * 3600.0);
        let count = self
            .conn
            .execute("DELETE FROM events WHERE timestamp < ?1", params![cutoff])
            .unwrap_or(0);
        let _ = self.conn.execute_batch("COMMIT; BEGIN;");
        count
    }

    /// Thin old positions to one per aircraft per interval. Returns rows deleted.
    pub fn downsample_positions(&mut self, older_than_hours: i64, keep_interval_sec: i64) -> usize {
        let cutoff = now() - (older_than_hours as f64 * 3600.0);
        let count = self
            .conn
            .execute(
                "DELETE FROM positions WHERE timestamp < ?1 AND id NOT IN (
                    SELECT MAX(id) FROM positions
                    WHERE timestamp < ?1
                    GROUP BY icao, CAST(timestamp / ?2 AS INTEGER)
                )",
                params![cutoff, keep_interval_sec],
            )
            .unwrap_or(0);
        let _ = self.conn.execute_batch("COMMIT; BEGIN;");
        count
    }

    /// Delete phantom aircraft (no positions, old). Returns total rows deleted.
    pub fn prune_phantom_aircraft(&mut self, min_age_hours: f64) -> usize {
        let cutoff = now() - (min_age_hours * 3600.0);
        let c1 = self
            .conn
            .execute(
                "DELETE FROM sightings WHERE icao IN (
                    SELECT icao FROM aircraft WHERE icao NOT IN
                    (SELECT DISTINCT icao FROM positions) AND last_seen < ?1
                )",
                params![cutoff],
            )
            .unwrap_or(0);
        let c2 = self
            .conn
            .execute(
                "DELETE FROM events WHERE icao IN (
                    SELECT icao FROM aircraft WHERE icao NOT IN
                    (SELECT DISTINCT icao FROM positions) AND last_seen < ?1
                )",
                params![cutoff],
            )
            .unwrap_or(0);
        let c3 = self
            .conn
            .execute(
                "DELETE FROM aircraft WHERE icao IN (
                    SELECT icao FROM aircraft WHERE icao NOT IN
                    (SELECT DISTINCT icao FROM positions) AND last_seen < ?1
                )",
                params![cutoff],
            )
            .unwrap_or(0);
        let _ = self.conn.execute_batch("COMMIT; BEGIN;");
        c1 + c2 + c3
    }

    /// VACUUM to reclaim disk space.
    pub fn vacuum(&mut self) {
        let _ = self.conn.execute_batch("VACUUM;");
    }

    // -----------------------------------------------------------------------
    // Stats
    // -----------------------------------------------------------------------

    pub fn stats(&self) -> DbStats {
        DbStats {
            aircraft: self.count_aircraft(),
            positions: self.count_positions(),
            events: self.count_events(),
            receivers: self
                .conn
                .query_row("SELECT COUNT(*) FROM receivers", [], |r| r.get(0))
                .unwrap_or(0),
            captures: self
                .conn
                .query_row("SELECT COUNT(*) FROM captures", [], |r| r.get(0))
                .unwrap_or(0),
        }
    }
}

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct AircraftRow {
    pub icao: String,
    pub registration: Option<String>,
    pub country: Option<String>,
    pub is_military: bool,
    pub first_seen: f64,
    pub last_seen: f64,
}

#[derive(Debug, Serialize)]
pub struct PositionRow {
    pub icao: String,
    pub lat: f64,
    pub lon: f64,
    pub altitude_ft: Option<i32>,
    pub speed_kts: Option<f64>,
    pub heading_deg: Option<f64>,
    pub vertical_rate_fpm: Option<i32>,
    pub timestamp: f64,
}

#[derive(Debug, Serialize)]
pub struct DbStats {
    pub aircraft: i64,
    pub positions: i64,
    pub events: i64,
    pub receivers: i64,
    pub captures: i64,
}

#[derive(Debug, Serialize)]
pub struct HistoryRow {
    pub icao: String,
    pub callsign: Option<String>,
    pub country: Option<String>,
    pub is_military: bool,
    pub min_altitude_ft: Option<i32>,
    pub max_altitude_ft: Option<i32>,
    pub message_count: i64,
    pub first_seen: f64,
    pub last_seen: f64,
}

#[derive(Debug, Serialize)]
pub struct EventRow {
    pub id: i64,
    pub icao: String,
    pub event_type: String,
    pub description: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub altitude_ft: Option<i32>,
    pub timestamp: f64,
}

#[derive(Debug, Serialize)]
pub struct ReceiverRow {
    pub id: i64,
    pub name: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub description: Option<String>,
    pub created_at: f64,
}

// ---------------------------------------------------------------------------
// Web query methods
// ---------------------------------------------------------------------------

impl Database {
    /// Get all aircraft ordered by last_seen DESC.
    pub fn get_all_aircraft(&self) -> Vec<AircraftRow> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT icao, registration, country, is_military, first_seen, last_seen
                 FROM aircraft ORDER BY last_seen DESC",
            )
            .unwrap();

        stmt.query_map([], |r| {
            Ok(AircraftRow {
                icao: r.get(0)?,
                registration: r.get(1)?,
                country: r.get(2)?,
                is_military: r.get::<_, i32>(3)? != 0,
                first_seen: r.get(4)?,
                last_seen: r.get(5)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Get recent positions within a time window.
    pub fn get_recent_positions(&self, minutes: f64, limit: i64) -> Vec<PositionRow> {
        let cutoff = now() - (minutes * 60.0);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT icao, lat, lon, altitude_ft, speed_kts, heading_deg, vertical_rate_fpm, timestamp
                 FROM positions WHERE timestamp >= ?1 ORDER BY timestamp DESC LIMIT ?2",
            )
            .unwrap();

        stmt.query_map(params![cutoff, limit], |r| {
            Ok(PositionRow {
                icao: r.get(0)?,
                lat: r.get(1)?,
                lon: r.get(2)?,
                altitude_ft: r.get(3)?,
                speed_kts: r.get(4)?,
                heading_deg: r.get(5)?,
                vertical_rate_fpm: r.get(6)?,
                timestamp: r.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Get events with optional type and ICAO filters.
    pub fn get_events(
        &self,
        event_type: Option<&str>,
        icao: Option<&str>,
        limit: i64,
    ) -> Vec<EventRow> {
        let sql = match (event_type, icao) {
            (Some(_), Some(_)) => {
                "SELECT id, icao, event_type, description, lat, lon, altitude_ft, timestamp
                 FROM events WHERE event_type = ?1 AND icao = ?2
                 ORDER BY timestamp DESC LIMIT ?3"
            }
            (Some(_), None) => {
                "SELECT id, icao, event_type, description, lat, lon, altitude_ft, timestamp
                 FROM events WHERE event_type = ?1
                 ORDER BY timestamp DESC LIMIT ?3"
            }
            (None, Some(_)) => {
                "SELECT id, icao, event_type, description, lat, lon, altitude_ft, timestamp
                 FROM events WHERE icao = ?2
                 ORDER BY timestamp DESC LIMIT ?3"
            }
            (None, None) => {
                "SELECT id, icao, event_type, description, lat, lon, altitude_ft, timestamp
                 FROM events ORDER BY timestamp DESC LIMIT ?3"
            }
        };

        let mut stmt = self.conn.prepare(sql).unwrap();
        let et = event_type.unwrap_or("");
        let ic = icao.unwrap_or("");

        stmt.query_map(params![et, ic, limit], |r| {
            Ok(EventRow {
                id: r.get(0)?,
                icao: r.get(1)?,
                event_type: r.get(2)?,
                description: r.get(3)?,
                lat: r.get(4)?,
                lon: r.get(5)?,
                altitude_ft: r.get(6)?,
                timestamp: r.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Get position trails within a time window.
    pub fn get_trails(&self, minutes: f64, limit_per_aircraft: i64) -> Vec<PositionRow> {
        let cutoff = now() - (minutes * 60.0);
        // Use window function to limit per aircraft
        let mut stmt = self
            .conn
            .prepare(
                "SELECT icao, lat, lon, altitude_ft, speed_kts, heading_deg, vertical_rate_fpm, timestamp
                 FROM (
                     SELECT *, ROW_NUMBER() OVER (PARTITION BY icao ORDER BY timestamp DESC) as rn
                     FROM positions WHERE timestamp >= ?1
                 ) WHERE rn <= ?2
                 ORDER BY icao, timestamp ASC",
            )
            .unwrap();

        stmt.query_map(params![cutoff, limit_per_aircraft], |r| {
            Ok(PositionRow {
                icao: r.get(0)?,
                lat: r.get(1)?,
                lon: r.get(2)?,
                altitude_ft: r.get(3)?,
                speed_kts: r.get(4)?,
                heading_deg: r.get(5)?,
                vertical_rate_fpm: r.get(6)?,
                timestamp: r.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Get heatmap data points.
    pub fn get_heatmap_positions(&self, minutes: f64, limit: i64) -> Vec<(f64, f64, Option<i32>)> {
        let cutoff = now() - (minutes * 60.0);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT lat, lon, altitude_ft FROM positions
                 WHERE timestamp >= ?1
                 ORDER BY RANDOM() LIMIT ?2",
            )
            .unwrap();

        stmt.query_map(params![cutoff, limit], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Query positions with filters.
    pub fn query_positions(
        &self,
        min_alt: Option<i32>,
        max_alt: Option<i32>,
        icao: Option<&str>,
        military: bool,
        limit: i64,
    ) -> Vec<PositionRow> {
        let mut conditions = vec!["1=1".to_string()];
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(min) = min_alt {
            conditions.push(format!("altitude_ft >= ?{}", bind_values.len() + 1));
            bind_values.push(Box::new(min));
        }
        if let Some(max) = max_alt {
            conditions.push(format!("altitude_ft <= ?{}", bind_values.len() + 1));
            bind_values.push(Box::new(max));
        }
        if let Some(ic) = icao {
            conditions.push(format!("p.icao = ?{}", bind_values.len() + 1));
            bind_values.push(Box::new(ic.to_string()));
        }
        if military {
            conditions.push("a.is_military = 1".to_string());
        }

        let where_clause = conditions.join(" AND ");
        let sql = format!(
            "SELECT p.icao, p.lat, p.lon, p.altitude_ft, p.speed_kts, p.heading_deg, p.vertical_rate_fpm, p.timestamp
             FROM positions p
             LEFT JOIN aircraft a ON p.icao = a.icao
             WHERE {where_clause}
             ORDER BY p.timestamp DESC LIMIT ?{}",
            bind_values.len() + 1
        );

        bind_values.push(Box::new(limit));

        let mut stmt = self.conn.prepare(&sql).unwrap();
        let refs: Vec<&dyn rusqlite::types::ToSql> = bind_values.iter().map(|b| b.as_ref()).collect();

        stmt.query_map(refs.as_slice(), |r| {
            Ok(PositionRow {
                icao: r.get(0)?,
                lat: r.get(1)?,
                lon: r.get(2)?,
                altitude_ft: r.get(3)?,
                speed_kts: r.get(4)?,
                heading_deg: r.get(5)?,
                vertical_rate_fpm: r.get(6)?,
                timestamp: r.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Get all positions for replay.
    pub fn get_all_positions_ordered(&self, limit: i64) -> Vec<PositionRow> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT icao, lat, lon, altitude_ft, speed_kts, heading_deg, vertical_rate_fpm, timestamp
                 FROM positions ORDER BY timestamp ASC LIMIT ?1",
            )
            .unwrap();

        stmt.query_map(params![limit], |r| {
            Ok(PositionRow {
                icao: r.get(0)?,
                lat: r.get(1)?,
                lon: r.get(2)?,
                altitude_ft: r.get(3)?,
                speed_kts: r.get(4)?,
                heading_deg: r.get(5)?,
                vertical_rate_fpm: r.get(6)?,
                timestamp: r.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Get aircraft seen within a time window, with latest sighting info.
    pub fn get_aircraft_history(&self, hours: f64) -> Vec<HistoryRow> {
        let cutoff = now() - (hours * 3600.0);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT a.icao, s.callsign, a.country, a.is_military,
                        s.min_altitude_ft, s.max_altitude_ft,
                        s.message_count, a.first_seen, a.last_seen
                 FROM aircraft a
                 LEFT JOIN sightings s ON a.icao = s.icao
                 WHERE a.last_seen >= ?1
                 ORDER BY a.last_seen DESC",
            )
            .unwrap();

        stmt.query_map(params![cutoff], |r| {
            Ok(HistoryRow {
                icao: r.get(0)?,
                callsign: r.get(1)?,
                country: r.get(2)?,
                is_military: r.get::<_, Option<i32>>(3)?.unwrap_or(0) != 0,
                min_altitude_ft: r.get(4)?,
                max_altitude_ft: r.get(5)?,
                message_count: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
                first_seen: r.get(7)?,
                last_seen: r.get(8)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Export all positions as flat records.
    pub fn export_positions(
        &self,
        hours: Option<f64>,
        icao: Option<&str>,
        limit: i64,
    ) -> Vec<PositionRow> {
        let mut conditions = Vec::new();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(h) = hours {
            let cutoff = now() - (h * 3600.0);
            conditions.push(format!("timestamp >= ?{}", bind_values.len() + 1));
            bind_values.push(Box::new(cutoff));
        }
        if let Some(ic) = icao {
            conditions.push(format!("icao = ?{}", bind_values.len() + 1));
            bind_values.push(Box::new(ic.to_string()));
        }

        let where_clause = if conditions.is_empty() {
            "1=1".to_string()
        } else {
            conditions.join(" AND ")
        };

        let sql = format!(
            "SELECT icao, lat, lon, altitude_ft, speed_kts, heading_deg, vertical_rate_fpm, timestamp
             FROM positions WHERE {where_clause}
             ORDER BY timestamp ASC LIMIT ?{}",
            bind_values.len() + 1
        );

        bind_values.push(Box::new(limit));

        let mut stmt = self.conn.prepare(&sql).unwrap();
        let refs: Vec<&dyn rusqlite::types::ToSql> = bind_values.iter().map(|b| b.as_ref()).collect();

        stmt.query_map(refs.as_slice(), |r| {
            Ok(PositionRow {
                icao: r.get(0)?,
                lat: r.get(1)?,
                lon: r.get(2)?,
                altitude_ft: r.get(3)?,
                speed_kts: r.get(4)?,
                heading_deg: r.get(5)?,
                vertical_rate_fpm: r.get(6)?,
                timestamp: r.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Get all receivers.
    pub fn get_receivers(&self) -> Vec<ReceiverRow> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, lat, lon, description, created_at FROM receivers ORDER BY id",
            )
            .unwrap();

        stmt.query_map([], |r| {
            Ok(ReceiverRow {
                id: r.get(0)?,
                name: r.get(1)?,
                lat: r.get(2)?,
                lon: r.get(3)?,
                description: r.get(4)?,
                created_at: r.get(5)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use adsb_core::types::icao_from_hex;

    fn test_db() -> Database {
        Database::open_memory().unwrap()
    }

    #[test]
    fn test_open_memory() {
        let db = test_db();
        assert_eq!(db.count_aircraft(), 0);
    }

    #[test]
    fn test_upsert_aircraft() {
        let mut db = test_db();
        let icao = icao_from_hex("4840D6").unwrap();
        db.upsert_aircraft(&icao, Some("Netherlands"), None, false, 1.0);

        let ac = db.get_aircraft("4840D6").unwrap();
        assert_eq!(ac.country.as_deref(), Some("Netherlands"));
        assert!(!ac.is_military);
        assert_eq!(ac.first_seen, 1.0);
    }

    #[test]
    fn test_upsert_aircraft_updates_last_seen() {
        let mut db = test_db();
        let icao = icao_from_hex("4840D6").unwrap();
        db.upsert_aircraft(&icao, Some("Netherlands"), None, false, 1.0);
        db.upsert_aircraft(&icao, None, None, false, 5.0);

        let ac = db.get_aircraft("4840D6").unwrap();
        assert_eq!(ac.first_seen, 1.0);
        assert_eq!(ac.last_seen, 5.0);
    }

    #[test]
    fn test_add_position() {
        let mut db = test_db();
        let icao = icao_from_hex("40621D").unwrap();
        db.upsert_aircraft(&icao, Some("UK"), None, false, 1.0);
        db.add_position(&icao, 52.25, 3.92, Some(38000), None, None, None, None, 1.0);

        assert_eq!(db.count_positions(), 1);
        let positions = db.get_positions("40621D", 10);
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].lat, 52.25);
        assert_eq!(positions[0].altitude_ft, Some(38000));
    }

    #[test]
    fn test_multiple_positions() {
        let mut db = test_db();
        let icao = icao_from_hex("40621D").unwrap();
        db.upsert_aircraft(&icao, Some("UK"), None, false, 1.0);
        db.add_position(&icao, 52.25, 3.92, Some(38000), None, None, None, None, 1.0);
        db.add_position(&icao, 52.26, 3.93, Some(38100), None, None, None, None, 2.0);
        db.add_position(&icao, 52.27, 3.94, Some(38200), None, None, None, None, 3.0);

        assert_eq!(db.count_positions(), 3);
        let positions = db.get_positions("40621D", 2);
        assert_eq!(positions.len(), 2);
        // Should be most recent first
        assert_eq!(positions[0].timestamp, 3.0);
    }

    #[test]
    fn test_add_receiver() {
        let mut db = test_db();
        let id = db.add_receiver("test-rx", Some(35.5), Some(-82.5), None, "Test");
        assert!(id > 0);

        // Adding same name returns same id
        let id2 = db.add_receiver("test-rx", Some(35.5), Some(-82.5), None, "Test");
        assert_eq!(id, id2);
    }

    #[test]
    fn test_capture_lifecycle() {
        let mut db = test_db();
        let cap_id = db.start_capture("test.txt", None);
        assert!(cap_id > 0);

        db.end_capture(cap_id, 100, 80, 5);

        let stats = db.stats();
        assert_eq!(stats.captures, 1);
    }

    #[test]
    fn test_add_event() {
        let mut db = test_db();
        let icao = icao_from_hex("ADF7C8").unwrap();
        db.upsert_aircraft(&icao, Some("United States"), None, true, 1.0);
        db.add_event(&icao, "military", "US military aircraft", None, None, None, 1.0);

        assert_eq!(db.count_events(), 1);
    }

    #[test]
    fn test_upsert_sighting_create() {
        let mut db = test_db();
        let icao = icao_from_hex("4840D6").unwrap();
        db.upsert_aircraft(&icao, None, None, false, 1.0);
        db.upsert_sighting(&icao, None, Some("KLM1023"), None, Some(38000), 1.0);

        // Verify via count
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM sightings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_upsert_sighting_update() {
        let mut db = test_db();
        let icao = icao_from_hex("4840D6").unwrap();
        db.upsert_aircraft(&icao, None, None, false, 1.0);

        db.upsert_sighting(&icao, None, Some("KLM1023"), None, Some(38000), 1.0);
        db.upsert_sighting(&icao, None, None, Some("1234"), Some(39000), 2.0);

        // Should still be 1 sighting (updated, not duplicated)
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM sightings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Check altitude tracking
        let (min_alt, max_alt): (Option<i32>, Option<i32>) = db
            .conn
            .query_row(
                "SELECT min_altitude_ft, max_altitude_ft FROM sightings WHERE icao = '4840D6'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(min_alt, Some(38000));
        assert_eq!(max_alt, Some(39000));
    }

    #[test]
    fn test_stats() {
        let mut db = test_db();
        let icao = icao_from_hex("4840D6").unwrap();
        db.upsert_aircraft(&icao, None, None, false, 1.0);
        db.add_position(&icao, 52.0, 3.0, None, None, None, None, None, 1.0);

        let stats = db.stats();
        assert_eq!(stats.aircraft, 1);
        assert_eq!(stats.positions, 1);
        assert_eq!(stats.events, 0);
    }

    #[test]
    fn test_count_aircraft() {
        let mut db = test_db();
        assert_eq!(db.count_aircraft(), 0);

        let icao1 = icao_from_hex("4840D6").unwrap();
        let icao2 = icao_from_hex("40621D").unwrap();
        db.upsert_aircraft(&icao1, None, None, false, 1.0);
        db.upsert_aircraft(&icao2, None, None, false, 1.0);

        assert_eq!(db.count_aircraft(), 2);
    }

    #[test]
    fn test_apply_track_events() {
        let mut db = test_db();

        let icao = [0x48, 0x40, 0xD6];
        let events = vec![
            TrackEvent::NewAircraft {
                icao,
                country: Some("Netherlands"),
                registration: None,
                is_military: false,
                timestamp: 1.0,
            },
            TrackEvent::PositionUpdate {
                icao,
                lat: 52.25,
                lon: 3.92,
                altitude_ft: Some(38000),
                speed_kts: Some(450.0),
                heading_deg: Some(90.0),
                vertical_rate_fpm: None,
                receiver_id: None,
                timestamp: 1.0,
            },
            TrackEvent::SightingUpdate {
                icao,
                capture_id: None,
                callsign: Some("KLM1023".into()),
                squawk: None,
                altitude_ft: Some(38000),
                timestamp: 1.0,
            },
        ];

        db.apply_events(&events);

        assert_eq!(db.count_aircraft(), 1);
        assert_eq!(db.count_positions(), 1);

        let ac = db.get_aircraft("4840D6").unwrap();
        assert_eq!(ac.country.as_deref(), Some("Netherlands"));
    }

    #[test]
    fn test_prune_phantom_aircraft() {
        let mut db = test_db();

        // Aircraft with positions (should survive)
        let real = icao_from_hex("4840D6").unwrap();
        db.upsert_aircraft(&real, None, None, false, 0.0);
        db.add_position(&real, 52.0, 3.0, None, None, None, None, None, 0.0);

        // Aircraft without positions, old (should be pruned)
        let phantom = icao_from_hex("AAAAAA").unwrap();
        db.upsert_aircraft(&phantom, None, None, false, 0.0);

        assert_eq!(db.count_aircraft(), 2);

        // Prune phantoms older than 0 hours (everything old)
        let deleted = db.prune_phantom_aircraft(0.0);
        assert!(deleted > 0);
        assert_eq!(db.count_aircraft(), 1);
        assert!(db.get_aircraft("4840D6").is_some());
        assert!(db.get_aircraft("AAAAAA").is_none());
    }
}
