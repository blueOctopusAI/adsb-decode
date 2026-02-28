"""Aircraft type enrichment — identify aircraft category from observed data.

Infers aircraft type from:
1. Speed/altitude profiles (fast+high = jet, slow+low = prop, etc.)
2. ICAO address blocks (military ranges)
3. Callsign patterns (airline ICAO prefixes)

Categories:
- jet:        Commercial jet transport (B737, A320, etc.)
- prop:       Single/twin engine propeller (C172, PA28, etc.)
- turboprop:  Turboprop transport (ATR72, Dash 8, etc.)
- helicopter: Rotary wing
- military:   Military aircraft (any type)
- cargo:      Cargo/freight (identified by operator)
- unknown:    Insufficient data

No external databases required — works purely from observed ADS-B data.
Supplement with OpenSky or FAA DB files if available.
"""

from __future__ import annotations

import csv
import sqlite3
from pathlib import Path

# Airline ICAO prefixes → operator name (common ones for North America)
AIRLINE_PREFIXES: dict[str, str] = {
    "AAL": "American Airlines",
    "DAL": "Delta Air Lines",
    "UAL": "United Airlines",
    "SWA": "Southwest Airlines",
    "JBU": "JetBlue Airways",
    "NKS": "Spirit Airlines",
    "FFT": "Frontier Airlines",
    "ASA": "Alaska Airlines",
    "HAL": "Hawaiian Airlines",
    "SKW": "SkyWest Airlines",
    "RPA": "Republic Airways",
    "ENY": "Envoy Air",
    "ASH": "Mesa Airlines",
    "PDT": "Piedmont Airlines",
    "JIA": "PSA Airlines",
    "UPS": "UPS",
    "FDX": "FedEx",
    "GTI": "Atlas Air",
    "ABX": "ABX Air",
    "ACA": "Air Canada",
    "WJA": "WestJet",
    "BAW": "British Airways",
    "DLH": "Lufthansa",
    "AFR": "Air France",
    "EZY": "easyJet",
    "RYR": "Ryanair",
}

# Cargo airline callsign prefixes
CARGO_PREFIXES = {"UPS", "FDX", "GTI", "ABX", "CLX", "GEC", "CKS", "BOX"}

# Category constants
CAT_JET = "jet"
CAT_PROP = "prop"
CAT_TURBOPROP = "turboprop"
CAT_HELICOPTER = "helicopter"
CAT_MILITARY = "military"
CAT_CARGO = "cargo"
CAT_UNKNOWN = "unknown"


def classify_from_profile(
    speed_kts: float | None = None,
    altitude_ft: int | None = None,
    vertical_rate_fpm: int | None = None,
    is_military: bool = False,
    callsign: str | None = None,
) -> str:
    """Classify aircraft category from observed flight profile.

    Uses speed and altitude to distinguish jets from props, and callsign
    patterns to identify cargo and airlines.
    """
    if is_military:
        return CAT_MILITARY

    # Check callsign for cargo operators
    if callsign:
        prefix = callsign[:3].upper()
        if prefix in CARGO_PREFIXES:
            return CAT_CARGO

    # Speed-based classification
    if speed_kts is not None:
        if speed_kts > 250:
            return CAT_JET
        if speed_kts < 80 and altitude_ft is not None and altitude_ft < 3000:
            return CAT_HELICOPTER
        if 80 <= speed_kts <= 180:
            if altitude_ft is not None and altitude_ft > 15000:
                return CAT_TURBOPROP
            return CAT_PROP
        if 180 < speed_kts <= 250:
            return CAT_TURBOPROP

    # Altitude-only fallback
    if altitude_ft is not None:
        if altitude_ft > 30000:
            return CAT_JET
        if altitude_ft < 5000:
            return CAT_PROP

    return CAT_UNKNOWN


def lookup_operator(callsign: str | None) -> str | None:
    """Look up operator name from callsign prefix."""
    if not callsign or len(callsign) < 3:
        return None
    prefix = callsign[:3].upper()
    return AIRLINE_PREFIXES.get(prefix)


class AircraftTypeDB:
    """Local cache of aircraft type lookups.

    Stores ICAO → type/operator mappings in a small SQLite database.
    Can be populated from OpenSky Network or FAA registration CSV files.
    """

    def __init__(self, db_path: str | Path = ":memory:"):
        self.conn = sqlite3.connect(str(db_path))
        self.conn.row_factory = sqlite3.Row
        self.conn.execute("""
            CREATE TABLE IF NOT EXISTS aircraft_types (
                icao TEXT PRIMARY KEY,
                registration TEXT,
                type_code TEXT,
                type_name TEXT,
                operator TEXT,
                category TEXT
            )
        """)
        self.conn.commit()

    def lookup(self, icao: str) -> dict | None:
        """Look up aircraft type info by ICAO address."""
        row = self.conn.execute(
            "SELECT * FROM aircraft_types WHERE icao = ?", (icao.upper(),)
        ).fetchone()
        return dict(row) if row else None

    def add(
        self,
        icao: str,
        registration: str | None = None,
        type_code: str | None = None,
        type_name: str | None = None,
        operator: str | None = None,
        category: str | None = None,
    ):
        """Add or update aircraft type record."""
        self.conn.execute(
            """INSERT OR REPLACE INTO aircraft_types
               (icao, registration, type_code, type_name, operator, category)
               VALUES (?, ?, ?, ?, ?, ?)""",
            (icao.upper(), registration, type_code, type_name, operator, category),
        )
        self.conn.commit()

    def load_csv(self, csv_path: str | Path):
        """Load aircraft data from a CSV file.

        Expected columns: icao, registration, type_code, type_name, operator, category
        (Flexible — missing columns are treated as None)
        """
        path = Path(csv_path)
        if not path.exists():
            return 0

        count = 0
        with open(path, newline="", encoding="utf-8") as f:
            reader = csv.DictReader(f)
            for row in reader:
                self.add(
                    icao=row.get("icao", "").strip(),
                    registration=row.get("registration", "").strip() or None,
                    type_code=row.get("type_code", "").strip() or None,
                    type_name=row.get("type_name", "").strip() or None,
                    operator=row.get("operator", "").strip() or None,
                    category=row.get("category", "").strip() or None,
                )
                count += 1
        return count

    def count(self) -> int:
        row = self.conn.execute("SELECT COUNT(*) as cnt FROM aircraft_types").fetchone()
        return row["cnt"]

    def close(self):
        self.conn.close()


