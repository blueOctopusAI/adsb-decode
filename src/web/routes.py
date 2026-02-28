"""REST API and page routes for the web dashboard.

API endpoints (JSON):
  GET /api/aircraft          List all tracked aircraft (with optional filters)
  GET /api/aircraft/<icao>   Single aircraft detail + recent positions
  GET /api/positions         Recent positions (for map updates, 2s polling)
  GET /api/events            Recent events (military, emergency, anomaly)
  GET /api/stats             Database statistics

Page routes (HTML):
  GET /                      Map view (Leaflet.js)
  GET /table                 Aircraft table with sort/filter
  GET /aircraft/<icao>       Single aircraft detail + history
  GET /stats                 Statistics dashboard
"""

from __future__ import annotations

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
    """Database statistics."""
    db = _db()
    return jsonify(db.stats())


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


@pages.route("/stats")
def stats_page():
    """Statistics dashboard page."""
    db = _db()
    return render_template("stats.html", stats=db.stats())


def register_routes(app: Flask):
    """Register all blueprints with the app."""
    app.register_blueprint(api)
    app.register_blueprint(pages)
