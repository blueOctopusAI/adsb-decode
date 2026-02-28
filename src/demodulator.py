"""IQ sample demodulation — convert raw radio samples to ADS-B bitstreams.

Pipeline:
1. IQ to magnitude: sqrt(I² + Q²) per sample pair (or squared magnitude for speed)
2. Preamble detection: slide window looking for 8µs pulse pattern at positions 0,1,3.5,4.5µs
3. Bit recovery: PPM — compare first half-µs energy to second half-µs per bit period

At 2 MHz sample rate:
- 1 bit = 2 samples (1 µs per bit)
- Preamble = 16 samples (8 µs)
- Short message (56 bits) = 112 samples after preamble
- Long message (112 bits) = 224 samples after preamble
- Total window for long message = 16 + 224 = 240 samples

The demodulator is the "built from scratch" piece — no dump1090, no rtl_adsb.
Raw IQ bytes in, hex frame strings out.
"""

from __future__ import annotations

import numpy as np

from .capture import RawFrame

# At 2 MHz, each microsecond = 2 samples
SAMPLES_PER_BIT = 2
PREAMBLE_SAMPLES = 16  # 8 µs preamble
SHORT_MSG_BITS = 56
LONG_MSG_BITS = 112
SHORT_MSG_SAMPLES = SHORT_MSG_BITS * SAMPLES_PER_BIT  # 112
LONG_MSG_SAMPLES = LONG_MSG_BITS * SAMPLES_PER_BIT  # 224

# Total window needed: preamble + longest message
WINDOW_SIZE = PREAMBLE_SAMPLES + LONG_MSG_SAMPLES  # 240

# Preamble pulse positions in samples (at 2 MHz):
# Pulses at 0, 1, 3.5, 4.5 µs → samples 0, 2, 7, 9
PULSE_POSITIONS = [0, 2, 7, 9]
# Gap positions (should be low energy)
GAP_POSITIONS = [1, 3, 4, 5, 6, 8]

# Minimum ratio of pulse energy to gap energy for valid preamble
MIN_PREAMBLE_RATIO = 2.0

# Minimum signal level (squared magnitude) to consider a preamble
MIN_SIGNAL_LEVEL = 100.0


def iq_to_magnitude(raw: np.ndarray) -> np.ndarray:
    """Convert interleaved uint8 IQ pairs to squared magnitude.

    Args:
        raw: Flat uint8 array [I0, Q0, I1, Q1, ...] from RTL-SDR.

    Returns:
        Float32 array of squared magnitudes (I²+Q²), one per sample.
    """
    iq = raw.reshape(-1, 2).astype(np.float32) - 127.5
    return iq[:, 0] ** 2 + iq[:, 1] ** 2


def check_preamble(mag: np.ndarray, pos: int) -> float | None:
    """Check if a valid ADS-B preamble starts at position `pos`.

    The preamble has pulses at sample offsets 0, 2, 7, 9 and gaps elsewhere
    within the 16-sample window. We check that pulse energy significantly
    exceeds gap energy.

    Args:
        mag: Squared magnitude array.
        pos: Start index to check.

    Returns:
        Signal level (average pulse magnitude) if valid preamble, None otherwise.
    """
    if pos + WINDOW_SIZE > len(mag):
        return None

    pulse_sum = sum(mag[pos + p] for p in PULSE_POSITIONS)
    gap_sum = sum(mag[pos + g] for g in GAP_POSITIONS)

    pulse_avg = pulse_sum / len(PULSE_POSITIONS)
    gap_avg = gap_sum / len(GAP_POSITIONS) if gap_sum > 0 else 0.001

    if pulse_avg < MIN_SIGNAL_LEVEL:
        return None

    if pulse_avg / gap_avg < MIN_PREAMBLE_RATIO:
        return None

    # Additional check: all pulses should be roughly similar amplitude
    pulse_values = [mag[pos + p] for p in PULSE_POSITIONS]
    if max(pulse_values) > 6 * min(pulse_values):
        return None

    return float(pulse_avg)


def recover_bits(mag: np.ndarray, pos: int, n_bits: int) -> list[int]:
    """Recover bits from magnitude signal using PPM decoding.

    Each bit occupies 2 samples (1 µs at 2 MHz). Pulse Position Modulation:
    - Bit '1': energy in first sample > energy in second sample
    - Bit '0': energy in second sample >= energy in first sample

    Args:
        mag: Squared magnitude array.
        pos: Start index (first bit after preamble).
        n_bits: Number of bits to recover.

    Returns:
        List of 0/1 integers.
    """
    bits = []
    for i in range(n_bits):
        sample_pos = pos + i * SAMPLES_PER_BIT
        if sample_pos + 1 >= len(mag):
            break
        if mag[sample_pos] > mag[sample_pos + 1]:
            bits.append(1)
        else:
            bits.append(0)
    return bits


