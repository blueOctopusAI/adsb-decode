"""Tests for intelligence filters — military, emergency, anomaly, geofence."""

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
