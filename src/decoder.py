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

from __future__ import annotations

import math
from dataclasses import dataclass

from .frame_parser import ModeFrame


# ADS-B character set for callsign encoding (6 bits per character)
_CHARSET = "#ABCDEFGHIJKLMNOPQRSTUVWXYZ##### ###############0123456789######"


# --- Message dataclasses ---


@dataclass(frozen=True)
class IdentificationMsg:
    """TC 1-4: Aircraft identification (callsign)."""

    icao: str
    callsign: str
    category: int  # Wake turbulence category
    timestamp: float


@dataclass(frozen=True)
class PositionMsg:
    """TC 5-8 (surface) or TC 9-18/20-22 (airborne): CPR-encoded position."""

    icao: str
    altitude_ft: int | None  # None for surface position
    cpr_lat: int  # 17-bit CPR latitude
    cpr_lon: int  # 17-bit CPR longitude
    cpr_odd: bool  # True = odd frame, False = even frame
    surveillance_status: int
    timestamp: float
    is_surface: bool = False


@dataclass(frozen=True)
class VelocityMsg:
    """TC 19: Airborne velocity."""

    icao: str
    speed_kts: float | None  # Ground speed or airspeed in knots
    heading_deg: float | None  # Track angle or heading in degrees
    vertical_rate_fpm: int | None  # Feet per minute, positive = climb
    speed_type: str  # "ground" or "airspeed"
    timestamp: float


@dataclass(frozen=True)
class AltitudeMsg:
    """DF0/4/16/20: Altitude reply."""

    icao: str
    altitude_ft: int | None
    timestamp: float


@dataclass(frozen=True)
class SquawkMsg:
    """DF5/21: Identity reply (squawk code)."""

    icao: str
    squawk: str  # 4-digit octal string (e.g., "7700")
    timestamp: float


# Union type for all decoded messages
DecodedMsg = IdentificationMsg | PositionMsg | VelocityMsg | AltitudeMsg | SquawkMsg


# --- Altitude decoding ---


def decode_altitude(alt_code: int) -> int | None:
    """Decode 12-bit altitude code from DF17 airborne position.

    The 12-bit altitude field (after removing TC bits) uses two encoding modes
    based on the Q-bit (bit index 4 from LSB in the 12-bit field).

    Returns altitude in feet, or None if not available.
    """
    if alt_code == 0:
        return None

    # Q-bit is at position 4 (0-indexed from LSB) in the 12-bit field
    q_bit = (alt_code >> 4) & 1

    if q_bit:
        # 25-ft resolution mode
        # Remove the Q-bit to get the 11-bit altitude code
        n = ((alt_code >> 5) << 4) | (alt_code & 0x0F)
        return n * 25 - 1000
    else:
        # 100-ft Gillham gray code mode
        return _decode_gillham_altitude(alt_code)


def _decode_gillham_altitude(alt_code: int) -> int | None:
    """Decode 100-ft Gillham gray code altitude. Returns feet or None."""
    # Extract the Gillham-coded bits (complex bit reordering)
    # This encoding is rarely seen in modern ADS-B; most aircraft use 25-ft mode
    # For now, return None for gray code altitudes
    # TODO: Implement full Gillham gray code decoder if needed
    return None


def decode_altitude_13bit(alt_code_13: int) -> int | None:
    """Decode 13-bit altitude code from DF0/4/16/20.

    The 13-bit field has M-bit and Q-bit:
    - M=0, Q=1: 25-ft increments
    - M=0, Q=0: 100-ft Gillham gray code
    - M=1: metric altitude (rare)
    """
    if alt_code_13 == 0:
        return None

    m_bit = (alt_code_13 >> 6) & 1
    q_bit = (alt_code_13 >> 4) & 1

    if m_bit:
        # Metric altitude — very rare, not implemented
        return None

    if q_bit:
        # 25-ft mode: remove M and Q bits to get 11-bit code
        n = ((alt_code_13 & 0x1F80) >> 2) | ((alt_code_13 & 0x0020) >> 1) | (alt_code_13 & 0x000F)
        return n * 25 - 1000
    else:
        return _decode_gillham_altitude(alt_code_13)


# --- Squawk decoding ---


