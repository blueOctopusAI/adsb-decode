"""Intelligence filters — military detection, emergency alerts, anomaly detection, geofence.

Filter types:
- Military: ICAO address in military allocation block, or callsign matches patterns
- Emergency: Squawk 7500 (hijack), 7600 (radio failure), 7700 (general emergency)
- Anomaly: Rapid descent (>5000 ft/min), circling patterns, unusually low altitude
- Geofence: Aircraft entering a configured lat/lon/radius zone

Each filter produces Event records written to the events table.
Filters run against AircraftState objects after each tracker update.
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field

from .tracker import AircraftState


# --- Event types ---

EVENT_MILITARY = "military_detected"
EVENT_EMERGENCY = "emergency_squawk"
EVENT_RAPID_DESCENT = "rapid_descent"
EVENT_LOW_ALTITUDE = "low_altitude"
EVENT_GEOFENCE = "geofence_entry"


# --- Thresholds ---

RAPID_DESCENT_THRESHOLD = -5000  # ft/min (negative = descending)
LOW_ALTITUDE_THRESHOLD = 500  # ft AGL (below this triggers alert)
EMERGENCY_SQUAWKS = {
    "7500": "Hijack",
    "7600": "Radio failure",
    "7700": "Emergency",
}


@dataclass
class Event:
    """A detected event/anomaly."""
    icao: str
    event_type: str
    description: str = ""
    lat: float | None = None
    lon: float | None = None
    altitude_ft: int | None = None
    timestamp: float = 0.0


@dataclass
class Geofence:
    """Circular geofence zone."""
    name: str
    lat: float
    lon: float
    radius_nm: float  # Nautical miles
    description: str = ""


def _haversine_nm(lat1: float, lon1: float, lat2: float, lon2: float) -> float:
    """Great-circle distance in nautical miles."""
    R_NM = 3440.065  # Earth radius in nautical miles
    dlat = math.radians(lat2 - lat1)
    dlon = math.radians(lon2 - lon1)
    a = (
        math.sin(dlat / 2) ** 2
        + math.cos(math.radians(lat1))
        * math.cos(math.radians(lat2))
        * math.sin(dlon / 2) ** 2
    )
    return R_NM * 2 * math.atan2(math.sqrt(a), math.sqrt(1 - a))


class FilterEngine:
    """Runs all filters against aircraft state and produces events.

    Tracks which events have already been emitted per aircraft to avoid
    duplicate alerts within a session. An event key (icao + event_type)
    is only emitted once until the condition clears and re-triggers.
    """

    def __init__(
        self,
        geofences: list[Geofence] | None = None,
        low_altitude_ft: int = LOW_ALTITUDE_THRESHOLD,
        rapid_descent_fpm: int = RAPID_DESCENT_THRESHOLD,
    ):
        self.geofences = geofences or []
        self.low_altitude_ft = low_altitude_ft
        self.rapid_descent_fpm = rapid_descent_fpm

        # Track emitted events to avoid duplicates: {(icao, event_type)}
        self._emitted: set[tuple[str, str]] = set()

    def check(self, ac: AircraftState) -> list[Event]:
        """Run all filters against an aircraft state. Returns new events."""
        events: list[Event] = []

        events.extend(self._check_military(ac))
        events.extend(self._check_emergency(ac))
        events.extend(self._check_rapid_descent(ac))
        events.extend(self._check_low_altitude(ac))
        events.extend(self._check_geofences(ac))

        return events

    def _emit(self, event: Event) -> Event | None:
        """Emit event if not already emitted for this aircraft + type."""
        key = (event.icao, event.event_type)
        if key in self._emitted:
            return None
        self._emitted.add(key)
        return event

    def clear(self, icao: str):
        """Clear emitted events for an aircraft (e.g., when pruned)."""
        self._emitted = {k for k in self._emitted if k[0] != icao}

    def _check_military(self, ac: AircraftState) -> list[Event]:
        if not ac.is_military:
            return []
        event = self._emit(Event(
            icao=ac.icao,
            event_type=EVENT_MILITARY,
            description=f"Military aircraft detected: {ac.callsign or ac.icao}",
            lat=ac.lat,
            lon=ac.lon,
            altitude_ft=ac.altitude_ft,
            timestamp=ac.last_seen,
        ))
        return [event] if event else []

    def _check_emergency(self, ac: AircraftState) -> list[Event]:
        if ac.squawk not in EMERGENCY_SQUAWKS:
            return []
        desc = EMERGENCY_SQUAWKS[ac.squawk]
        event = self._emit(Event(
            icao=ac.icao,
            event_type=EVENT_EMERGENCY,
            description=f"Squawk {ac.squawk}: {desc} — {ac.callsign or ac.icao}",
            lat=ac.lat,
            lon=ac.lon,
            altitude_ft=ac.altitude_ft,
            timestamp=ac.last_seen,
        ))
        return [event] if event else []

    def _check_rapid_descent(self, ac: AircraftState) -> list[Event]:
        if ac.vertical_rate_fpm is None:
            return []
        if ac.vertical_rate_fpm >= self.rapid_descent_fpm:
            return []
        event = self._emit(Event(
            icao=ac.icao,
            event_type=EVENT_RAPID_DESCENT,
            description=(
                f"Rapid descent {ac.vertical_rate_fpm} ft/min — "
                f"{ac.callsign or ac.icao} at {ac.altitude_ft or '?'} ft"
            ),
            lat=ac.lat,
            lon=ac.lon,
            altitude_ft=ac.altitude_ft,
            timestamp=ac.last_seen,
        ))
        return [event] if event else []

    def _check_low_altitude(self, ac: AircraftState) -> list[Event]:
        if ac.altitude_ft is None:
            return []
        if ac.altitude_ft >= self.low_altitude_ft:
            return []
        if ac.altitude_ft <= 0:
            return []  # On ground
        event = self._emit(Event(
            icao=ac.icao,
            event_type=EVENT_LOW_ALTITUDE,
            description=(
                f"Low altitude {ac.altitude_ft} ft — "
                f"{ac.callsign or ac.icao}"
            ),
            lat=ac.lat,
            lon=ac.lon,
            altitude_ft=ac.altitude_ft,
            timestamp=ac.last_seen,
        ))
        return [event] if event else []

    def _check_geofences(self, ac: AircraftState) -> list[Event]:
        if not ac.has_position:
            return []
        events = []
        for fence in self.geofences:
            dist = _haversine_nm(ac.lat, ac.lon, fence.lat, fence.lon)
            if dist <= fence.radius_nm:
                # Use fence-specific event type to allow per-fence dedup
                fence_type = f"{EVENT_GEOFENCE}:{fence.name}"
                key = (ac.icao, fence_type)
                if key in self._emitted:
                    continue
                self._emitted.add(key)
                events.append(Event(
                    icao=ac.icao,
                    event_type=EVENT_GEOFENCE,
                    description=(
                        f"Entered geofence '{fence.name}' — "
                        f"{ac.callsign or ac.icao} at {dist:.1f} nm"
                    ),
                    lat=ac.lat,
                    lon=ac.lon,
                    altitude_ft=ac.altitude_ft,
                    timestamp=ac.last_seen,
                ))
        return events
