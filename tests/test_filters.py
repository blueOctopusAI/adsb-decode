"""Tests for intelligence filters — military, emergency, anomaly, geofence, circling, proximity."""

import pytest

from src.filters import (
    FilterEngine,
    Geofence,
    Event,
    EVENT_MILITARY,
    EVENT_EMERGENCY,
    EVENT_RAPID_DESCENT,
    EVENT_LOW_ALTITUDE,
    EVENT_GEOFENCE,
    EVENT_CIRCLING,
    EVENT_PROXIMITY,
    EMERGENCY_SQUAWKS,
    _haversine_nm,
)
from src.tracker import AircraftState


@pytest.fixture
def engine():
    return FilterEngine()


def _make_ac(**kwargs) -> AircraftState:
    defaults = dict(icao="A00001", last_seen=1000.0)
    defaults.update(kwargs)
    return AircraftState(**defaults)


class TestMilitaryFilter:
    def test_detects_military(self, engine):
        ac = _make_ac(is_military=True, callsign="RCH123")
        events = engine.check(ac)
        assert any(e.event_type == EVENT_MILITARY for e in events)

    def test_ignores_civilian(self, engine):
        ac = _make_ac(is_military=False)
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_MILITARY for e in events)

    def test_no_duplicate(self, engine):
        ac = _make_ac(is_military=True)
        events1 = engine.check(ac)
        events2 = engine.check(ac)
        assert len([e for e in events1 if e.event_type == EVENT_MILITARY]) == 1
        assert len([e for e in events2 if e.event_type == EVENT_MILITARY]) == 0

    def test_clear_allows_re_emit(self, engine):
        ac = _make_ac(is_military=True)
        engine.check(ac)
        engine.clear(ac.icao)
        events = engine.check(ac)
        assert any(e.event_type == EVENT_MILITARY for e in events)


class TestEmergencyFilter:
    def test_squawk_7700(self, engine):
        ac = _make_ac(squawk="7700")
        events = engine.check(ac)
        assert any(e.event_type == EVENT_EMERGENCY for e in events)

    def test_squawk_7500(self, engine):
        ac = _make_ac(squawk="7500")
        events = engine.check(ac)
        assert any(e.event_type == EVENT_EMERGENCY for e in events)
        assert "Hijack" in events[0].description

    def test_squawk_7600(self, engine):
        ac = _make_ac(squawk="7600")
        events = engine.check(ac)
        assert any(e.event_type == EVENT_EMERGENCY for e in events)

    def test_normal_squawk_ignored(self, engine):
        ac = _make_ac(squawk="1200")
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_EMERGENCY for e in events)

    def test_no_squawk_ignored(self, engine):
        ac = _make_ac(squawk=None)
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_EMERGENCY for e in events)


class TestRapidDescentFilter:
    def test_rapid_descent_detected(self, engine):
        ac = _make_ac(vertical_rate_fpm=-6000, altitude_ft=10000)
        events = engine.check(ac)
        assert any(e.event_type == EVENT_RAPID_DESCENT for e in events)

    def test_normal_descent_ok(self, engine):
        ac = _make_ac(vertical_rate_fpm=-2000)
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_RAPID_DESCENT for e in events)

    def test_climb_ok(self, engine):
        ac = _make_ac(vertical_rate_fpm=3000)
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_RAPID_DESCENT for e in events)

    def test_no_vrate_ignored(self, engine):
        ac = _make_ac(vertical_rate_fpm=None)
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_RAPID_DESCENT for e in events)

    def test_custom_threshold(self):
        engine = FilterEngine(rapid_descent_fpm=-3000)
        ac = _make_ac(vertical_rate_fpm=-3500)
        events = engine.check(ac)
        assert any(e.event_type == EVENT_RAPID_DESCENT for e in events)


class TestLowAltitudeFilter:
    def test_low_altitude_detected(self, engine):
        ac = _make_ac(altitude_ft=200)
        events = engine.check(ac)
        assert any(e.event_type == EVENT_LOW_ALTITUDE for e in events)

    def test_normal_altitude_ok(self, engine):
        ac = _make_ac(altitude_ft=5000)
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_LOW_ALTITUDE for e in events)

    def test_on_ground_ignored(self, engine):
        ac = _make_ac(altitude_ft=0)
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_LOW_ALTITUDE for e in events)

    def test_no_altitude_ignored(self, engine):
        ac = _make_ac(altitude_ft=None)
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_LOW_ALTITUDE for e in events)


