"""Intelligence filters — military detection, emergency alerts, anomaly detection, geofence.

Filter types:
- Military: ICAO address in military allocation block, or callsign matches patterns
- Emergency: Squawk 7500 (hijack), 7600 (radio failure), 7700 (general emergency)
- Anomaly: Rapid descent (>5000 ft/min), circling patterns, unusually low altitude
- Geofence: Aircraft entering a configured lat/lon/radius zone

Each filter produces Event records written to the events table.
"""
