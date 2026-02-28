"""ICAO address resolution — country lookup, military detection, N-number decode.

Every aircraft has a unique 24-bit ICAO address assigned by country of registration.
Address ranges are allocated in blocks (e.g., 0xA00000-0xAFFFFF = United States).

Features:
- Country lookup from ICAO address
- Military address block detection (reserved ranges per country)
- US N-number algorithm: civil addresses 0xA00001-0xADF7C7 → tail number (e.g., N12345)
- Callsign pattern matching for military flights (RCH*, DUKE*, REACH*, etc.)
"""