class TestGeofenceFilter:
    def test_inside_fence(self):
        fence = Geofence(name="TestZone", lat=35.0, lon=-83.0, radius_nm=10)
        engine = FilterEngine(geofences=[fence])
        ac = _make_ac(lat=35.01, lon=-83.01)
        events = engine.check(ac)
        assert any(e.event_type == EVENT_GEOFENCE for e in events)

    def test_outside_fence(self):
        fence = Geofence(name="TestZone", lat=35.0, lon=-83.0, radius_nm=1)
        engine = FilterEngine(geofences=[fence])
        ac = _make_ac(lat=36.0, lon=-84.0)  # ~85nm away
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_GEOFENCE for e in events)

    def test_no_position_ignored(self):
        fence = Geofence(name="TestZone", lat=35.0, lon=-83.0, radius_nm=10)
        engine = FilterEngine(geofences=[fence])
        ac = _make_ac(lat=None, lon=None)
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_GEOFENCE for e in events)

    def test_multiple_fences(self):
        fences = [
            Geofence(name="Zone1", lat=35.0, lon=-83.0, radius_nm=10),
            Geofence(name="Zone2", lat=35.0, lon=-83.0, radius_nm=5),
        ]
        engine = FilterEngine(geofences=fences)
        ac = _make_ac(lat=35.01, lon=-83.01)
        events = engine.check(ac)
        geo_events = [e for e in events if e.event_type == EVENT_GEOFENCE]
        assert len(geo_events) == 2


class TestHaversine:
    def test_same_point(self):
        assert _haversine_nm(35.0, -83.0, 35.0, -83.0) == pytest.approx(0.0, abs=0.01)

    def test_known_distance(self):
        # Roughly 60nm = 1 degree of latitude
        dist = _haversine_nm(35.0, -83.0, 36.0, -83.0)
        assert 59.0 < dist < 61.0

    def test_symmetric(self):
        d1 = _haversine_nm(35.0, -83.0, 36.0, -84.0)
        d2 = _haversine_nm(36.0, -84.0, 35.0, -83.0)
        assert d1 == pytest.approx(d2, abs=0.001)


class TestMultipleFilters:
    def test_military_and_emergency(self, engine):
        """Military aircraft with emergency squawk triggers both filters."""
        ac = _make_ac(is_military=True, squawk="7700")
        events = engine.check(ac)
        types = {e.event_type for e in events}
        assert EVENT_MILITARY in types
        assert EVENT_EMERGENCY in types

    def test_event_has_position(self, engine):
        ac = _make_ac(is_military=True, lat=35.0, lon=-83.0, altitude_ft=10000)
        events = engine.check(ac)
        event = events[0]
        assert event.lat == 35.0
        assert event.lon == -83.0
        assert event.altitude_ft == 10000


class TestCirclingFilter:
    def test_full_circle_detected(self, engine):
        """Aircraft completing 360+ degrees of heading change triggers circling."""
        ac = _make_ac(lat=35.0, lon=-83.0, altitude_ft=5000)
        # Simulate 20 heading reports over 5 minutes, turning steadily
        base_t = 1000.0
        for i in range(20):
            ac.heading_history.append((base_t + i * 15, (i * 20) % 360))
        ac.last_seen = base_t + 19 * 15
        events = engine.check(ac)
        assert any(e.event_type == EVENT_CIRCLING for e in events)

    def test_straight_flight_not_detected(self, engine):
        """Steady heading should not trigger circling."""
        ac = _make_ac(lat=35.0, lon=-83.0)
        base_t = 1000.0
        for i in range(20):
            ac.heading_history.append((base_t + i * 15, 90))  # Constant heading
        ac.last_seen = base_t + 19 * 15
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_CIRCLING for e in events)

    def test_partial_turn_not_detected(self, engine):
        """180 degrees of heading change should not trigger."""
        ac = _make_ac(lat=35.0, lon=-83.0)
        base_t = 1000.0
        for i in range(10):
            ac.heading_history.append((base_t + i * 15, i * 18))  # 162 degrees total
        ac.last_seen = base_t + 9 * 15
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_CIRCLING for e in events)

    def test_too_few_points_ignored(self, engine):
        ac = _make_ac(lat=35.0, lon=-83.0)
        ac.heading_history = [(1000.0, 0), (1001.0, 180)]
        ac.last_seen = 1001.0
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_CIRCLING for e in events)

    def test_old_history_excluded(self, engine):
        """Heading changes older than 5 minutes should not count."""
        ac = _make_ac(lat=35.0, lon=-83.0)
        # Old data: lots of turning, but >5 min ago
        for i in range(20):
            ac.heading_history.append((100.0 + i * 15, (i * 20) % 360))
        # Recent data: straight flight
        for i in range(10):
            ac.heading_history.append((1000.0 + i * 15, 90))
        ac.last_seen = 1000.0 + 9 * 15
        events = engine.check(ac)
        assert not any(e.event_type == EVENT_CIRCLING for e in events)

    def test_no_duplicate(self, engine):
        ac = _make_ac(lat=35.0, lon=-83.0)
        base_t = 1000.0
        for i in range(20):
            ac.heading_history.append((base_t + i * 15, (i * 20) % 360))
        ac.last_seen = base_t + 19 * 15
        events1 = engine.check(ac)
        events2 = engine.check(ac)
        assert len([e for e in events1 if e.event_type == EVENT_CIRCLING]) == 1
        assert len([e for e in events2 if e.event_type == EVENT_CIRCLING]) == 0

    def test_wraparound_heading(self, engine):
        """Heading changes across 360/0 boundary should work correctly."""
        ac = _make_ac(lat=35.0, lon=-83.0)
        base_t = 1000.0
        # 350, 0, 10, 20, ... wrapping around
        headings = [350, 0, 10, 20, 30, 40, 50, 60, 70, 80,
                    90, 100, 110, 120, 130, 140, 150, 160, 170, 180,
                    190, 200]
        for i, h in enumerate(headings):
            ac.heading_history.append((base_t + i * 15, h))
        ac.last_seen = base_t + (len(headings) - 1) * 15
        events = engine.check(ac)
        # Should not count the 350->0 wrap as 350 degrees — it's 10 degrees
        # Total change: ~210 degrees, not enough for circling
        assert not any(e.event_type == EVENT_CIRCLING for e in events)


