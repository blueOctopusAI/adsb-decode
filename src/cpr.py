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
- Dlat_even = 360 / (4 * NZ) = 6.0 degrees
- Dlat_odd = 360 / (4 * NZ - 1) = 360/59 ~ 6.1017 degrees

Edge cases: zone boundary crossings, polar regions (NL=1), antimeridian wrapping.
"""

from __future__ import annotations

import math

NZ = 15  # Number of latitude zones per hemisphere
NB = 17  # Bits per coordinate
CPR_MAX = 2**NB  # 131072

# Maximum time between even/odd frames for global decode (seconds)
MAX_PAIR_AGE = 10.0


def _nl(lat: float) -> int:
    """Number of longitude zones at a given latitude (NL function).

    Returns the number of longitude zones for the given latitude.
    This determines how many CPR longitude zones exist at that latitude.

    Ranges from NL=1 near poles to NL=59 at equator.
    """
    if abs(lat) >= 87.0:
        return 1

    # NL formula from ICAO Doc 9871
    nz = NZ
    a = 1 - math.cos(math.pi / (2 * nz))
    b = math.cos(math.pi / 180 * abs(lat)) ** 2
    nl = math.floor(2 * math.pi / math.acos(1 - a / b))
    return max(nl, 1)


def _mod(x: float, y: float) -> float:
    """Modulo that always returns non-negative result."""
    return x - y * math.floor(x / y)


def global_decode(
    lat_even: int,
    lon_even: int,
    lat_odd: int,
    lon_odd: int,
    t_even: float,
    t_odd: float,
) -> tuple[float, float] | None:
    """Global CPR decode from an even/odd frame pair.

    Args:
        lat_even: 17-bit CPR latitude from even frame
        lon_even: 17-bit CPR longitude from even frame
        lat_odd: 17-bit CPR latitude from odd frame
        lon_odd: 17-bit CPR longitude from odd frame
        t_even: Timestamp of even frame
        t_odd: Timestamp of odd frame

    Returns:
        (latitude, longitude) in degrees, or None if decode fails
        (e.g., zone boundary crossing).
    """
    # Check time difference
    if abs(t_even - t_odd) > MAX_PAIR_AGE:
        return None

    # Latitude zone sizes
    dlat_even = 360.0 / (4 * NZ)       # 6.0 degrees
    dlat_odd = 360.0 / (4 * NZ - 1)    # ~6.1017 degrees

    # Normalize CPR values to [0, 1)
    lat_even_cpr = lat_even / CPR_MAX
    lon_even_cpr = lon_even / CPR_MAX
    lat_odd_cpr = lat_odd / CPR_MAX
    lon_odd_cpr = lon_odd / CPR_MAX

    # Compute latitude zone index j
    j = math.floor(59 * lat_even_cpr - 60 * lat_odd_cpr + 0.5)

    # Compute candidate latitudes
    lat_e = dlat_even * (_mod(j, 60) + lat_even_cpr)
    lat_o = dlat_odd * (_mod(j, 59) + lat_odd_cpr)

    # Normalize to [-90, 90]
    if lat_e >= 270:
        lat_e -= 360
    if lat_o >= 270:
        lat_o -= 360

    # Check that both latitudes give the same NL value
    if _nl(lat_e) != _nl(lat_o):
        return None  # Zone boundary crossing — discard pair

    # Use the most recent frame to compute longitude
    if t_even >= t_odd:
        # Use even frame
        lat = lat_e
        nl = _nl(lat)
        n_lon = max(nl, 1)
        dlon = 360.0 / n_lon if n_lon > 0 else 360.0
        m = math.floor(lon_even_cpr * (nl - 1) - lon_odd_cpr * nl + 0.5)
        lon = dlon * (_mod(m, n_lon) + lon_even_cpr)
    else:
        # Use odd frame
        lat = lat_o
        nl = _nl(lat)
        n_lon = max(nl - 1, 1)
        dlon = 360.0 / n_lon if n_lon > 0 else 360.0
        m = math.floor(lon_even_cpr * (nl - 1) - lon_odd_cpr * nl + 0.5)
        lon = dlon * (_mod(m, n_lon) + lon_odd_cpr)

    # Normalize longitude to [-180, 180]
    if lon >= 180:
        lon -= 360

    return (round(lat, 6), round(lon, 6))


def local_decode(
    cpr_lat: int,
    cpr_lon: int,
    cpr_odd: bool,
    ref_lat: float,
    ref_lon: float,
) -> tuple[float, float]:
    """Local CPR decode using a reference position.

    Uses a known reference position (receiver location or last decoded position)
    to resolve the CPR zone without needing a frame pair.

    Valid when the aircraft is within ~180nm of the reference.

    Args:
        cpr_lat: 17-bit CPR latitude
        cpr_lon: 17-bit CPR longitude
        cpr_odd: True if odd frame, False if even
        ref_lat: Reference latitude in degrees
        ref_lon: Reference longitude in degrees

    Returns:
        (latitude, longitude) in degrees.
    """
    i = 1 if cpr_odd else 0
    dlat = 360.0 / (4 * NZ - i)

    cpr_lat_norm = cpr_lat / CPR_MAX
    cpr_lon_norm = cpr_lon / CPR_MAX

    # Compute latitude zone index from reference
    j = math.floor(ref_lat / dlat) + math.floor(
        _mod(ref_lat, dlat) / dlat - cpr_lat_norm + 0.5
    )
    lat = dlat * (j + cpr_lat_norm)

    # Compute longitude zone size at this latitude
    nl = _nl(lat)
    n_lon = max(nl - i, 1)
    dlon = 360.0 / n_lon

    # Compute longitude zone index from reference
    m = math.floor(ref_lon / dlon) + math.floor(
        _mod(ref_lon, dlon) / dlon - cpr_lon_norm + 0.5
    )
    lon = dlon * (m + cpr_lon_norm)

    # Normalize
    if lat > 90:
        lat -= 360
    if lon >= 180:
        lon -= 360

    return (round(lat, 6), round(lon, 6))
