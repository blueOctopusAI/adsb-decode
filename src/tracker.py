"""Per-aircraft state machine with CPR frame pairing.

Maintains a dictionary of AircraftState objects keyed by ICAO address.
Each state tracks:
- Current position (lat, lon, altitude) from latest decode
- Current velocity (ground speed, heading, vertical rate)
- Callsign and squawk code
- CPR buffer (last even and odd position frames for global decode)
- Timestamps for age/staleness detection

Feeds decoded messages to the database and runs filter checks on each update.
Aircraft are considered stale after 60 seconds of no messages.
"""

from __future__ import annotations

import time
from dataclasses import dataclass, field

from . import cpr, icao
from .database import Database
from .decoder import (
    DecodedMsg,
    IdentificationMsg,
    PositionMsg,
    VelocityMsg,
    AltitudeMsg,
    SquawkMsg,
    decode,
)
from .frame_parser import ModeFrame

# Aircraft considered stale after this many seconds of silence
STALE_TIMEOUT = 60.0


@dataclass
class AircraftState:
    """Mutable state for a single tracked aircraft."""

    icao: str
    callsign: str | None = None
    squawk: str | None = None

    # Position
    lat: float | None = None
    lon: float | None = None
    altitude_ft: int | None = None

    # Velocity
    speed_kts: float | None = None
    heading_deg: float | None = None
    vertical_rate_fpm: int | None = None

    # CPR buffer for global decode
    cpr_even_lat: int | None = None
    cpr_even_lon: int | None = None
    cpr_even_time: float = 0.0
    cpr_odd_lat: int | None = None
    cpr_odd_lon: int | None = None
    cpr_odd_time: float = 0.0

    # Metadata
    country: str | None = None
    registration: str | None = None
    is_military: bool = False
    first_seen: float = 0.0
    last_seen: float = 0.0
    message_count: int = 0

    # History buffers for pattern detection
    heading_history: list = field(default_factory=list)  # [(timestamp, heading_deg)]
    position_history: list = field(default_factory=list)  # [(timestamp, lat, lon, alt)]

    # Max history entries to keep (rolling buffer)
    _MAX_HISTORY: int = field(default=120, repr=False)

    @property
    def has_position(self) -> bool:
        return self.lat is not None and self.lon is not None

    @property
    def age(self) -> float:
        """Seconds since last message."""
        return time.time() - self.last_seen if self.last_seen else float("inf")

    @property
    def is_stale(self) -> bool:
        return self.age > STALE_TIMEOUT


