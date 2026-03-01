"""Tests for SQLite database persistence."""

import time

import pytest

from src.database import Database


@pytest.fixture
def db(tmp_path):
    """Fresh database for each test."""
    d = Database(tmp_path / "test.db")
    _ = d.conn  # Initialize schema
    yield d
    d.close()


@pytest.fixture
def mem_db():
    """In-memory database."""
    d = Database(":memory:")
    _ = d.conn
    yield d
    d.close()


class TestReceivers:
    def test_add_receiver(self, db):
        rid = db.add_receiver("home", lat=35.18, lon=-83.38, altitude_ft=2100)
        assert rid > 0

    def test_add_duplicate_returns_existing_id(self, db):
        id1 = db.add_receiver("home")
        id2 = db.add_receiver("home")
        assert id1 == id2

    def test_get_receiver(self, db):
        db.add_receiver("home", lat=35.18, lon=-83.38)
        r = db.get_receiver("home")
        assert r is not None
        assert r["name"] == "home"
        assert r["lat"] == pytest.approx(35.18)

    def test_get_nonexistent(self, db):
        assert db.get_receiver("nope") is None


class TestAircraft:
    def test_upsert_new(self, db):
        db.upsert_aircraft("A00001", country="United States", timestamp=1000.0)
        ac = db.get_aircraft("A00001")
        assert ac is not None
        assert ac["country"] == "United States"

    def test_upsert_updates_last_seen(self, db):
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.upsert_aircraft("A00001", timestamp=2000.0)
        ac = db.get_aircraft("A00001")
        assert ac["last_seen"] == 2000.0

    def test_upsert_preserves_country(self, db):
        db.upsert_aircraft("A00001", country="United States", timestamp=1000.0)
        db.upsert_aircraft("A00001", timestamp=2000.0)  # No country this time
        ac = db.get_aircraft("A00001")
        assert ac["country"] == "United States"

    def test_military_flag_sticky(self, db):
        db.upsert_aircraft("ADF7C8", is_military=True, timestamp=1000.0)
        db.upsert_aircraft("ADF7C8", is_military=False, timestamp=2000.0)
        ac = db.get_aircraft("ADF7C8")
        assert ac["is_military"] == 1  # Once military, always military

    def test_count(self, db):
        assert db.count_aircraft() == 0
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.upsert_aircraft("A00002", timestamp=1000.0)
        assert db.count_aircraft() == 2


class TestPositions:
    def test_add_and_retrieve(self, db):
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.add_position("A00001", lat=35.18, lon=-83.38, altitude_ft=38000, timestamp=1000.0)
        positions = db.get_positions("A00001")
        assert len(positions) == 1
        assert positions[0]["lat"] == pytest.approx(35.18)
        assert positions[0]["altitude_ft"] == 38000

    def test_multiple_positions(self, db):
        db.upsert_aircraft("A00001", timestamp=1000.0)
        for i in range(5):
            db.add_position("A00001", lat=35.0 + i * 0.01, lon=-83.0, timestamp=1000.0 + i)
        positions = db.get_positions("A00001")
        assert len(positions) == 5

    def test_positions_ordered_by_time_desc(self, db):
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.add_position("A00001", lat=35.0, lon=-83.0, timestamp=1000.0)
        db.add_position("A00001", lat=35.1, lon=-83.0, timestamp=2000.0)
        positions = db.get_positions("A00001")
        assert positions[0]["timestamp"] > positions[1]["timestamp"]

    def test_count(self, db):
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.add_position("A00001", lat=35.0, lon=-83.0, timestamp=1000.0)
        assert db.count_positions() == 1

    def test_receiver_id_tagged(self, db):
        rid = db.add_receiver("home")
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.add_position("A00001", lat=35.0, lon=-83.0, receiver_id=rid, timestamp=1000.0)
        pos = db.get_positions("A00001")[0]
        assert pos["receiver_id"] == rid


class TestCaptures:
    def test_start_and_end(self, db):
        cap_id = db.start_capture(source="test.bin")
        assert cap_id > 0
        db.end_capture(cap_id, total_frames=1000, valid_frames=800, aircraft_count=25)

    def test_receiver_tagged(self, db):
        rid = db.add_receiver("home")
        cap_id = db.start_capture(source="test.bin", receiver_id=rid)
        assert cap_id > 0


class TestEvents:
    def test_add_event(self, db):
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.add_event("A00001", "emergency_squawk", "Squawk 7700", timestamp=1000.0)
        events = db.get_events()
        assert len(events) == 1
        assert events[0]["event_type"] == "emergency_squawk"

    def test_filter_by_type(self, db):
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.add_event("A00001", "emergency_squawk", timestamp=1000.0)
        db.add_event("A00001", "military_detected", timestamp=1001.0)
        assert len(db.get_events("emergency_squawk")) == 1
        assert len(db.get_events("military_detected")) == 1

    def test_count(self, db):
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.add_event("A00001", "test", timestamp=1000.0)
        assert db.count_events() == 1


