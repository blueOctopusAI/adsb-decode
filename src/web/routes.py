"""REST API and page routes for the web dashboard.

API endpoints (JSON):
  GET /api/aircraft          List all tracked aircraft (with optional filters)
  GET /api/aircraft/<icao>   Single aircraft detail + recent positions
  GET /api/positions         Recent positions (for map updates, 2s polling)
  GET /api/trails            Position trails per aircraft (for map polylines)
  GET /api/heatmap           Position density data for heatmap layer
  GET /api/events            Recent events (military, emergency, anomaly)
  GET /api/stats             Database statistics + receiver info
  GET /api/geofences         List configured geofences
  POST /api/geofences        Add a geofence
  DELETE /api/geofences/<id> Remove a geofence

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

# Module-level lookup cache: {icao: (result_dict, timestamp)}
_lookup_cache: dict[str, tuple[dict, float]] = {}
_LOOKUP_CACHE_TTL = 3600  # 1 hour

# Module-level geofence store (in-memory, survives across requests)
# List of dicts: {id, name, lat, lon, radius_nm, description}
_geofences: list[dict] = []
_geofence_next_id = 1

# Live tracker reference — set by app.py when running with a dongle.
# When set, /api/positions serves from in-memory state (~0.5s latency).
# When None, falls back to DB queries (~2s latency).
_live_tracker = None

from ..enrichment import AIRPORTS

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
    events = db.get_events(icao=icao)

    return jsonify({
        "aircraft": {**ac, "is_military": bool(ac["is_military"])},
        "positions": positions,
        "events": events,
    })


@api.route("/positions")
def recent_positions():
    """Recent positions for map updates (polling endpoint).

    When a live tracker is attached (local dongle mode), serves from
    in-memory state for sub-second latency. Otherwise queries the DB.

    Query params:
        minutes: Only show aircraft seen within the last N minutes
    """
    minutes = request.args.get("minutes", type=int)
    now = time.time()

    # Fast path: serve from live tracker memory
    if _live_tracker is not None:
        cutoff = now - (max(1, min(minutes, 525600)) * 60) if minutes else 0
        positions = []
        for ac in _live_tracker.aircraft.values():
            if ac.lat is None or ac.lon is None:
                continue
            if cutoff and ac.last_seen < cutoff:
                continue
            positions.append({
                "icao": ac.icao,
                "lat": ac.lat,
                "lon": ac.lon,
                "altitude_ft": ac.altitude_ft,
                "speed_kts": ac.speed_kts,
                "heading_deg": ac.heading_deg,
                "vertical_rate_fpm": ac.vertical_rate_fpm,
                "timestamp": ac.last_seen,
                "registration": ac.registration,
                "country": ac.country,
                "is_military": ac.is_military,
            })
        positions.sort(key=lambda p: p["timestamp"], reverse=True)
        return jsonify({"positions": positions, "count": len(positions)})

    # Slow path: query DB
    db = _db()
    if minutes is not None:
        minutes = max(1, min(minutes, 525600))  # 1 min to 1 year
        cutoff = now - (minutes * 60)
        rows = db.conn.execute("""
            SELECT p.*, a.registration, a.country, a.is_military
            FROM positions p
            JOIN aircraft a ON p.icao = a.icao
            WHERE p.id IN (
                SELECT MAX(id) FROM positions GROUP BY icao
            ) AND p.timestamp >= ?
            ORDER BY p.timestamp DESC
        """, (cutoff,)).fetchall()
    else:
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


@api.route("/query")
def query_positions():
    """Query positions with filters — power user endpoint for the query builder.

    Params:
        min_alt: Minimum altitude in feet
        max_alt: Maximum altitude in feet
        icao: Filter by ICAO address
        military: 1 for military only, 0 for civilian only
        limit: Max results (default 5000)
    """
    db = _db()
    clauses = ["1=1"]
    params = []

    min_alt = request.args.get("min_alt", type=int)
    max_alt = request.args.get("max_alt", type=int)
    icao_filter = request.args.get("icao", "").upper()
    military = request.args.get("military")
    limit = min(request.args.get("limit", 5000, type=int), 50000)

    if min_alt is not None:
        clauses.append("p.altitude_ft >= ?")
        params.append(min_alt)
    if max_alt is not None:
        clauses.append("p.altitude_ft <= ?")
        params.append(max_alt)
    if icao_filter:
        clauses.append("p.icao = ?")
        params.append(icao_filter)
    if military == "1":
        clauses.append("a.is_military = 1")
    elif military == "0":
        clauses.append("a.is_military = 0")

    where = " AND ".join(clauses)
    params.append(limit)

    rows = db.conn.execute(f"""
        SELECT p.*, a.registration, a.country, a.is_military
        FROM positions p
        JOIN aircraft a ON p.icao = a.icao
        WHERE {where}
        ORDER BY p.timestamp DESC
        LIMIT ?
    """, params).fetchall()

    positions = []
    for row in rows:
        p = dict(row)
        p["is_military"] = bool(p.get("is_military", 0))
        positions.append(p)

    return jsonify({"positions": positions, "count": len(positions)})


@api.route("/airports")
def list_airports():
    """List all known airports for map overlay."""
    from ..enrichment import _AIRPORT_TYPES
    airports = [
        {"icao": code, "name": name, "lat": lat, "lon": lon,
         "elevation_ft": elev, "type": _AIRPORT_TYPES.get(code, "small_airport")}
        for code, name, lat, lon, elev in AIRPORTS
    ]
    return jsonify({"airports": airports, "count": len(airports)})


@api.route("/positions/all")
def all_positions():
    """All positions in the database, ordered by timestamp.

    Used by the replay page. Returns all position records with aircraft info.
    """
    db = _db()
    limit = min(request.args.get("limit", 10000, type=int), 50000)

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
    """Position trails for all active aircraft.

    Returns trails keyed by ICAO with arrays of [lat, lon, alt, hdg, spd] for polylines.
    Query params:
        minutes: Only show positions from the last N minutes (default: 60)
        limit: Max positions per aircraft (default: 500)
    """
    db = _db()
    limit = min(request.args.get("limit", 500, type=int), 5000)
    minutes = max(1, min(request.args.get("minutes", 60, type=int), 525600))

    cutoff = time.time() - (minutes * 60)

    # Get all aircraft with positions in the time window
    rows = db.conn.execute(
        """SELECT DISTINCT icao FROM positions WHERE timestamp >= ?""",
        (cutoff,),
    ).fetchall()

    trails = {}
    for row in rows:
        icao = row["icao"]
        positions = db.conn.execute(
            """SELECT lat, lon, altitude_ft, heading_deg, speed_kts, timestamp
               FROM positions WHERE icao = ? AND timestamp >= ?
               ORDER BY timestamp ASC LIMIT ?""",
            (icao, cutoff, limit),
        ).fetchall()
        trails[icao] = [
            [p["lat"], p["lon"], p["altitude_ft"], p["heading_deg"], p["speed_kts"]]
            for p in positions
        ]

    return jsonify({"trails": trails})


@api.route("/heatmap")
def heatmap_data():
    """Position density data for heatmap layer.

    Returns lat/lon points from all positions in the time window,
    sampled to a reasonable count for client-side rendering.
    Query params:
        minutes: Time window (default: 1440 = 24h, max: 10080 = 7 days)
    """
    db = _db()
    minutes = max(1, min(request.args.get("minutes", 1440, type=int), 10080))
    cutoff = time.time() - (minutes * 60)

    # Sample up to 50k points — enough for density without killing the browser
    rows = db.conn.execute("""
        SELECT lat, lon, altitude_ft FROM positions
        WHERE timestamp >= ?
        ORDER BY timestamp DESC
        LIMIT 50000
    """, (cutoff,)).fetchall()

    points = [[r["lat"], r["lon"], r["altitude_ft"]] for r in rows]
    return jsonify({"points": points, "count": len(points)})


@api.route("/events")
def list_events():
    """Recent events."""
    db = _db()
    event_type = request.args.get("type")
    limit = min(request.args.get("limit", 50, type=int), 5000)
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


@api.route("/geofences", methods=["GET"])
def list_geofences():
    """List all configured geofences."""
    return jsonify({"geofences": _geofences, "count": len(_geofences)})


@api.route("/geofences", methods=["POST"])
def add_geofence():
    """Add a new geofence zone.

    JSON body: {name, lat, lon, radius_nm, description?}
    """
    global _geofence_next_id
    data = request.get_json(silent=True) or {}

    name = data.get("name", "").strip()
    lat = data.get("lat")
    lon = data.get("lon")
    radius_nm = data.get("radius_nm")

    if not name or lat is None or lon is None or radius_nm is None:
        return jsonify({"error": "name, lat, lon, radius_nm required"}), 400

    try:
        lat = float(lat)
        lon = float(lon)
        radius_nm = float(radius_nm)
    except (TypeError, ValueError):
        return jsonify({"error": "lat, lon, radius_nm must be numbers"}), 400

    if radius_nm <= 0 or radius_nm > 500:
        return jsonify({"error": "radius_nm must be 0-500"}), 400

    fence = {
        "id": _geofence_next_id,
        "name": name,
        "lat": lat,
        "lon": lon,
        "radius_nm": radius_nm,
        "description": data.get("description", ""),
    }
    _geofences.append(fence)
    _geofence_next_id += 1
    return jsonify(fence), 201


@api.route("/geofences/<int:fence_id>", methods=["DELETE"])
def delete_geofence(fence_id: int):
    """Remove a geofence by ID."""
    global _geofences
    before = len(_geofences)
    _geofences = [f for f in _geofences if f["id"] != fence_id]
    if len(_geofences) == before:
        return jsonify({"error": "Geofence not found"}), 404
    return jsonify({"deleted": fence_id})


@api.route("/lookup/<icao>")
def lookup_aircraft(icao: str):
    """Proxy lookup to hexdb.io for aircraft metadata.

    Returns manufacturer, type, owner, registration from external DB.
    Cached in-memory for the session to avoid repeated external calls.
    """
    icao = icao.upper()
    # Module-level cache with TTL
    cached = _lookup_cache.get(icao)
    if cached and time.time() - cached[1] < _LOOKUP_CACHE_TTL:
        return jsonify(cached[0])

    import urllib.request
    import json as _json

    result = {"icao": icao, "source": "hexdb.io"}
    try:
        url = f"https://hexdb.io/api/v1/aircraft/{icao}"
        req = urllib.request.Request(url, headers={"User-Agent": "adsb-decode/1.0"})
        with urllib.request.urlopen(req, timeout=5) as resp:
            data = _json.loads(resp.read().decode())
            result.update({
                "registration": data.get("Registration", ""),
                "manufacturer": data.get("Manufacturer", ""),
                "type_code": data.get("ICAOTypeCode", ""),
                "type": data.get("Type", ""),
                "owner": data.get("RegisteredOwners", ""),
                "operator_code": data.get("OperatorFlagCode", ""),
            })
    except Exception:
        result["error"] = "Lookup failed"

    _lookup_cache[icao] = (result, time.time())
    return jsonify(result)


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
    """Single aircraft detail page with split-screen external intel."""
    db = _db()
    icao = icao.upper()
    ac = db.get_aircraft(icao)
    if not ac:
        return "Aircraft not found", 404
    positions = db.get_positions(icao, limit=500)
    events = db.get_events(icao=icao)
    # Get sighting info (callsign, squawk)
    sighting = db.conn.execute(
        "SELECT callsign, squawk FROM sightings WHERE icao = ? ORDER BY id DESC LIMIT 1",
        (icao,),
    ).fetchone()
    return render_template(
        "detail.html",
        aircraft=ac,
        positions=positions,
        events=events,
        sighting=dict(sighting) if sighting else {},
    )


@pages.route("/query")
def query_page():
    """Query builder page."""
    return render_template("query.html")


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