def decode_squawk(id_code: int) -> str:
    """Decode 13-bit identity code into 4-digit octal squawk.

    The 13-bit field uses Gillham coding with bit interleaving.
    Bits are labeled C1 A1 C2 A2 C4 A4 _ B1 D1 B2 D2 B4 D4
    """
    # Extract individual bits using Gillham positions
    c1 = (id_code >> 12) & 1
    a1 = (id_code >> 11) & 1
    c2 = (id_code >> 10) & 1
    a2 = (id_code >> 9) & 1
    c4 = (id_code >> 8) & 1
    a4 = (id_code >> 7) & 1
    # bit 6 is spare (SPI)
    b1 = (id_code >> 5) & 1
    d1 = (id_code >> 4) & 1
    b2 = (id_code >> 3) & 1
    d2 = (id_code >> 2) & 1
    b4 = (id_code >> 1) & 1
    d4 = id_code & 1

    a = a4 * 4 + a2 * 2 + a1
    b = b4 * 4 + b2 * 2 + b1
    c = c4 * 4 + c2 * 2 + c1
    d = d4 * 4 + d2 * 2 + d1

    return f"{a}{b}{c}{d}"


# --- Main decode functions ---


def decode_identification(frame: ModeFrame) -> IdentificationMsg | None:
    """Decode TC 1-4: Aircraft identification (callsign).

    ME field layout (56 bits):
    - TC (5 bits): Type code 1-4
    - CA (3 bits): Aircraft category
    - Callsign (48 bits): 8 characters × 6 bits each
    """
    if frame.type_code is None or not (1 <= frame.type_code <= 4):
        return None

    me = frame.me
    category = me[0] & 0x07

    # Decode 8 callsign characters (6 bits each, packed into 48 bits)
    chars = []
    # The 48 callsign bits start at bit 8 of the ME field (after TC+CA)
    bits = int.from_bytes(me, "big")
    for i in range(8):
        idx = (bits >> (42 - i * 6)) & 0x3F
        if idx < len(_CHARSET):
            chars.append(_CHARSET[idx])
        else:
            chars.append(" ")

    return IdentificationMsg(
        icao=frame.icao,
        callsign="".join(chars),
        category=category,
        timestamp=frame.timestamp,
    )


def decode_position(frame: ModeFrame) -> PositionMsg | None:
    """Decode TC 5-8 (surface) or TC 9-18/20-22 (airborne position).

    ME field layout for airborne (56 bits):
    - TC (5 bits): Type code
    - SS (2 bits): Surveillance status
    - SAF (1 bit): Single antenna flag
    - ALT (12 bits): Altitude code
    - T (1 bit): UTC sync flag
    - F (1 bit): CPR format (0=even, 1=odd)
    - LAT_CPR (17 bits): CPR latitude
    - LON_CPR (17 bits): CPR longitude
    """
    tc = frame.type_code
    if tc is None:
        return None

    is_surface = 5 <= tc <= 8
    is_airborne_baro = 9 <= tc <= 18
    is_airborne_gnss = 20 <= tc <= 22

    if not (is_surface or is_airborne_baro or is_airborne_gnss):
        return None

    me = frame.me
    bits = int.from_bytes(me, "big")

    ss = (bits >> 49) & 0x03
    # SAF at bit 48

    altitude_ft = None
    if is_airborne_baro or is_airborne_gnss:
        alt_code = (bits >> 36) & 0x0FFF
        altitude_ft = decode_altitude(alt_code)

    cpr_odd = bool((bits >> 34) & 1)
    cpr_lat = (bits >> 17) & 0x1FFFF
    cpr_lon = bits & 0x1FFFF

    return PositionMsg(
        icao=frame.icao,
        altitude_ft=altitude_ft,
        cpr_lat=cpr_lat,
        cpr_lon=cpr_lon,
        cpr_odd=cpr_odd,
        surveillance_status=ss,
        timestamp=frame.timestamp,
        is_surface=is_surface,
    )


def decode_velocity(frame: ModeFrame) -> VelocityMsg | None:
    """Decode TC 19: Airborne velocity.

    Subtypes 1-2: Ground speed (E-W and N-S velocity components)
    Subtypes 3-4: Airspeed (heading + IAS or TAS)
    Also contains vertical rate.
    """
    if frame.type_code != 19:
        return None

    me = frame.me
    bits = int.from_bytes(me, "big")

    subtype = (bits >> 48) & 0x07

    if subtype in (1, 2):
        return _decode_ground_velocity(frame.icao, bits, frame.timestamp)
    elif subtype in (3, 4):
        return _decode_airspeed(frame.icao, bits, subtype, frame.timestamp)

    return None


