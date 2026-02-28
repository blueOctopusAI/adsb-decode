"""SQLite persistence — WAL mode, 6 tables, indexed queries.

Schema:
- receivers:  Sensor nodes. Name, lat, lon, altitude, description. Multi-receiver from day one.
- aircraft:   One row per unique ICAO address. Country, registration, military flag, first/last seen.
- sightings:  Per capture session per aircraft. Callsign, squawk, signal strength stats.
- positions:  Time-series lat/lon/alt/speed/heading/vrate. Tagged with receiver_id.
- captures:   Metadata per capture session. Source file, duration, frame counts. Tagged with receiver_id.
- events:     Detected anomalies. Emergency squawk, rapid descent, military, geofence breach.

Every position and capture records which receiver heard it. Single-receiver deployments
have one row in receivers. Adding receivers is adding data sources, not refactoring.

Follows BluePages pattern: WAL mode, foreign keys, indexed, tmp_path test fixtures.
"""
