"""SQLite persistence — WAL mode, 5 tables, indexed queries.

Schema:
- aircraft:   One row per unique ICAO address. Country, registration, military flag, first/last seen.
- sightings:  Per capture session per aircraft. Callsign, squawk, signal strength stats.
- positions:  Time-series lat/lon/alt/speed/heading/vrate. The historical record.
- captures:   Metadata per capture session. Source file, duration, frame counts.
- events:     Detected anomalies. Emergency squawk, rapid descent, military, geofence breach.

Follows BluePages pattern: WAL mode, foreign keys, indexed, tmp_path test fixtures.
"""
