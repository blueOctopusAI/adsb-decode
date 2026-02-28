"""Ingest API — receives frames from remote feeder agents.

Endpoints:
  POST /api/v1/frames     Accept batch of hex frames from a feeder
  POST /api/v1/heartbeat  Receiver heartbeat/status update
  GET  /api/v1/receivers  List all known receivers with status

Authentication: Bearer token in Authorization header (optional, configured per-deployment).
"""

from __future__ import annotations

import time

from flask import Blueprint, g, jsonify, request

from ..database import Database
from ..frame_parser import parse_frame
from ..tracker import Tracker
from ..filters import FilterEngine

ingest = Blueprint("ingest", __name__, url_prefix="/api/v1")

# Module-level tracker state (shared across requests for the same process)
_trackers: dict[str, Tracker] = {}
_filter_engine = FilterEngine()

# Receiver heartbeat data (in-memory, not persisted)
_receiver_status: dict[str, dict] = {}


def _db() -> Database:
    return g.db


def _get_api_key():
    """Get configured API key from app config, if any."""
    from flask import current_app
    return current_app.config.get("INGEST_API_KEY", "")


def _check_auth() -> bool:
    """Validate bearer token if API key is configured."""
    required_key = _get_api_key()
    if not required_key:
        return True  # No auth configured
    auth = request.headers.get("Authorization", "")
    if auth.startswith("Bearer "):
        return auth[7:] == required_key
    return False


@ingest.route("/frames", methods=["POST"])
def receive_frames():
    """Accept a batch of hex frames from a feeder agent.

    Expected JSON body:
    {
        "receiver": "roof-antenna",
        "lat": 35.18,
        "lon": -83.38,
        "frames": ["8D40621D58C382D690C8AC2863A7", ...],
        "timestamp": 1234567890.0
    }
    """
    if not _check_auth():
        return jsonify({"error": "Unauthorized"}), 401

    data = request.get_json()
    if not data or "frames" not in data:
        return jsonify({"error": "Missing 'frames' in request body"}), 400

    receiver_name = data.get("receiver", "unknown")
    frames = data["frames"]
    recv_lat = data.get("lat")
    recv_lon = data.get("lon")
    ts = data.get("timestamp", time.time())

    db = _db()

    # Register/get receiver
    rid = db.add_receiver(receiver_name, lat=recv_lat, lon=recv_lon)

    # Get or create tracker for this receiver
    if receiver_name not in _trackers:
        cap_id = db.start_capture(source=f"feeder:{receiver_name}", receiver_id=rid)
        _trackers[receiver_name] = Tracker(
            db=db, receiver_id=rid, capture_id=cap_id,
            ref_lat=recv_lat, ref_lon=recv_lon,
        )

    tracker = _trackers[receiver_name]
    # Ensure tracker uses current db connection (per-request)
    tracker.db = db

    decoded = 0
    positions = 0
    events_fired = []

    for hex_str in frames:
        frame = parse_frame(hex_str, timestamp=ts)
        if not frame:
            continue
        msg = tracker.update(frame)
        if msg:
            decoded += 1
            ac = tracker.aircraft.get(msg.icao)
            if ac:
                events = _filter_engine.check(ac)
                for event in events:
                    db.add_event(
                        icao=event.icao,
                        event_type=event.event_type,
                        description=event.description,
                        lat=event.lat,
                        lon=event.lon,
                        altitude_ft=event.altitude_ft,
                        timestamp=event.timestamp,
                    )
                    events_fired.append(event.description)
                if ac.has_position:
                    positions += 1

    # Update receiver status
    _receiver_status[receiver_name] = {
        "name": receiver_name,
        "lat": recv_lat,
        "lon": recv_lon,
        "last_seen": time.time(),
        "frames_received": len(frames),
        "frames_decoded": decoded,
        "active_aircraft": len(tracker.get_active()),
    }

    return jsonify({
        "accepted": len(frames),
        "decoded": decoded,
        "positions": positions,
        "events": events_fired,
    })


@ingest.route("/heartbeat", methods=["POST"])
def heartbeat():
    """Receiver heartbeat — status update without frames."""
    if not _check_auth():
        return jsonify({"error": "Unauthorized"}), 401

    data = request.get_json() or {}
    name = data.get("receiver", "unknown")

    _receiver_status[name] = {
        "name": name,
        "lat": data.get("lat"),
        "lon": data.get("lon"),
        "last_seen": time.time(),
        "frames_captured": data.get("frames_captured", 0),
        "frames_sent": data.get("frames_sent", 0),
        "uptime_sec": data.get("uptime_sec", 0),
    }

    return jsonify({"status": "ok"})


@ingest.route("/receivers", methods=["GET"])
def list_receivers():
    """List all known receivers with their status."""
    db = _db()
    # Merge database receivers with in-memory status
    db_receivers = db.conn.execute(
        "SELECT * FROM receivers ORDER BY created_at"
    ).fetchall()

    receivers = []
    for row in db_receivers:
        r = dict(row)
        # Merge live status if available
        status = _receiver_status.get(r["name"], {})
        r["online"] = status.get("last_seen", 0) > time.time() - 60
        r["last_heartbeat"] = status.get("last_seen")
        r["active_aircraft"] = status.get("active_aircraft", 0)
        receivers.append(r)

    return jsonify({"receivers": receivers, "count": len(receivers)})
