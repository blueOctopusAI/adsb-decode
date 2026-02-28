"""CRC-24 validation for Mode S messages.

ICAO standard polynomial: x^24 + x^23 + x^22 + ... + x^10 + x^3 + 1
Generator: 0xFFF409

For DF17/18 (ADS-B): last 24 bits are pure CRC. Valid frames -> remainder 0x000000.
For DF0/4/5/16/20/21: last 24 bits are CRC XORed with ICAO address.
"""

GENERATOR = 0xFFF409


def crc24(data: bytes) -> int:
    """Compute CRC-24 remainder using ICAO Mode S polynomial.

    Processes the full message bytes through polynomial long division in GF(2).
    Returns the 24-bit remainder.
    """
    n_bytes = len(data)
    msg = int.from_bytes(data, "big")
    bits = n_bytes * 8

    for i in range(bits - 24):
        if msg & (1 << (bits - 1 - i)):
            msg ^= GENERATOR << (bits - 24 - 1 - i)

    return msg & 0xFFFFFF


def validate(msg_hex: str) -> bool:
    """Check if a Mode S message has valid CRC.

    For DF17/18 frames, the last 24 bits are pure CRC.
    Computing CRC over the entire message yields 0 when valid.
    """
    data = bytes.fromhex(msg_hex)
    return crc24(data) == 0


def residual(msg_hex: str) -> int:
    """Get CRC residual of a full Mode S message.

    For DF17/18: returns 0 if valid.
    For DF0/4/5/16/20/21: returns the ICAO address (CRC XOR'd with address).
    """
    data = bytes.fromhex(msg_hex)
    return crc24(data)


def extract_icao(msg_hex: str) -> str | None:
    """Extract ICAO address from a Mode S message.

    For DF11/17/18: ICAO is explicitly in bytes 1-3.
    For DF0/4/5/16/20/21: ICAO is recovered from the CRC residual.

    Returns 6-char uppercase hex string, or None if DF is unrecognized.
    """
    data = bytes.fromhex(msg_hex)
    df = (data[0] >> 3) & 0x1F

    if df in (11, 17, 18):
        # ICAO address is in bytes 1-3
        return f"{data[1]:02X}{data[2]:02X}{data[3]:02X}"
    elif df in (0, 4, 5, 16, 20, 21):
        # ICAO recovered from CRC residual
        icao = crc24(data)
        return f"{icao:06X}"
    else:
        return None
