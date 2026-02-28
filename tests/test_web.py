"""Tests for web dashboard — Flask routes and API endpoints."""

import json

import pytest

from src.database import Database
from src.web.app import create_app


@pytest.fixture
def app(tmp_path):
    """Flask test app with sample data."""
    db_path = str(tmp_path / "web.db")
    db = Database(db_path)
    db.upsert_aircraft("A00001", country="United States", registration="N12345", timestamp=1000.0)
    db.upsert_aircraft("ADF7C8", country="United States", is_military=True, timestamp=1000.0)
    db.add_position("A00001", lat=35.18, lon=-83.38, altitude_ft=38000,
                   speed_kts=450.0, heading_deg=90.0, timestamp=1000.0)
    db.add_position("A00001", lat=35.20, lon=-83.30, altitude_ft=37500, timestamp=1001.0)
    db.add_event("A00001", "military_detected", "Test event", timestamp=1000.0)
    db.add_receiver("test-rx")
    db.start_capture(source="test")
    db.close()

    app = create_app(db_path=db_path)
    app.config["TESTING"] = True
    return app


@pytest.fixture
def client(app):
    return app.test_client()


class TestAPIAircraft:
    def test_list_aircraft(self, client):
        resp = client.get("/api/aircraft")
        assert resp.status_code == 200
        data = resp.get_json()
        assert "aircraft" in data
        assert data["count"] == 2

    def test_military_filter(self, client):
        resp = client.get("/api/aircraft?military=true")
        data = resp.get_json()
        assert data["count"] == 1
        assert data["aircraft"][0]["icao"] == "ADF7C8"

    def test_get_aircraft(self, client):
        resp = client.get("/api/aircraft/A00001")
        assert resp.status_code == 200
        data = resp.get_json()
        assert data["aircraft"]["icao"] == "A00001"
        assert data["aircraft"]["country"] == "United States"
        assert len(data["positions"]) == 2

    def test_get_aircraft_not_found(self, client):
        resp = client.get("/api/aircraft/FFFFFF")
        assert resp.status_code == 404

    def test_case_insensitive(self, client):
        resp = client.get("/api/aircraft/a00001")
        assert resp.status_code == 200


class TestAPIPositions:
    def test_recent_positions(self, client):
        resp = client.get("/api/positions")
        assert resp.status_code == 200
        data = resp.get_json()
        assert "positions" in data
        # A00001 has positions, ADF7C8 does not
        assert data["count"] >= 1

    def test_position_has_aircraft_data(self, client):
        resp = client.get("/api/positions")
        data = resp.get_json()
        pos = data["positions"][0]
        assert "registration" in pos
        assert "country" in pos


class TestAPIEvents:
    def test_list_events(self, client):
        resp = client.get("/api/events")
        assert resp.status_code == 200
        data = resp.get_json()
        assert data["count"] >= 1

    def test_filter_by_type(self, client):
        resp = client.get("/api/events?type=military_detected")
        data = resp.get_json()
        assert data["count"] == 1


class TestAPIStats:
    def test_stats(self, client):
        resp = client.get("/api/stats")
        assert resp.status_code == 200
        data = resp.get_json()
        assert data["aircraft"] == 2
        assert data["positions"] == 2
        assert data["receivers"] == 1


class TestPageRoutes:
    def test_map_page(self, client):
        resp = client.get("/")
        assert resp.status_code == 200
        assert b"leaflet" in resp.data.lower()

    def test_table_page(self, client):
        resp = client.get("/table")
        assert resp.status_code == 200
        assert b"A00001" in resp.data

    def test_aircraft_detail_page(self, client):
        resp = client.get("/aircraft/A00001")
        assert resp.status_code == 200
        assert b"A00001" in resp.data
        assert b"United States" in resp.data

    def test_aircraft_detail_not_found(self, client):
        resp = client.get("/aircraft/FFFFFF")
        assert resp.status_code == 404

    def test_stats_page(self, client):
        resp = client.get("/stats")
        assert resp.status_code == 200


