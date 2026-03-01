"""Tests for aircraft tracker — state machine + CPR pairing + DB integration."""

import time
import pytest

from src.tracker import Tracker, AircraftState, STALE_TIMEOUT
from src.database import Database
from src.frame_parser import parse_frame
from src.decoder import IdentificationMsg, PositionMsg, VelocityMsg
from tests.fixtures.known_frames import (
    IDENTIFICATION_FRAMES,
    POSITION_FRAMES,
    VELOCITY_FRAMES,
    POSITION_DECODED,
)


@pytest.fixture
def tracker():
    """Tracker without database."""
    return Tracker()


@pytest.fixture
def db_tracker(tmp_path):
    """Tracker with database persistence."""
    db = Database(tmp_path / "track.db")
    rid = db.add_receiver("test-rx", lat=52.0, lon=4.0)
    cap_id = db.start_capture(source="test", receiver_id=rid)
    t = Tracker(db=db, receiver_id=rid, capture_id=cap_id, ref_lat=52.0, ref_lon=4.0)
    yield t
    db.close()


class TestTrackerBasic:
    """Basic frame processing."""

    def test_identification_updates_callsign(self, tracker):
        hex_str = IDENTIFICATION_FRAMES[0][0]
        frame = parse_frame(hex_str, timestamp=1000.0)
        msg = tracker.update(frame)
        assert isinstance(msg, IdentificationMsg)

        ac = tracker.aircraft[IDENTIFICATION_FRAMES[0][1]]
        assert ac.callsign == "KLM1023"

    def test_velocity_updates_speed(self, tracker):
        hex_str = VELOCITY_FRAMES[0][0]
        frame = parse_frame(hex_str, timestamp=1000.0)
        msg = tracker.update(frame)
        assert isinstance(msg, VelocityMsg)

        ac = tracker.aircraft[VELOCITY_FRAMES[0][1]]
        assert ac.speed_kts is not None
        assert ac.heading_deg is not None
        assert ac.vertical_rate_fpm is not None

    def test_position_stores_cpr(self, tracker):
        # Even frame
        hex_even = POSITION_FRAMES[0][0]
        frame = parse_frame(hex_even, timestamp=1000.0)
        tracker.update(frame)

        icao = POSITION_FRAMES[0][1]
        ac = tracker.aircraft[icao]
        assert ac.cpr_even_lat is not None
        assert ac.cpr_even_lon is not None

    def test_corrupted_frame_rejected(self, tracker):
        frame = parse_frame("8D4840D6202CC371C32CE0576099", timestamp=1000.0)
        msg = tracker.update(frame)
        assert msg is None
        assert tracker.valid_frames == 0

    def test_frame_counters(self, tracker):
        frame = parse_frame(IDENTIFICATION_FRAMES[0][0], timestamp=1000.0)
        tracker.update(frame)
        assert tracker.total_frames == 1
        assert tracker.valid_frames == 1


