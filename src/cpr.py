"""Compact Position Reporting — the hardest part of ADS-B.

Decodes 17-bit CPR-encoded latitude/longitude into geographic coordinates.

Two decode modes:
- Global decode: Requires even+odd frame pair within 10 seconds. No reference needed.
  Determines zone index from both frames, computes precise lat/lon (~5.1m resolution).
- Local decode: Single frame + reference position within 180nm.
  Uses reference to determine zone, decodes relative position.

Key constants:
- NZ = 15 (number of latitude zones per hemisphere for even frames)
- Nb = 17 (bits per coordinate)
- Dlat_even = 360 / (4 * NZ) = 6.0°
- Dlat_odd = 360 / (4 * NZ - 1) = 360/59 ≈ 6.1017°

Edge cases: zone boundary crossings, polar regions (NL=1), antimeridian wrapping.
"""