class TestAPITrails:
    def test_trails_endpoint(self, client):
        # Use large minutes window since test data has old timestamps
        resp = client.get("/api/trails?minutes=999999999")
        assert resp.status_code == 200
        data = resp.get_json()
        assert "trails" in data
        # A00001 has 2 positions
        assert "A00001" in data["trails"]
        trail = data["trails"]["A00001"]
        assert len(trail) == 2
        # Each point is [lat, lon, alt, heading, speed]
        assert len(trail[0]) == 5

    def test_trails_limit(self, client):
        resp = client.get("/api/trails?limit=1&minutes=999999999")
        data = resp.get_json()
        trail = data["trails"]["A00001"]
        assert len(trail) == 1

    def test_trails_ordered_oldest_first(self, client):
        resp = client.get("/api/trails?minutes=999999999")
        data = resp.get_json()
        trail = data["trails"]["A00001"]
        # Oldest first (timestamp 1000 has alt 38000, timestamp 1001 has 37500)
        assert trail[0][2] == 38000
        assert trail[1][2] == 37500

    def test_trails_default_filters_old_data(self, client):
        """Default 60-min window excludes ancient test timestamps."""
        resp = client.get("/api/trails")
        data = resp.get_json()
        assert data["trails"] == {}


class TestStatsReceiver:
    def test_stats_has_receiver(self, client):
        resp = client.get("/api/stats")
        data = resp.get_json()
        assert "receiver" in data
        assert data["receiver"]["name"] == "test-rx"

    def test_stats_has_capture_start(self, client):
        resp = client.get("/api/stats")
        data = resp.get_json()
        assert "capture_start" in data


class TestAirports:
    def test_airports_endpoint(self, client):
        resp = client.get("/api/airports")
        assert resp.status_code == 200
        data = resp.get_json()
        assert "airports" in data
        assert data["count"] >= 20  # 3,642 from CSV, fallback is 4
        apt = data["airports"][0]
        assert "icao" in apt
        assert "name" in apt
        assert "lat" in apt
        assert "lon" in apt


class TestAllPositions:
    def test_all_positions(self, client):
        resp = client.get("/api/positions/all")
        assert resp.status_code == 200
        data = resp.get_json()
        assert "positions" in data
        assert data["count"] == 2
        # Ordered by timestamp ascending
        assert data["positions"][0]["timestamp"] <= data["positions"][1]["timestamp"]

    def test_all_positions_limit(self, client):
        resp = client.get("/api/positions/all?limit=1")
        data = resp.get_json()
        assert data["count"] == 1


class TestEventsPage:
    def test_events_page(self, client):
        resp = client.get("/events")
        assert resp.status_code == 200
        assert b"Events" in resp.data

    def test_replay_page(self, client):
        resp = client.get("/replay")
        assert resp.status_code == 200
        assert b"Replay" in resp.data

    def test_receivers_page(self, client):
        resp = client.get("/receivers")
        assert resp.status_code == 200
        assert b"Receivers" in resp.data


class TestQueryAPI:
    def test_query_all(self, client):
        resp = client.get("/api/query")
        assert resp.status_code == 200
        data = resp.get_json()
        assert "positions" in data
        assert data["count"] >= 1

    def test_query_min_alt(self, client):
        resp = client.get("/api/query?min_alt=39000")
        data = resp.get_json()
        # Only one position at 38000, should be excluded
        assert data["count"] == 0

    def test_query_max_alt(self, client):
        resp = client.get("/api/query?max_alt=37000")
        data = resp.get_json()
        # Both positions (38000, 37500) are above 37000
        assert data["count"] == 0

    def test_query_icao_filter(self, client):
        resp = client.get("/api/query?icao=A00001")
        data = resp.get_json()
        assert data["count"] == 2
        assert all(p["icao"] == "A00001" for p in data["positions"])

    def test_query_military_filter(self, client):
        resp = client.get("/api/query?military=1")
        data = resp.get_json()
        # ADF7C8 is military but has no positions
        assert data["count"] == 0

    def test_query_limit(self, client):
        resp = client.get("/api/query?limit=1")
        data = resp.get_json()
        assert data["count"] == 1

    def test_query_page(self, client):
        resp = client.get("/query")
        assert resp.status_code == 200
        assert b"Query" in resp.data


class TestCORS:
    def test_cors_header(self, client):
        resp = client.get("/api/stats")
        assert resp.headers.get("Access-Control-Allow-Origin") == "*"
