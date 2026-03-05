//! SQLite persistence — WAL mode, 6 tables, indexed queries.
//!
//! Schema: receivers, aircraft, sightings, positions, captures, events.
//! Every position and capture records which receiver heard it.

use rusqlite::{params, Connection, Result as SqlResult};
use serde::Serialize;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use adsb_core::tracker::TrackEvent;
use adsb_core::types::{icao_from_hex, icao_to_string, Icao};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS receivers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,
    email TEXT,
    lat REAL,
    lon REAL,
    altitude_ft REAL,
    description TEXT,
    api_key TEXT UNIQUE,
    created_at REAL NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_receivers_api_key ON receivers(api_key);

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

CREATE TABLE IF NOT EXISTS vessels (
    mmsi TEXT PRIMARY KEY,
    name TEXT,
    vessel_type TEXT,
    flag TEXT,
    first_seen REAL NOT NULL,
    last_seen REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS vessel_positions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi TEXT NOT NULL REFERENCES vessels(mmsi),
    lat REAL NOT NULL,
    lon REAL NOT NULL,
    speed_kts REAL,
    course_deg REAL,
    heading_deg REAL,
    timestamp REAL NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_vessel_positions_mmsi ON vessel_positions(mmsi);
CREATE INDEX IF NOT EXISTS idx_vessel_positions_timestamp ON vessel_positions(timestamp);
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

#[allow(dead_code)]
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
            .query_row(
                "SELECT id FROM receivers WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
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
            params![
                icao_str,
                country,
                registration,
                is_military as i32,
                timestamp
            ],
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

    #[allow(clippy::too_many_arguments)]
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

    #[allow(clippy::too_many_arguments)]
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
            params![
                icao_str,
                event_type,
                description,
                lat,
                lon,
                altitude_ft,
                timestamp
            ],
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
            .execute(
                "DELETE FROM positions WHERE timestamp < ?1",
                params![cutoff],
            )
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
    // Vessels (AIS)
    // -----------------------------------------------------------------------

    pub fn upsert_vessel(
        &mut self,
        mmsi: &str,
        name: Option<&str>,
        vessel_type: Option<&str>,
        flag: Option<&str>,
        timestamp: f64,
    ) {
        let _ = self.conn.execute(
            "INSERT INTO vessels (mmsi, name, vessel_type, flag, first_seen, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(mmsi) DO UPDATE SET
                 name = COALESCE(?2, name),
                 vessel_type = COALESCE(?3, vessel_type),
                 flag = COALESCE(?4, flag),
                 last_seen = ?5",
            params![mmsi, name, vessel_type, flag, timestamp],
        );
        self.maybe_commit();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_vessel_position(
        &mut self,
        mmsi: &str,
        lat: f64,
        lon: f64,
        speed_kts: Option<f64>,
        course_deg: Option<f64>,
        heading_deg: Option<f64>,
        timestamp: f64,
    ) {
        let _ = self.conn.execute(
            "INSERT INTO vessel_positions (mmsi, lat, lon, speed_kts, course_deg, heading_deg, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![mmsi, lat, lon, speed_kts, course_deg, heading_deg, timestamp],
        );
        self.maybe_commit();
    }

    pub fn get_vessels(&self, limit: i64) -> Vec<VesselRow> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT mmsi, name, vessel_type, flag, first_seen, last_seen
                 FROM vessels ORDER BY last_seen DESC LIMIT ?1",
            )
            .unwrap();
        stmt.query_map(params![limit], |row| {
            Ok(VesselRow {
                mmsi: row.get(0)?,
                name: row.get(1)?,
                vessel_type: row.get(2)?,
                flag: row.get(3)?,
                first_seen: row.get(4)?,
                last_seen: row.get(5)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn get_vessel_positions(&self, minutes: f64, limit: i64) -> Vec<VesselPositionRow> {
        let cutoff = now() - (minutes * 60.0);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT mmsi, lat, lon, speed_kts, course_deg, heading_deg, timestamp
                 FROM vessel_positions
                 WHERE timestamp >= ?1
                 ORDER BY timestamp DESC LIMIT ?2",
            )
            .unwrap();
        stmt.query_map(params![cutoff, limit], |row| {
            Ok(VesselPositionRow {
                mmsi: row.get(0)?,
                lat: row.get(1)?,
                lon: row.get(2)?,
                speed_kts: row.get(3)?,
                course_deg: row.get(4)?,
                heading_deg: row.get(5)?,
                timestamp: row.get(6)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn get_recent_vessel_positions(&self, limit: i64) -> Vec<VesselPositionRow> {
        // Get latest position per vessel
        let mut stmt = self
            .conn
            .prepare(
                "SELECT vp.mmsi, vp.lat, vp.lon, vp.speed_kts, vp.course_deg, vp.heading_deg, vp.timestamp
                 FROM vessel_positions vp
                 INNER JOIN (
                     SELECT mmsi, MAX(timestamp) as max_ts
                     FROM vessel_positions GROUP BY mmsi
                 ) latest ON vp.mmsi = latest.mmsi AND vp.timestamp = latest.max_ts
                 ORDER BY vp.timestamp DESC LIMIT ?1",
            )
            .unwrap();
        stmt.query_map(params![limit], |row| {
            Ok(VesselPositionRow {
                mmsi: row.get(0)?,
                lat: row.get(1)?,
                lon: row.get(2)?,
                speed_kts: row.get(3)?,
                course_deg: row.get(4)?,
                heading_deg: row.get(5)?,
                timestamp: row.get(6)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    // -----------------------------------------------------------------------
    // Registration
    // -----------------------------------------------------------------------

    pub fn register_receiver(
        &mut self,
        name: &str,
        email: Option<&str>,
        lat: Option<f64>,
        lon: Option<f64>,
        description: Option<&str>,
    ) -> Option<(i64, String)> {
        let api_key = uuid::Uuid::new_v4().to_string();
        let ts = now();
        match self.conn.execute(
            "INSERT INTO receivers (name, email, lat, lon, description, api_key, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![name, email, lat, lon, description, api_key, ts],
        ) {
            Ok(_) => {
                let id = self.conn.last_insert_rowid();
                Some((id, api_key))
            }
            Err(_) => None, // duplicate name
        }
    }

    pub fn lookup_receiver_by_api_key(&self, key: &str) -> Option<(i64, String)> {
        self.conn
            .query_row(
                "SELECT id, name FROM receivers WHERE api_key = ?1",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok()
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
pub struct HeatmapCell {
    pub lat: f64,
    pub lon: f64,
    pub count: i64,
    pub avg_alt: Option<i32>,
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

#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct ReceiverRow {
    pub id: i64,
    pub name: String,
    pub email: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub description: Option<String>,
    pub created_at: f64,
}

#[derive(Debug, Serialize)]
pub struct VesselRow {
    pub mmsi: String,
    pub name: Option<String>,
    pub vessel_type: Option<String>,
    pub flag: Option<String>,
    pub first_seen: f64,
    pub last_seen: f64,
}

#[derive(Debug, Serialize)]
pub struct VesselPositionRow {
    pub mmsi: String,
    pub lat: f64,
    pub lon: f64,
    pub speed_kts: Option<f64>,
    pub course_deg: Option<f64>,
    pub heading_deg: Option<f64>,
    pub timestamp: f64,
}

// ---------------------------------------------------------------------------
// Web query methods
// ---------------------------------------------------------------------------

#[allow(dead_code)]
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

    /// Get grid-aggregated heatmap density cells.
    pub fn get_heatmap_density(&self, minutes: f64, resolution: f64) -> Vec<HeatmapCell> {
        let cutoff = now() - (minutes * 60.0);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT
                     ROUND(lat / ?1) * ?1 AS cell_lat,
                     ROUND(lon / ?1) * ?1 AS cell_lon,
                     COUNT(*) AS cnt,
                     CAST(AVG(altitude_ft) AS INTEGER) AS avg_alt
                 FROM positions
                 WHERE timestamp >= ?2
                 GROUP BY cell_lat, cell_lon
                 ORDER BY cnt DESC",
            )
            .unwrap();

        stmt.query_map(params![resolution, cutoff], |r| {
            Ok(HeatmapCell {
                lat: r.get(0)?,
                lon: r.get(1)?,
                count: r.get(2)?,
                avg_alt: r.get(3)?,
            })
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
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|b| b.as_ref()).collect();

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

    /// Get all positions for replay, optionally filtered by time range.
    pub fn get_all_positions_ordered(
        &self,
        limit: i64,
        start: Option<f64>,
        end: Option<f64>,
    ) -> Vec<PositionRow> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT icao, lat, lon, altitude_ft, speed_kts, heading_deg, vertical_rate_fpm, timestamp
                 FROM positions
                 WHERE (?1 IS NULL OR timestamp >= ?1)
                   AND (?2 IS NULL OR timestamp <= ?2)
                 ORDER BY timestamp ASC LIMIT ?3",
            )
            .unwrap();

        stmt.query_map(params![start, end, limit], |r| {
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
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|b| b.as_ref()).collect();

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
                "SELECT id, name, email, lat, lon, description, created_at FROM receivers ORDER BY id",
            )
            .unwrap();

        stmt.query_map([], |r| {
            Ok(ReceiverRow {
                id: r.get(0)?,
                name: r.get(1)?,
                email: r.get(2)?,
                lat: r.get(3)?,
                lon: r.get(4)?,
                description: r.get(5)?,
                created_at: r.get(6)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }
}

// ---------------------------------------------------------------------------
// Database trait — backend abstraction for SQLite / TimescaleDB
// ---------------------------------------------------------------------------

/// Async database trait for web server use.
///
/// The CLI uses `Database` directly (synchronous, single connection).
/// The web server uses `Arc<dyn AdsbDatabase>` for backend-agnostic access.
#[allow(dead_code)]
#[async_trait::async_trait]
pub trait AdsbDatabase: Send + Sync {
    async fn stats(&self) -> DbStats;
    async fn get_all_aircraft(&self) -> Vec<AircraftRow>;
    async fn get_aircraft(&self, icao: &str) -> Option<AircraftRow>;
    async fn get_positions(&self, icao: &str, limit: i64) -> Vec<PositionRow>;
    async fn get_recent_positions(&self, minutes: f64, limit: i64) -> Vec<PositionRow>;
    async fn get_events(
        &self,
        event_type: Option<&str>,
        icao: Option<&str>,
        limit: i64,
    ) -> Vec<EventRow>;
    async fn get_trails(&self, minutes: f64, limit_per_aircraft: i64) -> Vec<PositionRow>;
    async fn get_heatmap_positions(&self, minutes: f64, limit: i64)
        -> Vec<(f64, f64, Option<i32>)>;
    async fn get_heatmap_density(&self, minutes: f64, resolution: f64) -> Vec<HeatmapCell>;
    async fn query_positions(
        &self,
        min_alt: Option<i32>,
        max_alt: Option<i32>,
        icao: Option<&str>,
        military: bool,
        limit: i64,
    ) -> Vec<PositionRow>;
    async fn get_all_positions_ordered(
        &self,
        limit: i64,
        start: Option<f64>,
        end: Option<f64>,
    ) -> Vec<PositionRow>;
    async fn get_receivers(&self) -> Vec<ReceiverRow>;
    async fn get_aircraft_history(&self, hours: f64) -> Vec<HistoryRow>;
    async fn export_positions(
        &self,
        hours: Option<f64>,
        icao: Option<&str>,
        limit: i64,
    ) -> Vec<PositionRow>;

    // Write methods for ingest persistence
    async fn upsert_aircraft(
        &self,
        icao: &str,
        country: Option<&str>,
        registration: Option<&str>,
        is_military: bool,
        timestamp: f64,
    );
    async fn upsert_sighting(
        &self,
        icao: &str,
        capture_id: Option<i64>,
        callsign: Option<&str>,
        squawk: Option<&str>,
        altitude_ft: Option<i32>,
        timestamp: f64,
    );
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
    );

    // Vessel (AIS) methods
    async fn get_vessels(&self, limit: i64) -> Vec<VesselRow>;
    async fn get_vessel_positions(&self, minutes: f64, limit: i64) -> Vec<VesselPositionRow>;
    async fn get_recent_vessel_positions(&self, limit: i64) -> Vec<VesselPositionRow>;

    // Registration methods
    async fn register_receiver(
        &self,
        name: &str,
        email: Option<&str>,
        lat: Option<f64>,
        lon: Option<f64>,
        description: Option<&str>,
    ) -> Option<(i64, String)>;
    async fn lookup_receiver_by_api_key(&self, key: &str) -> Option<(i64, String)>;
}

// ---------------------------------------------------------------------------
// SQLite backend (wraps Database, opens connection per call)
// ---------------------------------------------------------------------------

/// SQLite backend for the web server.
///
/// Each method opens a fresh connection, matching the per-request pattern.
/// The CLI uses `Database` directly for batched writes with held connections.
pub struct SqliteDb {
    pub path: String,
}

impl SqliteDb {
    pub fn new(path: String) -> Self {
        // Verify the database is accessible at startup
        let _db = Database::open(&path).expect("Failed to open SQLite database");
        SqliteDb { path }
    }

    fn open(&self) -> Database {
        Database::open(&self.path).expect("Failed to open SQLite database")
    }
}

#[async_trait::async_trait]
impl AdsbDatabase for SqliteDb {
    async fn stats(&self) -> DbStats {
        self.open().stats()
    }

    async fn get_all_aircraft(&self) -> Vec<AircraftRow> {
        self.open().get_all_aircraft()
    }

    async fn get_aircraft(&self, icao: &str) -> Option<AircraftRow> {
        self.open().get_aircraft(icao)
    }

    async fn get_positions(&self, icao: &str, limit: i64) -> Vec<PositionRow> {
        self.open().get_positions(icao, limit)
    }

    async fn get_recent_positions(&self, minutes: f64, limit: i64) -> Vec<PositionRow> {
        self.open().get_recent_positions(minutes, limit)
    }

    async fn get_events(
        &self,
        event_type: Option<&str>,
        icao: Option<&str>,
        limit: i64,
    ) -> Vec<EventRow> {
        self.open().get_events(event_type, icao, limit)
    }

    async fn get_trails(&self, minutes: f64, limit_per_aircraft: i64) -> Vec<PositionRow> {
        self.open().get_trails(minutes, limit_per_aircraft)
    }

    async fn get_heatmap_positions(
        &self,
        minutes: f64,
        limit: i64,
    ) -> Vec<(f64, f64, Option<i32>)> {
        self.open().get_heatmap_positions(minutes, limit)
    }

    async fn get_heatmap_density(&self, minutes: f64, resolution: f64) -> Vec<HeatmapCell> {
        self.open().get_heatmap_density(minutes, resolution)
    }

    async fn query_positions(
        &self,
        min_alt: Option<i32>,
        max_alt: Option<i32>,
        icao: Option<&str>,
        military: bool,
        limit: i64,
    ) -> Vec<PositionRow> {
        self.open()
            .query_positions(min_alt, max_alt, icao, military, limit)
    }

    async fn get_all_positions_ordered(
        &self,
        limit: i64,
        start: Option<f64>,
        end: Option<f64>,
    ) -> Vec<PositionRow> {
        self.open().get_all_positions_ordered(limit, start, end)
    }

    async fn get_receivers(&self) -> Vec<ReceiverRow> {
        self.open().get_receivers()
    }

    async fn get_aircraft_history(&self, hours: f64) -> Vec<HistoryRow> {
        self.open().get_aircraft_history(hours)
    }

    async fn export_positions(
        &self,
        hours: Option<f64>,
        icao: Option<&str>,
        limit: i64,
    ) -> Vec<PositionRow> {
        self.open().export_positions(hours, icao, limit)
    }

    async fn upsert_aircraft(
        &self,
        icao_str: &str,
        country: Option<&str>,
        registration: Option<&str>,
        is_military: bool,
        timestamp: f64,
    ) {
        let icao = match icao_from_hex(icao_str) {
            Some(i) => i,
            None => return,
        };
        self.open()
            .upsert_aircraft(&icao, country, registration, is_military, timestamp);
    }

    async fn upsert_sighting(
        &self,
        icao_str: &str,
        capture_id: Option<i64>,
        callsign: Option<&str>,
        squawk: Option<&str>,
        altitude_ft: Option<i32>,
        timestamp: f64,
    ) {
        let icao = match icao_from_hex(icao_str) {
            Some(i) => i,
            None => return,
        };
        self.open()
            .upsert_sighting(&icao, capture_id, callsign, squawk, altitude_ft, timestamp);
    }

    async fn add_position(
        &self,
        icao_str: &str,
        lat: f64,
        lon: f64,
        altitude_ft: Option<i32>,
        speed_kts: Option<f64>,
        heading_deg: Option<f64>,
        vertical_rate_fpm: Option<i32>,
        receiver_id: Option<i64>,
        timestamp: f64,
    ) {
        let icao = match icao_from_hex(icao_str) {
            Some(i) => i,
            None => return,
        };
        self.open().add_position(
            &icao,
            lat,
            lon,
            altitude_ft,
            speed_kts,
            heading_deg,
            vertical_rate_fpm,
            receiver_id,
            timestamp,
        );
    }

    async fn get_vessels(&self, limit: i64) -> Vec<VesselRow> {
        self.open().get_vessels(limit)
    }

    async fn get_vessel_positions(&self, minutes: f64, limit: i64) -> Vec<VesselPositionRow> {
        self.open().get_vessel_positions(minutes, limit)
    }

    async fn get_recent_vessel_positions(&self, limit: i64) -> Vec<VesselPositionRow> {
        self.open().get_recent_vessel_positions(limit)
    }

    async fn register_receiver(
        &self,
        name: &str,
        email: Option<&str>,
        lat: Option<f64>,
        lon: Option<f64>,
        description: Option<&str>,
    ) -> Option<(i64, String)> {
        self.open()
            .register_receiver(name, email, lat, lon, description)
    }

    async fn lookup_receiver_by_api_key(&self, key: &str) -> Option<(i64, String)> {
        self.open().lookup_receiver_by_api_key(key)
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
        db.add_event(
            &icao,
            "military",
            "US military aircraft",
            None,
            None,
            None,
            1.0,
        );

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
    fn test_military_sticky_flag() {
        // is_military = MAX(is_military, excluded.is_military)
        // Once set to true, it should never revert to false.
        let mut db = test_db();
        let icao = icao_from_hex("ADF7C8").unwrap();

        // First insert as military
        db.upsert_aircraft(&icao, Some("United States"), None, true, 1.0);
        assert!(db.get_aircraft("ADF7C8").unwrap().is_military);

        // Re-upsert with is_military=false — should stay true
        db.upsert_aircraft(&icao, None, None, false, 2.0);
        assert!(
            db.get_aircraft("ADF7C8").unwrap().is_military,
            "Military flag should be sticky (MAX)"
        );
    }

    #[test]
    fn test_country_preserved_on_reupsert() {
        // country = COALESCE(excluded.country, country)
        // If new insert has NULL country, the existing value should survive.
        let mut db = test_db();
        let icao = icao_from_hex("4840D6").unwrap();

        db.upsert_aircraft(&icao, Some("Netherlands"), None, false, 1.0);
        assert_eq!(
            db.get_aircraft("4840D6").unwrap().country.as_deref(),
            Some("Netherlands")
        );

        // Re-upsert with no country — should preserve "Netherlands"
        db.upsert_aircraft(&icao, None, None, false, 2.0);
        assert_eq!(
            db.get_aircraft("4840D6").unwrap().country.as_deref(),
            Some("Netherlands"),
            "Country should be preserved via COALESCE"
        );

        // Re-upsert with a different country — should overwrite
        db.upsert_aircraft(&icao, Some("Germany"), None, false, 3.0);
        assert_eq!(
            db.get_aircraft("4840D6").unwrap().country.as_deref(),
            Some("Germany")
        );
    }

    #[test]
    fn test_registration_preserved_on_reupsert() {
        let mut db = test_db();
        let icao = icao_from_hex("A12345").unwrap();

        db.upsert_aircraft(&icao, None, Some("N12345"), false, 1.0);
        assert_eq!(
            db.get_aircraft("A12345").unwrap().registration.as_deref(),
            Some("N12345")
        );

        // Re-upsert with no registration — should preserve
        db.upsert_aircraft(&icao, None, None, false, 2.0);
        assert_eq!(
            db.get_aircraft("A12345").unwrap().registration.as_deref(),
            Some("N12345"),
            "Registration should be preserved via COALESCE"
        );
    }

    #[test]
    fn test_downsample_positions() {
        let mut db = test_db();
        let icao = icao_from_hex("4840D6").unwrap();
        db.upsert_aircraft(&icao, None, None, false, 1.0);

        // Insert 10 positions at 1-second intervals, with old timestamps
        // (well before "now" so they qualify for downsampling)
        let base_ts = 1000.0; // ancient timestamp
        for i in 0..10 {
            db.add_position(
                &icao,
                52.0 + (i as f64) * 0.001,
                3.0,
                Some(38000),
                None,
                None,
                None,
                None,
                base_ts + (i as f64),
            );
        }
        assert_eq!(db.count_positions(), 10);

        // Downsample: keep one per 5-second bucket for data older than 0 hours
        // 10 positions over 9 seconds → 2 buckets (0-4s, 5-9s) → keep 2, delete 8
        let deleted = db.downsample_positions(0, 5);
        assert!(deleted > 0, "Should have thinned some positions");
        let remaining = db.count_positions();
        assert_eq!(remaining, 2, "Should keep one per 5-second bucket");
    }

    #[test]
    fn test_downsample_preserves_recent() {
        let mut db = test_db();
        let icao = icao_from_hex("4840D6").unwrap();
        db.upsert_aircraft(&icao, None, None, false, now());

        // Insert positions with current timestamps
        let base_ts = now();
        for i in 0..5 {
            db.add_position(
                &icao,
                52.0,
                3.0,
                None,
                None,
                None,
                None,
                None,
                base_ts + (i as f64),
            );
        }
        assert_eq!(db.count_positions(), 5);

        // Downsample only data older than 24 hours — these are fresh, none should be deleted
        let deleted = db.downsample_positions(24, 30);
        assert_eq!(deleted, 0, "Recent positions should not be downsampled");
        assert_eq!(db.count_positions(), 5);
    }

    #[test]
    fn test_prune_positions() {
        let mut db = test_db();
        let icao = icao_from_hex("4840D6").unwrap();
        db.upsert_aircraft(&icao, None, None, false, 1.0);

        // Old position
        db.add_position(&icao, 52.0, 3.0, None, None, None, None, None, 1000.0);
        // Recent position
        db.add_position(&icao, 52.1, 3.1, None, None, None, None, None, now());

        assert_eq!(db.count_positions(), 2);

        // Prune positions older than 1 hour — the ancient one should go
        let deleted = db.prune_positions(1);
        assert_eq!(deleted, 1);
        assert_eq!(db.count_positions(), 1);
    }

    #[test]
    fn test_prune_events() {
        let mut db = test_db();
        let icao = icao_from_hex("ADF7C8").unwrap();
        db.upsert_aircraft(&icao, None, None, true, 1.0);

        // Old event
        db.add_event(&icao, "military", "Old", None, None, None, 1000.0);
        // Recent event
        db.add_event(&icao, "military", "New", None, None, None, now());

        assert_eq!(db.count_events(), 2);

        let deleted = db.prune_events(1);
        assert_eq!(deleted, 1);
        assert_eq!(db.count_events(), 1);
    }

    #[test]
    fn test_sighting_altitude_tracking() {
        let mut db = test_db();
        let icao = icao_from_hex("4840D6").unwrap();
        db.upsert_aircraft(&icao, None, None, false, 1.0);

        // Multiple altitude updates
        db.upsert_sighting(&icao, None, Some("KLM1023"), None, Some(38000), 1.0);
        db.upsert_sighting(&icao, None, None, None, Some(35000), 2.0);
        db.upsert_sighting(&icao, None, None, None, Some(41000), 3.0);

        let (min_alt, max_alt, msg_count): (Option<i32>, Option<i32>, i64) = db
            .conn
            .query_row(
                "SELECT min_altitude_ft, max_altitude_ft, message_count FROM sightings WHERE icao = '4840D6'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(min_alt, Some(35000));
        assert_eq!(max_alt, Some(41000));
        assert_eq!(msg_count, 3);
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

    #[test]
    fn test_upsert_vessel() {
        let mut db = test_db();
        db.upsert_vessel(
            "367000001",
            Some("TEST SHIP"),
            Some("Cargo"),
            Some("US"),
            1.0,
        );

        let vessels = db.get_vessels(10);
        assert_eq!(vessels.len(), 1);
        assert_eq!(vessels[0].mmsi, "367000001");
        assert_eq!(vessels[0].name.as_deref(), Some("TEST SHIP"));
        assert_eq!(vessels[0].vessel_type.as_deref(), Some("Cargo"));
    }

    #[test]
    fn test_upsert_vessel_updates() {
        let mut db = test_db();
        db.upsert_vessel(
            "367000001",
            Some("OLD NAME"),
            Some("Cargo"),
            Some("US"),
            1.0,
        );
        db.upsert_vessel("367000001", Some("NEW NAME"), None, None, 5.0);

        let vessels = db.get_vessels(10);
        assert_eq!(vessels.len(), 1);
        assert_eq!(vessels[0].name.as_deref(), Some("NEW NAME"));
        assert_eq!(vessels[0].first_seen, 1.0);
        assert_eq!(vessels[0].last_seen, 5.0);
    }

    #[test]
    fn test_add_vessel_position() {
        let mut db = test_db();
        db.upsert_vessel("367000001", Some("TEST"), None, None, 1.0);
        db.add_vessel_position(
            "367000001",
            32.5,
            -79.8,
            Some(12.0),
            Some(180.0),
            Some(175.0),
            1.0,
        );

        let _positions = db.get_vessel_positions(60.0, 100);
        // get_vessel_positions uses now() - minutes*60, so timestamp 1.0 is very old
        // Use get_recent_vessel_positions instead for latest-per-vessel
        let latest = db.get_recent_vessel_positions(100);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].mmsi, "367000001");
        assert_eq!(latest[0].lat, 32.5);
        assert_eq!(latest[0].speed_kts, Some(12.0));
    }

    #[test]
    fn test_vessel_multiple_positions() {
        let mut db = test_db();
        db.upsert_vessel("367000001", Some("SHIP A"), None, None, 1.0);
        db.upsert_vessel("367000002", Some("SHIP B"), None, None, 1.0);
        db.add_vessel_position("367000001", 32.5, -79.8, Some(12.0), Some(180.0), None, 1.0);
        db.add_vessel_position("367000001", 32.4, -79.9, Some(11.5), Some(185.0), None, 2.0);
        db.add_vessel_position("367000002", 33.0, -78.5, Some(8.0), Some(90.0), None, 1.5);

        let latest = db.get_recent_vessel_positions(100);
        assert_eq!(latest.len(), 2);
        // Should have one per vessel (the latest for each)
        let mmsis: Vec<&str> = latest.iter().map(|p| p.mmsi.as_str()).collect();
        assert!(mmsis.contains(&"367000001"));
        assert!(mmsis.contains(&"367000002"));
    }

    // -----------------------------------------------------------------------
    // Registration tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_register_receiver() {
        let mut db = test_db();
        let result = db.register_receiver(
            "home-pi",
            Some("test@example.com"),
            Some(35.5),
            Some(-82.5),
            Some("RTL-SDR"),
        );
        assert!(result.is_some());
        let (id, api_key) = result.unwrap();
        assert!(id > 0);
        assert!(!api_key.is_empty());
        // UUID v4 format: 8-4-4-4-12 hex chars
        assert_eq!(api_key.len(), 36);
        assert_eq!(api_key.chars().filter(|c| *c == '-').count(), 4);
    }

    #[test]
    fn test_register_receiver_duplicate_name() {
        let mut db = test_db();
        let first = db.register_receiver("my-rx", None, None, None, None);
        assert!(first.is_some());
        let second = db.register_receiver("my-rx", None, None, None, None);
        assert!(second.is_none(), "Duplicate name should fail");
    }

    #[test]
    fn test_lookup_receiver_by_api_key() {
        let mut db = test_db();
        let (id, api_key) = db
            .register_receiver("test-rx", None, None, None, None)
            .unwrap();

        let lookup = db.lookup_receiver_by_api_key(&api_key);
        assert!(lookup.is_some());
        let (found_id, found_name) = lookup.unwrap();
        assert_eq!(found_id, id);
        assert_eq!(found_name, "test-rx");
    }

    #[test]
    fn test_lookup_receiver_invalid_key() {
        let db = test_db();
        let lookup = db.lookup_receiver_by_api_key("nonexistent-key");
        assert!(lookup.is_none());
    }

    #[test]
    fn test_register_receiver_unique_keys() {
        let mut db = test_db();
        let (_, key1) = db
            .register_receiver("rx-1", None, None, None, None)
            .unwrap();
        let (_, key2) = db
            .register_receiver("rx-2", None, None, None, None)
            .unwrap();
        assert_ne!(key1, key2, "Each receiver should get a unique API key");
    }
}
