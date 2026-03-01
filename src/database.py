"""SQLite persistence — WAL mode, 6 tables, indexed queries.

Schema:
- receivers:  Sensor nodes. Name, lat, lon, altitude, description. Multi-receiver from day one.
- aircraft:   One row per unique ICAO address. Country, registration, military flag, first/last seen.
- sightings:  Per capture session per aircraft. Callsign, squawk, signal strength stats.
- positions:  Time-series lat/lon/alt/speed/heading/vrate. Tagged with receiver_id.
- captures:   Metadata per capture session. Source file, duration, frame counts. Tagged with receiver_id.
- events:     Detected anomalies. Emergency squawk, rapid descent, military, geofence breach.

Every position and capture records which receiver heard it. Single-receiver deployments
have one row in receivers. Adding receivers is adding data sources, not refactoring.
"""

from __future__ import annotations

import sqlite3
import time
from pathlib import Path

SCHEMA = """
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
"""


class Database:
    """SQLite database for ADS-B aircraft tracking data."""

    def __init__(self, path: str | Path = ":memory:", autocommit: bool = True):
        self.path = str(path)
        self._conn: sqlite3.Connection | None = None
        self._autocommit = autocommit
        self._pending = 0

    @property
    def conn(self) -> sqlite3.Connection:
        if self._conn is None:
            self._conn = sqlite3.connect(self.path)
            self._conn.row_factory = sqlite3.Row
            self._conn.execute("PRAGMA journal_mode=WAL")
            self._conn.execute("PRAGMA foreign_keys=ON")
            self._conn.executescript(SCHEMA)
        return self._conn

    def _maybe_commit(self):
        """Commit immediately if autocommit, otherwise batch."""
        self._pending += 1
        if self._autocommit:
            self.conn.commit()
            self._pending = 0

    def flush(self):
        """Commit any pending writes."""
        if self._conn and self._pending > 0:
            self._conn.commit()
            self._pending = 0

    def close(self):
        if self._conn:
            self.flush()
            self._conn.close()
            self._conn = None

    # --- Receivers ---

    def add_receiver(
        self,
        name: str,
        lat: float | None = None,
        lon: float | None = None,
        altitude_ft: float | None = None,
        description: str = "",
    ) -> int:
        """Register a receiver. Returns receiver_id."""
        cur = self.conn.execute(
            """INSERT OR IGNORE INTO receivers (name, lat, lon, altitude_ft, description, created_at)
               VALUES (?, ?, ?, ?, ?, ?)""",
            (name, lat, lon, altitude_ft, description, time.time()),
        )
        self._maybe_commit()
        if cur.lastrowid and cur.rowcount > 0:
            return cur.lastrowid
        # Already exists — fetch id
        row = self.conn.execute(
            "SELECT id FROM receivers WHERE name = ?", (name,)
        ).fetchone()
        return row["id"]

    def get_receiver(self, name: str) -> dict | None:
        row = self.conn.execute(
            "SELECT * FROM receivers WHERE name = ?", (name,)
        ).fetchone()
        return dict(row) if row else None

    # --- Aircraft ---

    def upsert_aircraft(
        self,
        icao: str,
        country: str | None = None,
        registration: str | None = None,
        is_military: bool = False,
        timestamp: float | None = None,
    ):
        """Insert or update aircraft record."""
        ts = timestamp or time.time()
        self.conn.execute(
            """INSERT INTO aircraft (icao, country, registration, is_military, first_seen, last_seen)
               VALUES (?, ?, ?, ?, ?, ?)
               ON CONFLICT(icao) DO UPDATE SET
                   country = COALESCE(excluded.country, country),
                   registration = COALESCE(excluded.registration, registration),
                   is_military = MAX(is_military, excluded.is_military),
                   last_seen = MAX(last_seen, excluded.last_seen)""",
            (icao, country, registration, int(is_military), ts, ts),
        )
        self._maybe_commit()

    def get_aircraft(self, icao: str) -> dict | None:
        row = self.conn.execute(
            "SELECT * FROM aircraft WHERE icao = ?", (icao,)
        ).fetchone()
        return dict(row) if row else None

    def count_aircraft(self) -> int:
        row = self.conn.execute("SELECT COUNT(*) as cnt FROM aircraft").fetchone()
        return row["cnt"]

    # --- Positions ---

    def add_position(
        self,
        icao: str,
        lat: float,
        lon: float,
        altitude_ft: int | None = None,
        speed_kts: float | None = None,
        heading_deg: float | None = None,
        vertical_rate_fpm: int | None = None,
        receiver_id: int | None = None,
        timestamp: float | None = None,
    ):
        """Record a position report."""
        ts = timestamp or time.time()
        self.conn.execute(
            """INSERT INTO positions
               (icao, receiver_id, lat, lon, altitude_ft, speed_kts, heading_deg, vertical_rate_fpm, timestamp)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            (icao, receiver_id, lat, lon, altitude_ft, speed_kts, heading_deg, vertical_rate_fpm, ts),
        )
        self._maybe_commit()

    def get_positions(self, icao: str, limit: int = 100) -> list[dict]:
        rows = self.conn.execute(
            "SELECT * FROM positions WHERE icao = ? ORDER BY timestamp DESC LIMIT ?",
            (icao, limit),
        ).fetchall()
        return [dict(r) for r in rows]

    def count_positions(self) -> int:
        row = self.conn.execute("SELECT COUNT(*) as cnt FROM positions").fetchone()
        return row["cnt"]

    # --- Captures ---

    def start_capture(
        self,
        source: str = "",
        receiver_id: int | None = None,
    ) -> int:
        """Start a new capture session. Returns capture_id."""
        cur = self.conn.execute(
            """INSERT INTO captures (receiver_id, source, start_time, total_frames, valid_frames, aircraft_count)
               VALUES (?, ?, ?, 0, 0, 0)""",
            (receiver_id, source, time.time()),
        )
        self._maybe_commit()
        return cur.lastrowid

    def end_capture(
        self,
        capture_id: int,
        total_frames: int = 0,
        valid_frames: int = 0,
        aircraft_count: int = 0,
    ):
        """Finalize a capture session."""
        self.conn.execute(
            """UPDATE captures SET end_time = ?, total_frames = ?, valid_frames = ?, aircraft_count = ?
               WHERE id = ?""",
            (time.time(), total_frames, valid_frames, aircraft_count, capture_id),
        )
        self._maybe_commit()

    # --- Events ---

    def add_event(
        self,
        icao: str,
        event_type: str,
        description: str = "",
        lat: float | None = None,
        lon: float | None = None,
        altitude_ft: int | None = None,
        timestamp: float | None = None,
    ):
        """Record a detected event/anomaly."""
        ts = timestamp or time.time()
        self.conn.execute(
            """INSERT INTO events (icao, event_type, description, lat, lon, altitude_ft, timestamp)
               VALUES (?, ?, ?, ?, ?, ?, ?)""",
            (icao, event_type, description, lat, lon, altitude_ft, ts),
        )
        self._maybe_commit()

    def get_events(
        self,
        event_type: str | None = None,
        icao: str | None = None,
        limit: int = 100,
    ) -> list[dict]:
        clauses = []
        params: list = []
        if event_type:
            clauses.append("event_type = ?")
            params.append(event_type)
        if icao:
            clauses.append("icao = ?")
            params.append(icao)
        where = " AND ".join(clauses) if clauses else "1=1"
        params.append(limit)
        rows = self.conn.execute(
            f"SELECT * FROM events WHERE {where} ORDER BY timestamp DESC LIMIT ?",
            params,
        ).fetchall()
        return [dict(r) for r in rows]

    def count_events(self) -> int:
        row = self.conn.execute("SELECT COUNT(*) as cnt FROM events").fetchone()
        return row["cnt"]

    # --- Sightings ---

    def upsert_sighting(
        self,
        icao: str,
        capture_id: int | None = None,
        callsign: str | None = None,
        squawk: str | None = None,
        altitude_ft: int | None = None,
        timestamp: float | None = None,
    ):
        """Update or create a sighting for the current capture session."""
        ts = timestamp or time.time()
        # Try to find existing sighting for this icao + capture
        row = self.conn.execute(
            "SELECT id, min_altitude_ft, max_altitude_ft, message_count FROM sightings WHERE icao = ? AND capture_id IS ?",
            (icao, capture_id),
        ).fetchone()

        if row:
            min_alt = row["min_altitude_ft"]
            max_alt = row["max_altitude_ft"]
            if altitude_ft is not None:
                min_alt = min(min_alt, altitude_ft) if min_alt is not None else altitude_ft
                max_alt = max(max_alt, altitude_ft) if max_alt is not None else altitude_ft
            self.conn.execute(
                """UPDATE sightings SET
                       callsign = COALESCE(?, callsign),
                       squawk = COALESCE(?, squawk),
                       min_altitude_ft = ?,
                       max_altitude_ft = ?,
                       message_count = message_count + 1,
                       last_seen = ?
                   WHERE id = ?""",
                (callsign, squawk, min_alt, max_alt, ts, row["id"]),
            )
        else:
            self.conn.execute(
                """INSERT INTO sightings
                   (icao, capture_id, callsign, squawk, min_altitude_ft, max_altitude_ft, message_count, first_seen, last_seen)
                   VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)""",
                (icao, capture_id, callsign, squawk, altitude_ft, altitude_ft, ts, ts),
            )
        self._maybe_commit()

    # --- Maintenance ---

    def prune_positions(self, max_age_hours: int = 168) -> int:
        """Delete positions older than max_age_hours (default 7 days).

        Returns the number of rows deleted.
        """
        cutoff = time.time() - (max_age_hours * 3600)
        cur = self.conn.execute(
            "DELETE FROM positions WHERE timestamp < ?", (cutoff,)
        )
        self.conn.commit()
        return cur.rowcount

    def prune_events(self, max_age_hours: int = 720) -> int:
        """Delete events older than max_age_hours (default 30 days).

        Returns the number of rows deleted.
        """
        cutoff = time.time() - (max_age_hours * 3600)
        cur = self.conn.execute(
            "DELETE FROM events WHERE timestamp < ?", (cutoff,)
        )
        self.conn.commit()
        return cur.rowcount

    # --- Stats ---

    def stats(self) -> dict:
        """Return summary statistics."""
        return {
            "aircraft": self.count_aircraft(),
            "positions": self.count_positions(),
            "events": self.count_events(),
            "receivers": self.conn.execute("SELECT COUNT(*) as cnt FROM receivers").fetchone()["cnt"],
            "captures": self.conn.execute("SELECT COUNT(*) as cnt FROM captures").fetchone()["cnt"],
        }
