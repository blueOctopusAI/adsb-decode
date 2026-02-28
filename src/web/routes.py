"""REST API and page routes for the web dashboard.

API endpoints (JSON):
  GET /api/aircraft          List all tracked aircraft (with optional filters)
  GET /api/aircraft/<icao>   Single aircraft detail + recent positions
  GET /api/positions         Recent positions (for map updates, 2s polling)
  GET /api/trails            Position trails per aircraft (for map polylines)
  GET /api/events            Recent events (military, emergency, anomaly)
  GET /api/stats             Database statistics + receiver info

Page routes (HTML):
  GET /                      Map view (Leaflet.js)
  GET /table                 Aircraft table with sort/filter
  GET /aircraft/<icao>       Single aircraft detail + history
  GET /events                Events dashboard
  GET /stats                 Statistics dashboard
"""

from __future__ import annotations

import time

from flask import Blueprint, Flask, g, jsonify, render_template, request

api = Blueprint("api", __name__, url_prefix="/api")
pages = Blueprint("pages", __name__)


def _db():
    return g.db


# --- API Routes ---

@api.route("/aircraft")
def list_aircraft():
    """List all aircraft with optional filters."""
    db = _db()
    military_only = request.args.get("military", "").lower() == "true"

    rows = db.conn.execute(
        "SELECT * FROM aircraft ORDER BY last_seen DESC"
    ).fetchall()

    aircraft = []
    for row in rows:
        ac = dict(row)
        ac["is_military"] = bool(ac["is_military"])
        if military_only and not ac["is_military"]:
            continue
        aircraft.append(ac)

    return jsonify({"aircraft": aircraft, "count": len(aircraft)})


@api.route("/aircraft/<icao>")
def get_aircraft(icao: str):
    """Single aircraft detail with recent positions."""
    db = _db()
    icao = icao.upper()
    ac = db.get_aircraft(icao)
    if not ac:
        return jsonify({"error": "Aircraft not found"}), 404

    positions = db.get_positions(icao, limit=100)
    events = [
        dict(e) for e in db.get_events()
        if e["icao"] == icao
    ]

    return jsonify({
        "aircraft": {**ac, "is_military": bool(ac["is_military"])},
        "positions": positions,
        "events": events,
    })


@api.route("/positions")
def recent_positions():
    """Recent positions for map updates (polling endpoint).

    Returns the most recent position per aircraft.
    """
    db = _db()
    rows = db.conn.execute("""
        SELECT p.*, a.registration, a.country, a.is_military
        FROM positions p
        JOIN aircraft a ON p.icao = a.icao
        WHERE p.id IN (
            SELECT MAX(id) FROM positions GROUP BY icao
        )
        ORDER BY p.timestamp DESC
    """).fetchall()

    positions = []
    for row in rows:
        p = dict(row)
        p["is_military"] = bool(p.get("is_military", 0))
        positions.append(p)

    return jsonify({"positions": positions, "count": len(positions)})


@api.route("/positions/all")
def all_positions():
    """All positions in the database, ordered by timestamp.

    Used by the replay page. Returns all position records with aircraft info.
    """
    db = _db()
    limit = int(request.args.get("limit", 10000))

    rows = db.conn.execute("""
        SELECT p.*, a.registration, a.country, a.is_military,
               s.callsign
        FROM positions p
        JOIN aircraft a ON p.icao = a.icao
        LEFT JOIN sightings s ON s.icao = p.icao
        ORDER BY p.timestamp ASC
        LIMIT ?
    """, (limit,)).fetchall()

    positions = []
    for row in rows:
        p = dict(row)
        p["is_military"] = bool(p.get("is_military", 0))
        positions.append(p)

    return jsonify({"positions": positions, "count": len(positions)})


@api.route("/trails")
def get_trails():
    """Position trails for all active aircraft (last N positions each).

    Returns trails keyed by ICAO with arrays of [lat, lon, alt] for polylines.
    """
    db = _db()
    limit = int(request.args.get("limit", 50))

    # Get all aircraft with recent positions
    rows = db.conn.execute("""
        SELECT DISTINCT icao FROM positions
        WHERE id IN (SELECT MAX(id) FROM positions GROUP BY icao)
    """).fetchall()

    trails = {}
    for row in rows:
        icao = row["icao"]
        positions = db.conn.execute(
            """SELECT lat, lon, altitude_ft, heading_deg, speed_kts, timestamp
               FROM positions WHERE icao = ?
               ORDER BY timestamp DESC LIMIT ?""",
            (icao, limit),
        ).fetchall()
        # Reverse so oldest first (for drawing polylines start→end)
        trails[icao] = [
            [p["lat"], p["lon"], p["altitude_ft"], p["heading_deg"], p["speed_kts"]]
            for p in reversed(positions)
        ]

    return jsonify({"trails": trails})


@api.route("/events")
def list_events():
    """Recent events."""
    db = _db()
    event_type = request.args.get("type")
    limit = int(request.args.get("limit", 50))
    events = db.get_events(event_type=event_type, limit=limit)
    return jsonify({"events": events, "count": len(events)})


@api.route("/stats")
def get_stats():
    """Database statistics with receiver info."""
    db = _db()
    s = db.stats()

    # Add receiver location for map centering
    receiver = db.conn.execute(
        "SELECT * FROM receivers ORDER BY created_at DESC LIMIT 1"
    ).fetchone()
    if receiver:
        s["receiver"] = {
            "name": receiver["name"],
            "lat": receiver["lat"],
            "lon": receiver["lon"],
        }

    # Add capture start time for uptime calculation
    capture = db.conn.execute(
        "SELECT start_time FROM captures ORDER BY start_time DESC LIMIT 1"
    ).fetchone()
    if capture:
        s["capture_start"] = capture["start_time"]

    return jsonify(s)


# --- Page Routes ---

@pages.route("/")
def map_view():
    """Main map view."""
    return render_template("map.html")


@pages.route("/table")
def table_view():
    """Aircraft table view."""
    db = _db()
    rows = db.conn.execute(
        "SELECT * FROM aircraft ORDER BY last_seen DESC"
    ).fetchall()
    aircraft = [dict(r) for r in rows]
    return render_template("table.html", aircraft=aircraft)


@pages.route("/aircraft/<icao>")
def aircraft_detail(icao: str):
    """Single aircraft detail page."""
    db = _db()
    icao = icao.upper()
    ac = db.get_aircraft(icao)
    if not ac:
        return "Aircraft not found", 404
    positions = db.get_positions(icao, limit=200)
    return render_template("detail.html", aircraft=ac, positions=positions)


@pages.route("/replay")
def replay_page():
    """Historical replay page."""
    return render_template("replay.html")


@pages.route("/events")
def events_page():
    """Events dashboard page."""
    return render_template("events.html")


@pages.route("/receivers")
def receivers_page():
    """Receiver management page."""
    return render_template("receivers.html")


@pages.route("/stats")
def stats_page():
    """Statistics dashboard page."""
    db = _db()
    return render_template("stats.html", stats=db.stats())


def register_routes(app: Flask):
    """Register all blueprints with the app."""
    app.register_blueprint(api)
    app.register_blueprint(pages)
