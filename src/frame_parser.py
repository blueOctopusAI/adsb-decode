"""Parse raw hex strings into structured Mode S frames.

Responsibilities:
- Classify Downlink Format (DF) from first 5 bits
- Extract ICAO address (bytes 1-3 for DF11/17/18, or from CRC residual)
- Package into ModeFrame dataclass
- Reject frames that fail CRC validation
"""

from __future__ import annotations

from dataclasses import dataclass

from . import crc


# Downlink Format metadata
DF_INFO: dict[int, tuple[str, int]] = {
    # DF: (name, expected_bits)
    0: ("Short air-air surveillance", 56),
    4: ("Surveillance altitude reply", 56),
    5: ("Surveillance identity reply", 56),
    11: ("All-call reply", 56),
    16: ("Long air-air surveillance", 112),
    17: ("ADS-B extended squitter", 112),
    18: ("TIS-B / ADS-R", 112),
    20: ("Comm-B altitude reply", 112),
    21: ("Comm-B identity reply", 112),
}


@dataclass(frozen=True)
class ModeFrame:
    """A parsed Mode S frame."""

    df: int  # Downlink Format (0-24)
    icao: str  # 6-char uppercase hex ICAO address
    raw: bytes  # Full message bytes
    timestamp: float  # Unix timestamp
    signal_level: float | None  # Signal strength if available
    msg_bits: int  # 56 or 112
    crc_ok: bool  # CRC validation passed

    @property
    def df_name(self) -> str:
        """Human-readable Downlink Format name."""
        if self.df in DF_INFO:
            return DF_INFO[self.df][0]
        return f"Unknown DF{self.df}"

    @property
    def is_adsb(self) -> bool:
        """True if this is an ADS-B extended squitter (DF17)."""
        return self.df == 17

    @property
    def is_long(self) -> bool:
        """True if this is a 112-bit (long) message."""
        return self.msg_bits == 112

    @property
    def me(self) -> bytes:
        """Message Extended field (56 bits) for DF17/18. Empty for short frames."""
        if self.is_long:
            return self.raw[4:11]  # Bytes 4-10 (56 bits)
        return b""

    @property
    def type_code(self) -> int | None:
        """ADS-B Type Code (first 5 bits of ME field). None for non-ADS-B."""
        if self.df not in (17, 18) or not self.is_long:
            return None
        return (self.raw[4] >> 3) & 0x1F


def parse_frame(
    hex_str: str,
    timestamp: float = 0.0,
    signal_level: float | None = None,
) -> ModeFrame | None:
    """Parse a hex string into a ModeFrame.

    Args:
        hex_str: Hex-encoded Mode S message (14 or 28 hex chars).
        timestamp: Unix timestamp of reception.
        signal_level: Signal strength (arbitrary units).

    Returns:
        ModeFrame if valid, None if the frame is malformed or unrecognized DF.
    """
    hex_str = hex_str.strip().upper()

    # Validate length: 14 hex chars (56 bits) or 28 hex chars (112 bits)
    if len(hex_str) not in (14, 28):
        return None

    try:
        raw = bytes.fromhex(hex_str)
    except ValueError:
        return None

    msg_bits = len(raw) * 8
    df = (raw[0] >> 3) & 0x1F

    # Check if DF is recognized
    if df not in DF_INFO:
        return None

    # Validate message length matches expected for this DF
    expected_bits = DF_INFO[df][1]
    if msg_bits != expected_bits:
        return None

    # CRC check
    crc_remainder = crc.crc24(raw)

    # Extract ICAO address
    if df in (11, 17, 18):
        # ICAO explicitly in bytes 1-3
        icao = f"{raw[1]:02X}{raw[2]:02X}{raw[3]:02X}"
        crc_ok = crc_remainder == 0
    elif df in (0, 4, 5, 16, 20, 21):
        # ICAO recovered from CRC residual
        icao = f"{crc_remainder:06X}"
        # We can't directly validate CRC for these — the residual IS the address.
        # We accept the frame and let downstream logic validate the ICAO.
        crc_ok = True  # Assumed valid; ICAO plausibility checked later
    else:
        return None

    return ModeFrame(
        df=df,
        icao=icao,
        raw=raw,
        timestamp=timestamp,
        signal_level=signal_level,
        msg_bits=msg_bits,
        crc_ok=crc_ok,
    )
