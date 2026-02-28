"""Per-aircraft state machine with CPR frame pairing.

Maintains a dictionary of AircraftState objects keyed by ICAO address.
Each state tracks:
- Current position (lat, lon, altitude) from latest decode
- Current velocity (ground speed, heading, vertical rate)
- Callsign and squawk code
- CPR buffer (last even and odd position frames for global decode)
- Timestamps for age/staleness detection
- Signal strength history

Feeds decoded messages to the database and runs filter checks on each update.
Aircraft are considered stale after 60 seconds of no messages.
"""
