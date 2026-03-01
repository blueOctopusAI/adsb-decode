"""CRC-24 validation for Mode S messages.

ICAO standard polynomial: x^24 + x^23 + x^22 + ... + x^10 + x^3 + 1
Generator: 0xFFF409

For DF17/18 (ADS-B): last 24 bits are pure CRC. Valid frames -> remainder 0x000000.
For DF0/4/5/16/20/21: last 24 bits are CRC XORed with ICAO address.

Mode S CRC algorithm: polynomial division of the first (n-3) bytes (data portion),
then XOR with the last 3 bytes (PI/CRC field). This is NOT standard CRC-24 —
the PI field is not processed through the polynomial division.
"""

GENERATOR = 0xFFF409


def _build_crc_table() -> list[int]:
    """Pre-compute 256-entry CRC-24 lookup table for byte-at-a-time processing."""
    table = []
    for i in range(256):
        crc = i << 16
        for _ in range(8):
            if crc & 0x800000:
                crc = (crc << 1) ^ GENERATOR
            else:
                crc = crc << 1
        table.append(crc & 0xFFFFFF)
    return table


_CRC_TABLE = _build_crc_table()


def _crc24_raw(data: bytes) -> int:
    """Pure CRC-24 polynomial division of all bytes.

    Used for computing CRC of payload data and building syndrome tables.
    NOT the Mode S CRC check (use crc24() for that).
    """
    crc = 0
    for byte in data:
        crc = ((crc << 8) ^ _CRC_TABLE[((crc >> 16) ^ byte) & 0xFF]) & 0xFFFFFF
    return crc


def crc24(data: bytes) -> int:
    """Mode S CRC-24 check.

    Performs polynomial division of the first (n-3) bytes (data portion),
    then XOR with the last 3 bytes (PI/CRC field).

    For DF17/18: returns 0 when valid (PI = CRC of data).
    For DF0/4/5/16/20/21: returns ICAO address (PI = CRC XOR ICAO).

    Byte-at-a-time table lookup — 8x faster than bit-by-bit.
    """
    if len(data) <= 3:
        return int.from_bytes(data, "big") & 0xFFFFFF

    # Polynomial division of data portion (all except last 3 bytes)
    crc = 0
    for byte in data[:-3]:
        crc = ((crc << 8) ^ _CRC_TABLE[((crc >> 16) ^ byte) & 0xFF]) & 0xFFFFFF

    # XOR with PI field (last 3 bytes)
    crc ^= (data[-3] << 16) | (data[-2] << 8) | data[-1]
    return crc


def crc24_payload(data: bytes) -> int:
    """Compute CRC-24 of the payload bytes (all except last 3 CRC bytes).

    Uses raw polynomial division (no XOR with PI field).
    Useful for computing the expected CRC to compare against the transmitted CRC.
    """
    return _crc24_raw(data[:-3])


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


def _build_syndrome_table(n_bits: int) -> dict[int, list[int]]:
    """Build syndrome-to-bit-position lookup table for error correction.

    For each possible 1-bit and 2-bit error in an n_bits message,
    compute the CRC syndrome (non-zero remainder) and map it to the
    corrupted bit position(s). This allows O(1) error correction when
    CRC fails — just look up the syndrome to find which bits to flip.

    Uses the Mode S CRC (crc24) so syndromes match what we get from
    corrupted real messages.

    Args:
        n_bits: Message length in bits (112 for long, 56 for short).

    Returns:
        Dict mapping syndrome -> list of bit positions to flip.
    """
    n_bytes = n_bits // 8
    table: dict[int, list[int]] = {}

    # Single-bit errors
    for bit in range(n_bits):
        msg = bytearray(n_bytes)
        msg[bit // 8] |= 1 << (7 - (bit % 8))
        syndrome = crc24(bytes(msg))
        if syndrome not in table:
            table[syndrome] = [bit]

    # Double-bit errors
    for bit1 in range(n_bits):
        for bit2 in range(bit1 + 1, n_bits):
            msg = bytearray(n_bytes)
            msg[bit1 // 8] |= 1 << (7 - (bit1 % 8))
            msg[bit2 // 8] |= 1 << (7 - (bit2 % 8))
            syndrome = crc24(bytes(msg))
            if syndrome not in table:
                table[syndrome] = [bit1, bit2]

    return table


_SYNDROME_TABLE_112 = _build_syndrome_table(112)
_SYNDROME_TABLE_56 = _build_syndrome_table(56)


def try_fix(msg_hex: str) -> str | None:
    """Attempt to correct 1-2 bit errors in a Mode S message.

    Looks up the CRC syndrome in pre-built tables. If found, flips the
    identified bits and re-validates. Never corrects bits 0-4 (DF field)
    to avoid turning one message type into another.

    Args:
        msg_hex: Hex-encoded Mode S message that failed CRC.

    Returns:
        Corrected hex string if fixable, None otherwise.
    """
    data = bytes.fromhex(msg_hex)
    n_bits = len(data) * 8
    syndrome = crc24(data)

    if syndrome == 0:
        return msg_hex  # Already valid

    table = _SYNDROME_TABLE_112 if n_bits == 112 else _SYNDROME_TABLE_56
    if syndrome not in table:
        return None

    bit_positions = table[syndrome]

    # Safety: never correct the DF field (bits 0-4)
    if any(b < 5 for b in bit_positions):
        return None

    # Flip the identified bits
    fixed = bytearray(data)
    for bit in bit_positions:
        fixed[bit // 8] ^= 1 << (7 - (bit % 8))

    # Verify the fix actually works
    if crc24(bytes(fixed)) != 0:
        return None

    return fixed.hex().upper()