class Tracker:
    """Track multiple aircraft from decoded messages.

    Maintains per-aircraft state, pairs CPR frames for position decode,
    and optionally persists to database.
    """

    def __init__(
        self,
        db: Database | None = None,
        receiver_id: int | None = None,
        capture_id: int | None = None,
        ref_lat: float | None = None,
        ref_lon: float | None = None,
    ):
        """Initialize tracker.

        Args:
            db: Optional database for persistence.
            receiver_id: Receiver ID for tagging positions.
            capture_id: Current capture session ID.
            ref_lat: Receiver latitude for local CPR decode.
            ref_lon: Receiver longitude for local CPR decode.
        """
        self.aircraft: dict[str, AircraftState] = {}
        self.db = db
        self.receiver_id = receiver_id
        self.capture_id = capture_id
        self.ref_lat = ref_lat
        self.ref_lon = ref_lon

        # Counters
        self.total_frames = 0
        self.valid_frames = 0
        self.position_decodes = 0

    def _get_or_create(self, icao_addr: str, timestamp: float) -> AircraftState:
        """Get existing aircraft state or create new one."""
        if icao_addr not in self.aircraft:
            country = icao.lookup_country(icao_addr)
            registration = icao.icao_to_n_number(icao_addr)
            military = icao.is_military(icao_addr)

            state = AircraftState(
                icao=icao_addr,
                country=country,
                registration=registration,
                is_military=military,
                first_seen=timestamp,
                last_seen=timestamp,
            )
            self.aircraft[icao_addr] = state

            # Persist new aircraft
            if self.db:
                self.db.upsert_aircraft(
                    icao_addr,
                    country=country,
                    registration=registration,
                    is_military=military,
                    timestamp=timestamp,
                )
        return self.aircraft[icao_addr]

    def update(self, frame: ModeFrame) -> DecodedMsg | None:
        """Process a single parsed frame through the tracker.

        Decodes the frame, updates aircraft state, attempts CPR position
        resolve, and persists to database if configured.

        Returns the decoded message, or None if decode failed.
        """
        self.total_frames += 1

        msg = decode(frame)
        if msg is None:
            return None

        self.valid_frames += 1
        ac = self._get_or_create(msg.icao, frame.timestamp)
        ac.last_seen = frame.timestamp
        ac.message_count += 1

        # Update military flag from callsign if needed
        if hasattr(msg, "callsign") and not ac.is_military:
            ac.is_military = icao.is_military(ac.icao, getattr(msg, "callsign", None))

        if isinstance(msg, IdentificationMsg):
            self._handle_identification(ac, msg)
        elif isinstance(msg, PositionMsg):
            self._handle_position(ac, msg)
        elif isinstance(msg, VelocityMsg):
            self._handle_velocity(ac, msg)
        elif isinstance(msg, AltitudeMsg):
            self._handle_altitude(ac, msg)
        elif isinstance(msg, SquawkMsg):
            self._handle_squawk(ac, msg)

        # Update sighting in DB
        if self.db:
            self.db.upsert_aircraft(
                ac.icao, timestamp=frame.timestamp
            )
            self.db.upsert_sighting(
                icao=ac.icao,
                capture_id=self.capture_id,
                callsign=ac.callsign,
                squawk=ac.squawk,
                altitude_ft=ac.altitude_ft,
                timestamp=frame.timestamp,
            )

        return msg

    def _handle_identification(self, ac: AircraftState, msg: IdentificationMsg):
        ac.callsign = msg.callsign.strip() or ac.callsign
        # Re-check military status with callsign
        if not ac.is_military and ac.callsign:
            ac.is_military = icao.is_military(ac.icao, ac.callsign)

    def _handle_position(self, ac: AircraftState, msg: PositionMsg):
        if msg.altitude_ft is not None:
            ac.altitude_ft = msg.altitude_ft

        # Store CPR frame
        if msg.cpr_odd:
            ac.cpr_odd_lat = msg.cpr_lat
            ac.cpr_odd_lon = msg.cpr_lon
            ac.cpr_odd_time = msg.timestamp
        else:
            ac.cpr_even_lat = msg.cpr_lat
            ac.cpr_even_lon = msg.cpr_lon
            ac.cpr_even_time = msg.timestamp

        # Attempt position decode
        position = self._try_cpr_decode(ac)
        if position:
            ac.lat, ac.lon = position
            self.position_decodes += 1

            # Record position for pattern detection
            ac.position_history.append((msg.timestamp, ac.lat, ac.lon, ac.altitude_ft))
            if len(ac.position_history) > ac._MAX_HISTORY:
                ac.position_history = ac.position_history[-ac._MAX_HISTORY:]

            if self.db:
                self.db.add_position(
                    icao=ac.icao,
                    lat=ac.lat,
                    lon=ac.lon,
                    altitude_ft=ac.altitude_ft,
                    speed_kts=ac.speed_kts,
                    heading_deg=ac.heading_deg,
                    vertical_rate_fpm=ac.vertical_rate_fpm,
                    receiver_id=self.receiver_id,
                    timestamp=msg.timestamp,
                )

    def _try_cpr_decode(self, ac: AircraftState) -> tuple[float, float] | None:
        """Try to decode position from CPR frames.

        Attempts global decode first (needs even+odd pair).
        Falls back to local decode if reference position available.
        """
        # Try global decode if we have both even and odd
        if (
            ac.cpr_even_lat is not None
            and ac.cpr_odd_lat is not None
        ):
            result = cpr.global_decode(
                lat_even=ac.cpr_even_lat,
                lon_even=ac.cpr_even_lon,
                lat_odd=ac.cpr_odd_lat,
                lon_odd=ac.cpr_odd_lon,
                t_even=ac.cpr_even_time,
                t_odd=ac.cpr_odd_time,
            )
            if result:
                return result

        # Try local decode with reference position
        ref_lat = self.ref_lat
        ref_lon = self.ref_lon

        # If no receiver reference, use last known position
        if ref_lat is None and ac.lat is not None:
            ref_lat = ac.lat
            ref_lon = ac.lon

        if ref_lat is not None and ref_lon is not None:
            # Use the most recent CPR frame
            if ac.cpr_odd_time >= ac.cpr_even_time and ac.cpr_odd_lat is not None:
                return cpr.local_decode(
                    ac.cpr_odd_lat, ac.cpr_odd_lon, True, ref_lat, ref_lon
                )
            elif ac.cpr_even_lat is not None:
                return cpr.local_decode(
                    ac.cpr_even_lat, ac.cpr_even_lon, False, ref_lat, ref_lon
                )

        return None

    def _handle_velocity(self, ac: AircraftState, msg: VelocityMsg):
        if msg.speed_kts is not None:
            ac.speed_kts = msg.speed_kts
        if msg.heading_deg is not None:
            ac.heading_deg = msg.heading_deg
            # Record heading for circling detection
            ac.heading_history.append((msg.timestamp, msg.heading_deg))
            if len(ac.heading_history) > ac._MAX_HISTORY:
                ac.heading_history = ac.heading_history[-ac._MAX_HISTORY:]
        if msg.vertical_rate_fpm is not None:
            ac.vertical_rate_fpm = msg.vertical_rate_fpm

    def _handle_altitude(self, ac: AircraftState, msg: AltitudeMsg):
        if msg.altitude_ft is not None:
            ac.altitude_ft = msg.altitude_ft

    def _handle_squawk(self, ac: AircraftState, msg: SquawkMsg):
        ac.squawk = msg.squawk

    def get_active(self) -> list[AircraftState]:
        """Return all non-stale aircraft, sorted by last seen."""
        return sorted(
            (ac for ac in self.aircraft.values() if not ac.is_stale),
            key=lambda ac: ac.last_seen,
            reverse=True,
        )

    def prune_stale(self) -> int:
        """Remove stale aircraft from tracking. Returns count removed."""
        stale = [k for k, v in self.aircraft.items() if v.is_stale]
        for k in stale:
            del self.aircraft[k]
        return len(stale)
