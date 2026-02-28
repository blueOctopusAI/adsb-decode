"""Decode Mode S frames into typed aircraft messages.

Handles all Downlink Formats and ADS-B Type Codes:
- DF17 TC 1-4:  Aircraft identification (callsign)
- DF17 TC 9-18: Airborne position (barometric alt + CPR-encoded lat/lon)
- DF17 TC 19:   Airborne velocity (ground speed or airspeed + heading)
- DF17 TC 20-22: Airborne position (GNSS altitude)
- DF17 TC 28:   Aircraft status (emergency/priority)
- DF4/20:       Surveillance/Comm-B altitude reply
- DF5/21:       Surveillance/Comm-B identity reply (squawk)
- DF11:         All-call reply (ICAO address acquisition)

Output: typed dataclasses (IdentificationMsg, PositionMsg, VelocityMsg, etc.)
"""