class TestCPRPairing:
    """CPR even/odd frame pairing for position decode."""

    def test_global_decode_from_pair(self, tracker):
        """Feed even then odd frame — should resolve position."""
        hex_even = POSITION_FRAMES[0][0]
        hex_odd = POSITION_FRAMES[1][0]
        icao = POSITION_FRAMES[0][1]

        tracker.update(parse_frame(hex_even, timestamp=1000.0))
        tracker.update(parse_frame(hex_odd, timestamp=1000.5))

        ac = tracker.aircraft[icao]
        assert ac.has_position
        assert abs(ac.lat - POSITION_DECODED["lat"]) < 0.1
        assert abs(ac.lon - POSITION_DECODED["lon"]) < 0.1

    def test_odd_then_even_also_works(self, tracker):
        """Feed odd then even — should also resolve."""
        hex_even = POSITION_FRAMES[0][0]
        hex_odd = POSITION_FRAMES[1][0]
        icao = POSITION_FRAMES[0][1]

        tracker.update(parse_frame(hex_odd, timestamp=1000.0))
        tracker.update(parse_frame(hex_even, timestamp=1000.5))

        ac = tracker.aircraft[icao]
        assert ac.has_position

    def test_single_frame_no_position_without_reference(self):
        """A single CPR frame without reference can't resolve position."""
        tracker = Tracker()  # No ref_lat/ref_lon
        hex_even = POSITION_FRAMES[0][0]
        tracker.update(parse_frame(hex_even, timestamp=1000.0))

        ac = tracker.aircraft[POSITION_FRAMES[0][1]]
        assert not ac.has_position

    def test_single_frame_resolves_with_reference(self):
        """A single CPR frame with receiver reference should resolve via local decode."""
        tracker = Tracker(ref_lat=52.0, ref_lon=4.0)
        hex_even = POSITION_FRAMES[0][0]
        tracker.update(parse_frame(hex_even, timestamp=1000.0))

        ac = tracker.aircraft[POSITION_FRAMES[0][1]]
        assert ac.has_position
        assert abs(ac.lat - POSITION_DECODED["lat"]) < 1.0  # Wider tolerance for local decode

    def test_position_decode_counter(self, tracker):
        hex_even = POSITION_FRAMES[0][0]
        hex_odd = POSITION_FRAMES[1][0]
        tracker.update(parse_frame(hex_even, timestamp=1000.0))
        tracker.update(parse_frame(hex_odd, timestamp=1000.5))
        assert tracker.position_decodes >= 1


class TestAircraftState:
    """AircraftState properties."""

    def test_has_position(self):
        ac = AircraftState(icao="A00001")
        assert not ac.has_position
        ac.lat = 35.0
        ac.lon = -83.0
        assert ac.has_position

    def test_staleness(self):
        ac = AircraftState(icao="A00001", last_seen=time.time())
        assert not ac.is_stale
        ac.last_seen = time.time() - STALE_TIMEOUT - 1
        assert ac.is_stale

    def test_age(self):
        ac = AircraftState(icao="A00001", last_seen=time.time() - 30)
        assert 29 < ac.age < 31


class TestTrackerManagement:
    """Active aircraft list and pruning."""

    def test_get_active(self, tracker):
        frame = parse_frame(IDENTIFICATION_FRAMES[0][0], timestamp=time.time())
        tracker.update(frame)
        active = tracker.get_active()
        assert len(active) == 1

    def test_prune_stale(self, tracker):
        frame = parse_frame(IDENTIFICATION_FRAMES[0][0], timestamp=1000.0)
        tracker.update(frame)
        # Make it stale
        icao = IDENTIFICATION_FRAMES[0][1]
        tracker.aircraft[icao].last_seen = time.time() - STALE_TIMEOUT - 10
        removed = tracker.prune_stale()
        assert removed == 1
        assert len(tracker.aircraft) == 0


class TestDatabaseIntegration:
    """Tracker with database persistence."""

    def test_aircraft_persisted(self, db_tracker):
        frame = parse_frame(IDENTIFICATION_FRAMES[0][0], timestamp=1000.0)
        db_tracker.update(frame)
        ac = db_tracker.db.get_aircraft(IDENTIFICATION_FRAMES[0][1])
        assert ac is not None

    def test_positions_persisted(self, db_tracker):
        hex_even = POSITION_FRAMES[0][0]
        hex_odd = POSITION_FRAMES[1][0]
        db_tracker.update(parse_frame(hex_even, timestamp=1000.0))
        db_tracker.update(parse_frame(hex_odd, timestamp=1000.5))

        positions = db_tracker.db.get_positions(POSITION_FRAMES[0][1])
        assert len(positions) >= 1

    def test_country_resolved(self, db_tracker):
        frame = parse_frame(IDENTIFICATION_FRAMES[0][0], timestamp=1000.0)
        db_tracker.update(frame)
        icao = IDENTIFICATION_FRAMES[0][1]
        ac = db_tracker.aircraft[icao]
        assert ac.country is not None  # ICAO lookup should resolve country

    def test_sighting_created(self, db_tracker):
        frame = parse_frame(IDENTIFICATION_FRAMES[0][0], timestamp=1000.0)
        db_tracker.update(frame)
        icao = IDENTIFICATION_FRAMES[0][1]
        row = db_tracker.db.conn.execute(
            "SELECT * FROM sightings WHERE icao = ?", (icao,)
        ).fetchone()
        assert row is not None


