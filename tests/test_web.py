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


class TestCORS:
    def test_cors_header(self, client):
        resp = client.get("/api/stats")
        assert resp.headers.get("Access-Control-Allow-Origin") == "*"