def bits_to_hex(bits: list[int]) -> str:
    """Convert bit list to uppercase hex string."""
    hex_chars = []
    for i in range(0, len(bits), 4):
        nibble = bits[i:i + 4]
        if len(nibble) < 4:
            break
        value = (nibble[0] << 3) | (nibble[1] << 2) | (nibble[2] << 1) | nibble[3]
        hex_chars.append(f"{value:X}")
    return "".join(hex_chars)


def demodulate_buffer(mag: np.ndarray, timestamp: float = 0.0) -> list[RawFrame]:
    """Scan a magnitude buffer for ADS-B messages.

    Slides through the buffer looking for valid preambles, then recovers
    bits and produces hex frame strings.

    Args:
        mag: Squared magnitude array from iq_to_magnitude().
        timestamp: Base timestamp for the buffer.

    Returns:
        List of RawFrame objects with hex strings.
    """
    frames = []
    i = 0
    sample_rate = 2_000_000

    while i < len(mag) - WINDOW_SIZE:
        signal_level = check_preamble(mag, i)
        if signal_level is None:
            i += 1
            continue

        # Preamble found — try to recover message bits
        msg_start = i + PREAMBLE_SAMPLES

        # Try long message first (112 bits)
        if msg_start + LONG_MSG_SAMPLES <= len(mag):
            bits = recover_bits(mag, msg_start, LONG_MSG_BITS)
            if len(bits) == LONG_MSG_BITS:
                hex_str = bits_to_hex(bits)
                if len(hex_str) == 28:
                    # Check DF — first 5 bits determine if this is valid
                    df = (bits[0] << 4) | (bits[1] << 3) | (bits[2] << 2) | (bits[3] << 1) | bits[4]
                    if df in (16, 17, 18, 19, 20, 21):
                        frame_time = timestamp + i / sample_rate
                        frames.append(RawFrame(
                            hex_str=hex_str,
                            timestamp=frame_time,
                            signal_level=signal_level,
                            source="demodulator",
                        ))
                        # Skip past this message
                        i = msg_start + LONG_MSG_SAMPLES
                        continue

        # Try short message (56 bits)
        if msg_start + SHORT_MSG_SAMPLES <= len(mag):
            bits = recover_bits(mag, msg_start, SHORT_MSG_BITS)
            if len(bits) == SHORT_MSG_BITS:
                hex_str = bits_to_hex(bits)
                if len(hex_str) == 14:
                    df = (bits[0] << 4) | (bits[1] << 3) | (bits[2] << 2) | (bits[3] << 1) | bits[4]
                    if df in (0, 4, 5, 11):
                        frame_time = timestamp + i / sample_rate
                        frames.append(RawFrame(
                            hex_str=hex_str,
                            timestamp=frame_time,
                            signal_level=signal_level,
                            source="demodulator",
                        ))
                        i = msg_start + SHORT_MSG_SAMPLES
                        continue

        # Not a valid message — advance past false preamble
        i += 1

    return frames


def demodulate_file(
    path: str,
    sample_rate: int = 2_000_000,
    chunk_samples: int = 2_000_000,  # 1 second chunks
) -> list[RawFrame]:
    """Demodulate a raw IQ file into ADS-B frames.

    Reads the file in chunks to manage memory. Each chunk overlaps
    the previous by WINDOW_SIZE samples to avoid missing frames at
    chunk boundaries.

    Args:
        path: Path to raw IQ binary file.
        sample_rate: Sample rate in Hz.
        chunk_samples: Number of IQ samples per processing chunk.

    Returns:
        List of all recovered RawFrame objects.
    """
    import os
    file_size = os.path.getsize(path)
    total_samples = file_size // 2  # 2 bytes per IQ pair

    all_frames = []
    overlap = WINDOW_SIZE
    offset = 0

    while offset < total_samples:
        # Read chunk of raw bytes
        byte_offset = offset * 2
        byte_count = min(chunk_samples * 2, file_size - byte_offset)
        raw = np.fromfile(path, dtype=np.uint8, count=byte_count, offset=byte_offset)

        if len(raw) < WINDOW_SIZE * 2:
            break

        mag = iq_to_magnitude(raw)
        chunk_time = offset / sample_rate
        frames = demodulate_buffer(mag, timestamp=chunk_time)
        all_frames.extend(frames)

        # Advance by chunk size minus overlap
        offset += chunk_samples - overlap

    return all_frames