class TestIngestDownsampling:
    """Position downsampling at ingest time."""

    def test_skips_position_within_interval(self, tmp_path):
        """Positions within min_position_interval are not stored in DB."""
        db = Database(tmp_path / "ds.db")
        rid = db.add_receiver("test-rx", lat=52.0, lon=4.0)
        cap_id = db.start_capture(source="test", receiver_id=rid)
        t = Tracker(db=db, receiver_id=rid, capture_id=cap_id,
                    ref_lat=52.0, ref_lon=4.0, min_position_interval=2.0)

        hex_even = POSITION_FRAMES[0][0]
        hex_odd = POSITION_FRAMES[1][0]

        # First pair — should store
        t.update(parse_frame(hex_even, timestamp=1000.0))
        t.update(parse_frame(hex_odd, timestamp=1000.5))

        # Second pair 1s later — should skip (within 2s interval)
        t.update(parse_frame(hex_even, timestamp=1001.0))
        t.update(parse_frame(hex_odd, timestamp=1001.5))

        # Third pair 3s after first — should store (past 2s interval)
        t.update(parse_frame(hex_even, timestamp=1003.0))
        t.update(parse_frame(hex_odd, timestamp=1003.5))

        icao = POSITION_FRAMES[0][1]
        positions = db.get_positions(icao)
        assert len(positions) == 2  # Stored at 1000.5 and 1006.5
        assert t.positions_skipped >= 1
        db.close()

    def test_zero_interval_stores_all(self, tmp_path):
        """min_position_interval=0 stores every position (old behavior)."""
        db = Database(tmp_path / "ds0.db")
        rid = db.add_receiver("test-rx", lat=52.0, lon=4.0)
        cap_id = db.start_capture(source="test", receiver_id=rid)
        t = Tracker(db=db, receiver_id=rid, capture_id=cap_id,
                    ref_lat=52.0, ref_lon=4.0, min_position_interval=0)

        hex_even = POSITION_FRAMES[0][0]
        hex_odd = POSITION_FRAMES[1][0]

        t.update(parse_frame(hex_even, timestamp=1000.0))
        t.update(parse_frame(hex_odd, timestamp=1000.5))
        t.update(parse_frame(hex_even, timestamp=1001.0))
        t.update(parse_frame(hex_odd, timestamp=1001.5))

        icao = POSITION_FRAMES[0][1]
        positions = db.get_positions(icao)
        assert len(positions) >= 2
        db.close()

    def test_pattern_detection_not_affected(self):
        """Position history buffer is filled even when DB write is skipped."""
        t = Tracker(ref_lat=52.0, ref_lon=4.0, min_position_interval=2.0)

        hex_even = POSITION_FRAMES[0][0]
        hex_odd = POSITION_FRAMES[1][0]

        t.update(parse_frame(hex_even, timestamp=1000.0))
        t.update(parse_frame(hex_odd, timestamp=1000.5))
        t.update(parse_frame(hex_even, timestamp=1001.0))
        t.update(parse_frame(hex_odd, timestamp=1001.5))

        icao = POSITION_FRAMES[0][1]
        ac = t.aircraft[icao]
        # Position history should have entries regardless of DB downsampling
        assert len(ac.position_history) >= 2


class TestMultiAircraft:
    """Tracking multiple aircraft simultaneously."""

    def test_separate_state(self, tracker):
        """Each aircraft should have independent state."""
        frame1 = parse_frame(IDENTIFICATION_FRAMES[0][0], timestamp=1000.0)
        frame2 = parse_frame(VELOCITY_FRAMES[0][0], timestamp=1000.0)
        tracker.update(frame1)
        tracker.update(frame2)

        icao1 = IDENTIFICATION_FRAMES[0][1]
        icao2 = VELOCITY_FRAMES[0][1]

        # Should be different aircraft (different ICAOs)
        if icao1 != icao2:
            assert len(tracker.aircraft) == 2
            assert tracker.aircraft[icao1].callsign is not None
            assert tracker.aircraft[icao2].speed_kts is not None
