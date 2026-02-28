"""Tests for ingest API — remote frame ingestion from feeder agents."""

import json

import pytest

from src.database import Database
from src.web.app import create_app


@pytest.fixture
def app(tmp_path):
    """Flask test app for ingest testing."""
    db_path = str(tmp_path / "ingest.db")
    db = Database(db_path)
    db.add_receiver("existing-rx", lat=35.0, lon=-83.0)
    db.close()

    app = create_app(db_path=db_path)
    app.config["TESTING"] = True
    return app


@pytest.fixture
def client(app):
    return app.test_client()


# Valid ADS-B frame (DF17, ICAO A00001, identification)
VALID_FRAME = "8DA00001200464B3C884F50A39D1"


class TestFrameIngestion:
    def test_accept_frames(self, client):
        resp = client.post("/api/v1/frames", json={
            "receiver": "test-feeder",
            "lat": 35.18,
            "lon": -83.38,
            "frames": [VALID_FRAME],
        })
        assert resp.status_code == 200
        data = resp.get_json()
        assert data["accepted"] == 1

    def test_empty_frames(self, client):
        resp = client.post("/api/v1/frames", json={
            "receiver": "test-feeder",
            "frames": [],
        })
        assert resp.status_code == 200
        data = resp.get_json()
        assert data["accepted"] == 0

    def test_missing_frames_field(self, client):
        resp = client.post("/api/v1/frames", json={
            "receiver": "test-feeder",
        })
        assert resp.status_code == 400

    def test_invalid_frame_skipped(self, client):
        resp = client.post("/api/v1/frames", json={
            "receiver": "test-feeder",
            "frames": ["DEADBEEF", VALID_FRAME],
        })
        data = resp.get_json()
        assert data["accepted"] == 2  # Both accepted for processing

    def test_receiver_registered(self, client):
        client.post("/api/v1/frames", json={
            "receiver": "new-feeder",
            "lat": 36.0,
            "lon": -82.0,
            "frames": [VALID_FRAME],
        })
        # Check receiver was created
        resp = client.get("/api/v1/receivers")
        data = resp.get_json()
        names = [r["name"] for r in data["receivers"]]
        assert "new-feeder" in names


class TestAuthentication:
    def test_no_auth_required_by_default(self, client):
        resp = client.post("/api/v1/frames", json={
            "receiver": "test",
            "frames": [],
        })
        assert resp.status_code == 200

    def test_auth_required_when_configured(self, app):
        app.config["INGEST_API_KEY"] = "secret123"
        client = app.test_client()

        # No auth
        resp = client.post("/api/v1/frames", json={
            "receiver": "test",
            "frames": [],
        })
        assert resp.status_code == 401

        # Wrong auth
        resp = client.post("/api/v1/frames", json={
            "receiver": "test",
            "frames": [],
        }, headers={"Authorization": "Bearer wrong"})
        assert resp.status_code == 401

        # Correct auth
        resp = client.post("/api/v1/frames", json={
            "receiver": "test",
            "frames": [],
        }, headers={"Authorization": "Bearer secret123"})
        assert resp.status_code == 200


class TestHeartbeat:
    def test_heartbeat(self, client):
        resp = client.post("/api/v1/heartbeat", json={
            "receiver": "test-feeder",
            "lat": 35.18,
            "lon": -83.38,
            "frames_captured": 1000,
            "frames_sent": 950,
            "uptime_sec": 3600,
        })
        assert resp.status_code == 200
        assert resp.get_json()["status"] == "ok"


class TestReceiverList:
    def test_list_receivers(self, client):
        resp = client.get("/api/v1/receivers")
        assert resp.status_code == 200
        data = resp.get_json()
        assert "receivers" in data
        # Has the existing-rx from fixture
        assert data["count"] >= 1

    def test_receiver_online_status(self, client):
        # Send heartbeat to make receiver "online"
        client.post("/api/v1/heartbeat", json={
            "receiver": "existing-rx",
        })
        resp = client.get("/api/v1/receivers")
        data = resp.get_json()
        rx = next(r for r in data["receivers"] if r["name"] == "existing-rx")
        assert rx["online"] is True