# --- Airport Awareness ---

# Bundled airports within typical ADS-B range of western NC
# Format: (ICAO, name, lat, lon, elevation_ft)
AIRPORTS: list[tuple[str, str, float, float, int]] = [
    ("KATL", "Atlanta Hartsfield-Jackson", 33.6367, -84.4281, 1026),
    ("KCLT", "Charlotte Douglas", 35.2140, -80.9431, 748),
    ("KAVL", "Asheville Regional", 35.4362, -82.5418, 2165),
    ("KGSP", "Greenville-Spartanburg", 34.8957, -82.2189, 964),
    ("KTYS", "Knoxville McGhee Tyson", 35.8110, -83.9940, 981),
    ("KCHA", "Chattanooga Metropolitan", 35.0353, -85.2038, 683),
    ("KBNA", "Nashville International", 36.1246, -86.6782, 599),
    ("KRDU", "Raleigh-Durham", 35.8776, -78.7875, 435),
    ("KGSO", "Piedmont Triad", 36.0978, -79.9373, 925),
    ("KJFK", "New York JFK", 40.6413, -73.7781, 13),
    ("KORD", "Chicago O'Hare", 41.9742, -87.9073, 672),
    ("KDFW", "Dallas/Fort Worth", 32.8998, -97.0403, 607),
    ("KMIA", "Miami International", 25.7959, -80.2870, 8),
    ("KIAD", "Washington Dulles", 38.9531, -77.4565, 312),
    ("KDCA", "Reagan National", 38.8512, -77.0402, 15),
    ("KPHL", "Philadelphia", 39.8721, -75.2408, 36),
    ("KPIT", "Pittsburgh", 40.4915, -80.2329, 1203),
    ("KCVG", "Cincinnati/Northern KY", 39.0488, -84.6678, 896),
    ("KMCO", "Orlando International", 28.4294, -81.3090, 96),
    ("KTPA", "Tampa International", 27.9755, -82.5332, 26),
]

import math


def _haversine_nm(lat1: float, lon1: float, lat2: float, lon2: float) -> float:
    """Great-circle distance in nautical miles."""
    R_NM = 3440.065
    dlat = math.radians(lat2 - lat1)
    dlon = math.radians(lon2 - lon1)
    a = (
        math.sin(dlat / 2) ** 2
        + math.cos(math.radians(lat1))
        * math.cos(math.radians(lat2))
        * math.sin(dlon / 2) ** 2
    )
    return R_NM * 2 * math.atan2(math.sqrt(a), math.sqrt(1 - a))


def nearest_airport(
    lat: float, lon: float, max_nm: float = 50.0
) -> tuple[str, str, float] | None:
    """Find nearest airport within max_nm nautical miles.

    Returns (icao_code, name, distance_nm) or None if none within range.
    """
    best = None
    best_dist = max_nm

    for icao, name, alat, alon, _ in AIRPORTS:
        dist = _haversine_nm(lat, lon, alat, alon)
        if dist < best_dist:
            best = (icao, name, dist)
            best_dist = dist

    return best


def classify_flight_phase(
    lat: float,
    lon: float,
    altitude_ft: int | None,
    vertical_rate_fpm: int | None,
    max_airport_nm: float = 30.0,
) -> str | None:
    """Classify aircraft's flight phase relative to nearest airport.

    Returns a string like "Approaching KAVL (12nm)" or "Departing KCLT (8nm)"
    or "Overflying KATL (45nm)" or None if no airport nearby.
    """
    airport = nearest_airport(lat, lon, max_nm=max_airport_nm)
    if not airport:
        return None

    code, name, dist = airport

    if altitude_ft is not None and vertical_rate_fpm is not None:
        if dist < 15 and vertical_rate_fpm < -200 and altitude_ft < 10000:
            return f"Approaching {code} ({dist:.0f}nm)"
        if dist < 15 and vertical_rate_fpm > 200 and altitude_ft < 10000:
            return f"Departing {code} ({dist:.0f}nm)"

    if dist < 5:
        return f"Near {code} ({dist:.1f}nm)"

    return f"Overflying {code} ({dist:.0f}nm)"