class TestProximityFilter:
    def test_close_aircraft_detected(self):
        engine = FilterEngine(proximity_nm=5.0, proximity_ft=1000)
        a = _make_ac(icao="AAA001", lat=35.0, lon=-83.0, altitude_ft=10000)
        b = _make_ac(icao="BBB002", lat=35.01, lon=-83.01, altitude_ft=10200)
        events = engine.check_proximity([a, b])
        assert len(events) == 1
        assert events[0].event_type == EVENT_PROXIMITY

    def test_far_aircraft_ignored(self):
        engine = FilterEngine(proximity_nm=5.0, proximity_ft=1000)
        a = _make_ac(icao="AAA001", lat=35.0, lon=-83.0, altitude_ft=10000)
        b = _make_ac(icao="BBB002", lat=36.0, lon=-84.0, altitude_ft=10000)
        events = engine.check_proximity([a, b])
        assert len(events) == 0

    def test_vertical_separation_ok(self):
        """Close horizontally but >1000ft vertical separation — no alert."""
        engine = FilterEngine(proximity_nm=5.0, proximity_ft=1000)
        a = _make_ac(icao="AAA001", lat=35.0, lon=-83.0, altitude_ft=10000)
        b = _make_ac(icao="BBB002", lat=35.01, lon=-83.01, altitude_ft=12000)
        events = engine.check_proximity([a, b])
        assert len(events) == 0

    def test_no_duplicate_pair(self):
        engine = FilterEngine(proximity_nm=5.0, proximity_ft=1000)
        a = _make_ac(icao="AAA001", lat=35.0, lon=-83.0, altitude_ft=10000)
        b = _make_ac(icao="BBB002", lat=35.01, lon=-83.01, altitude_ft=10200)
        events1 = engine.check_proximity([a, b])
        events2 = engine.check_proximity([a, b])
        assert len(events1) == 1
        assert len(events2) == 0

    def test_no_position_skipped(self):
        engine = FilterEngine(proximity_nm=5.0, proximity_ft=1000)
        a = _make_ac(icao="AAA001", lat=None, lon=None)
        b = _make_ac(icao="BBB002", lat=35.0, lon=-83.0, altitude_ft=10000)
        events = engine.check_proximity([a, b])
        assert len(events) == 0

    def test_three_aircraft_multiple_alerts(self):
        engine = FilterEngine(proximity_nm=100.0, proximity_ft=50000)
        a = _make_ac(icao="AAA001", lat=35.0, lon=-83.0, altitude_ft=10000)
        b = _make_ac(icao="BBB002", lat=35.01, lon=-83.01, altitude_ft=10200)
        c = _make_ac(icao="CCC003", lat=35.02, lon=-83.02, altitude_ft=10400)
        events = engine.check_proximity([a, b, c])
        # Should detect 3 pairs: AB, AC, BC
        assert len(events) == 3