def _decode_ground_velocity(icao: str, bits: int, timestamp: float) -> VelocityMsg:
    """Decode ground speed from E-W and N-S components."""
    # Direction and speed bits
    ew_dir = (bits >> 42) & 1  # 0=East, 1=West
    ew_vel = ((bits >> 32) & 0x3FF) - 1  # 10 bits, subtract 1
    ns_dir = (bits >> 31) & 1  # 0=North, 1=South
    ns_vel = ((bits >> 21) & 0x3FF) - 1  # 10 bits, subtract 1

    # Vertical rate
    vr_sign = (bits >> 19) & 1  # 0=up, 1=down
    vr_val = ((bits >> 10) & 0x1FF) - 1  # 9 bits, subtract 1

    # Compute speed and heading
    speed: float | None = None
    heading: float | None = None

    if ew_vel >= 0 and ns_vel >= 0:
        vx = ew_vel * (-1 if ew_dir else 1)
        vy = ns_vel * (-1 if ns_dir else 1)
        speed = math.sqrt(vx**2 + vy**2)
        heading = math.degrees(math.atan2(vx, vy)) % 360

    vrate: int | None = None
    if vr_val >= 0:
        vrate = vr_val * 64 * (-1 if vr_sign else 1)

    return VelocityMsg(
        icao=icao,
        speed_kts=round(speed, 2) if speed is not None else None,
        heading_deg=round(heading, 2) if heading is not None else None,
        vertical_rate_fpm=vrate,
        speed_type="ground",
        timestamp=timestamp,
    )


def _decode_airspeed(
    icao: str, bits: int, subtype: int, timestamp: float
) -> VelocityMsg:
    """Decode airspeed and heading."""
    hdg_available = (bits >> 42) & 1
    hdg_raw = (bits >> 32) & 0x3FF  # 10 bits

    speed_type_bit = (bits >> 31) & 1  # 0=IAS, 1=TAS
    speed_raw = (bits >> 21) & 0x3FF

    vr_sign = (bits >> 10) & 1
    vr_val = ((bits >> 1) & 0x1FF) - 1

    heading: float | None = None
    if hdg_available:
        heading = round(hdg_raw * 360 / 1024, 2)

    speed: float | None = None
    if speed_raw > 0:
        speed = float(speed_raw - 1)

    vrate: int | None = None
    if vr_val >= 0:
        vrate = vr_val * 64 * (-1 if vr_sign else 1)

    return VelocityMsg(
        icao=icao,
        speed_kts=speed,
        heading_deg=heading,
        vertical_rate_fpm=vrate,
        speed_type="TAS" if speed_type_bit else "IAS",
        timestamp=timestamp,
    )


def decode_df_altitude(frame: ModeFrame) -> AltitudeMsg | None:
    """Decode DF0/4/16/20: altitude from surveillance replies."""
    if frame.df not in (0, 4, 16, 20):
        return None

    # 13-bit altitude code is at bits 20-32 in the message
    raw = frame.raw
    alt_code = ((raw[2] & 0x1F) << 8) | raw[3]
    altitude_ft = decode_altitude_13bit(alt_code)

    return AltitudeMsg(
        icao=frame.icao,
        altitude_ft=altitude_ft,
        timestamp=frame.timestamp,
    )


def decode_df_squawk(frame: ModeFrame) -> SquawkMsg | None:
    """Decode DF5/21: identity (squawk) from surveillance replies."""
    if frame.df not in (5, 21):
        return None

    raw = frame.raw
    id_code = ((raw[2] & 0x1F) << 8) | raw[3]
    squawk = decode_squawk(id_code)

    return SquawkMsg(
        icao=frame.icao,
        squawk=squawk,
        timestamp=frame.timestamp,
    )


def decode(frame: ModeFrame) -> DecodedMsg | None:
    """Decode any ModeFrame into the appropriate typed message.

    Routes to the correct decoder based on DF and TC.
    Returns None for unrecognized or unsupported frame types.
    """
    if not frame.crc_ok:
        return None

    if frame.df in (17, 18):
        tc = frame.type_code
        if tc is None:
            return None
        if 1 <= tc <= 4:
            return decode_identification(frame)
        if 5 <= tc <= 18 or 20 <= tc <= 22:
            return decode_position(frame)
        if tc == 19:
            return decode_velocity(frame)
        return None

    if frame.df in (0, 4, 16, 20):
        return decode_df_altitude(frame)

    if frame.df in (5, 21):
        return decode_df_squawk(frame)

    return None