class TestSightings:
    def test_upsert_creates(self, db):
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.upsert_sighting("A00001", callsign="DAL123", timestamp=1000.0)

    def test_upsert_increments_count(self, db):
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.upsert_sighting("A00001", callsign="DAL123", timestamp=1000.0)
        db.upsert_sighting("A00001", timestamp=1001.0)
        row = db.conn.execute(
            "SELECT message_count FROM sightings WHERE icao = ?", ("A00001",)
        ).fetchone()
        assert row["message_count"] == 2

    def test_altitude_tracking(self, db):
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.upsert_sighting("A00001", altitude_ft=30000, timestamp=1000.0)
        db.upsert_sighting("A00001", altitude_ft=35000, timestamp=1001.0)
        db.upsert_sighting("A00001", altitude_ft=28000, timestamp=1002.0)
        row = db.conn.execute(
            "SELECT min_altitude_ft, max_altitude_ft FROM sightings WHERE icao = ?", ("A00001",)
        ).fetchone()
        assert row["min_altitude_ft"] == 28000
        assert row["max_altitude_ft"] == 35000


class TestStats:
    def test_empty_stats(self, db):
        s = db.stats()
        assert s["aircraft"] == 0
        assert s["positions"] == 0

    def test_stats_after_data(self, db):
        db.add_receiver("home")
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.add_position("A00001", lat=35.0, lon=-83.0, timestamp=1000.0)
        s = db.stats()
        assert s["aircraft"] == 1
        assert s["positions"] == 1
        assert s["receivers"] == 1


class TestDownsampling:
    def test_downsample_keeps_one_per_bucket(self, db):
        """Positions within the same time bucket are thinned to one."""
        db.upsert_aircraft("A00001", timestamp=1000.0)
        now = time.time()
        # Use a round base to avoid straddling a 30s bucket boundary
        old = (int((now - 100000) / 30) * 30) + 1  # Align to start of a 30s bucket
        # 10 positions within the same 30s bucket (span 9 seconds)
        for i in range(10):
            db.add_position("A00001", lat=35.0, lon=-83.0, altitude_ft=30000 + i,
                           timestamp=old + i)
        assert db.count_positions() == 10
        removed = db.downsample_positions(older_than_hours=24, keep_interval_sec=30)
        assert removed == 9  # Keep 1 per bucket
        assert db.count_positions() == 1

    def test_downsample_preserves_recent(self, db):
        """Positions newer than the cutoff are untouched."""
        db.upsert_aircraft("A00001", timestamp=1000.0)
        now = time.time()
        # Add recent positions (< 24h old)
        for i in range(5):
            db.add_position("A00001", lat=35.0, lon=-83.0, timestamp=now - i)
        removed = db.downsample_positions(older_than_hours=24, keep_interval_sec=30)
        assert removed == 0
        assert db.count_positions() == 5

    def test_downsample_multiple_aircraft(self, db):
        """Each aircraft is downsampled independently."""
        now = time.time()
        old = (int((now - 100000) / 30) * 30) + 1  # Align to bucket
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.upsert_aircraft("A00002", timestamp=1000.0)
        for i in range(10):
            db.add_position("A00001", lat=35.0, lon=-83.0, timestamp=old + i)
            db.add_position("A00002", lat=36.0, lon=-84.0, timestamp=old + i)
        assert db.count_positions() == 20
        db.downsample_positions(older_than_hours=24, keep_interval_sec=30)
        assert db.count_positions() == 2  # 1 per aircraft

    def test_downsample_separate_buckets_preserved(self, db):
        """Positions in different time buckets are each kept."""
        db.upsert_aircraft("A00001", timestamp=1000.0)
        now = time.time()
        old = now - 100000
        # 3 positions in 3 separate 30s buckets
        db.add_position("A00001", lat=35.0, lon=-83.0, timestamp=old)
        db.add_position("A00001", lat=35.0, lon=-83.0, timestamp=old + 35)
        db.add_position("A00001", lat=35.0, lon=-83.0, timestamp=old + 70)
        db.downsample_positions(older_than_hours=24, keep_interval_sec=30)
        assert db.count_positions() == 3  # Each in its own bucket

    def test_vacuum(self, db):
        """VACUUM runs without error."""
        db.upsert_aircraft("A00001", timestamp=1000.0)
        db.add_position("A00001", lat=35.0, lon=-83.0, timestamp=1000.0)
        db.vacuum()  # Should not raise


class TestWALMode:
    def test_wal_mode_enabled(self, db):
        mode = db.conn.execute("PRAGMA journal_mode").fetchone()[0]
        assert mode == "wal"

    def test_foreign_keys_enabled(self, db):
        fk = db.conn.execute("PRAGMA foreign_keys").fetchone()[0]
        assert fk == 1
