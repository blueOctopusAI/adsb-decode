"""IQ sample demodulation — convert raw radio samples to ADS-B bitstreams.

Pipeline:
1. IQ to magnitude: lookup table for (I-127.5)² + (Q-127.5)² per sample pair
2. Preamble detection: slide window with strict ordering, quiet zone, SNR check
3. Bit recovery: PPM with continuity check for weak transitions + confidence tracking
4. Adaptive signal threshold: noise floor tracking via exponential moving average

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

# Quiet zone: samples 10-15 (post-preamble, pre-data) should be low
QUIET_ZONE_POSITIONS = [10, 11, 12, 13, 14, 15]

# Minimum ratio of pulse energy to gap energy for valid preamble
MIN_PREAMBLE_RATIO = 2.0

# Minimum signal level (squared magnitude) to consider a preamble
MIN_SIGNAL_LEVEL = 100.0

# SNR threshold: signal * 2 >= 3 * noise  (3.5 dB minimum)
SNR_SIGNAL_FACTOR = 2
SNR_NOISE_FACTOR = 3

# Bit recovery: minimum magnitude delta to make a confident bit decision
BIT_DELTA_THRESHOLD = 0.15

# Maximum fraction of uncertain bits before rejecting a frame
MAX_UNCERTAIN_RATIO = 0.20

# Adaptive threshold: noise floor EMA decay factor (0 < alpha < 1)
NOISE_FLOOR_ALPHA = 0.05
# Multiplier applied to noise floor to get adaptive threshold
SNR_ADAPTIVE_FACTOR = 3.0
# Absolute floor — adaptive threshold can never go below this
MIN_ADAPTIVE_LEVEL = 50.0


# --- Phase 7: Magnitude Lookup Table ---

def _build_mag_lut() -> np.ndarray:
    """Pre-compute squared magnitude for all 256x256 IQ combinations.

    Returns a (256, 256) float32 array where lut[I][Q] = (I-127.5)² + (Q-127.5)².
    Replaces per-sample arithmetic with numpy fancy indexing for ~2-3x speedup.
    """
    vals = np.arange(256, dtype=np.float32) - 127.5
    # Outer product: (I-127.5)² + (Q-127.5)²
    return vals[:, np.newaxis] ** 2 + vals[np.newaxis, :] ** 2


_MAG_LUT = _build_mag_lut()


def iq_to_magnitude(raw: np.ndarray) -> np.ndarray:
    """Convert interleaved uint8 IQ pairs to squared magnitude.

    Uses pre-computed lookup table for speed. Functionally identical
    to: (I - 127.5)² + (Q - 127.5)² per sample.

    Args:
        raw: Flat uint8 array [I0, Q0, I1, Q1, ...] from RTL-SDR.

    Returns:
        Float32 array of squared magnitudes, one per sample.
    """
    iq = raw.reshape(-1, 2)
    return _MAG_LUT[iq[:, 0], iq[:, 1]]


# --- Phase 8: Adaptive Signal Threshold ---

class _NoiseFloorTracker:
    """Track noise floor via exponential moving average of local medians."""

    def __init__(self):
        self._noise_floor: float = MIN_SIGNAL_LEVEL

    @property
    def noise_floor(self) -> float:
        return self._noise_floor

    @property
    def threshold(self) -> float:
        """Current adaptive threshold: max(noise_floor * factor, absolute minimum)."""
        return max(self._noise_floor * SNR_ADAPTIVE_FACTOR, MIN_ADAPTIVE_LEVEL)

    def update(self, mag_chunk: np.ndarray) -> None:
        """Update noise floor estimate from a magnitude buffer."""
        if len(mag_chunk) < 100:
            return
        # Sample 64 evenly spaced windows of 16 samples, take median of each
        step = max(1, len(mag_chunk) // 64)
        medians = []
        for i in range(0, len(mag_chunk) - 16, step):
            medians.append(float(np.median(mag_chunk[i:i + 16])))
        if not medians:
            return
        # Use the 25th percentile of medians as noise estimate (avoid signal spikes)
        local_noise = float(np.percentile(medians, 25))
        self._noise_floor = (
            (1 - NOISE_FLOOR_ALPHA) * self._noise_floor
            + NOISE_FLOOR_ALPHA * local_noise
        )

    def reset(self) -> None:
        self._noise_floor = MIN_SIGNAL_LEVEL


_noise_tracker = _NoiseFloorTracker()


def get_adaptive_threshold() -> float:
    """Return the current adaptive signal threshold."""
    return _noise_tracker.threshold


def reset_noise_tracker() -> None:
    """Reset the noise floor tracker. Used for test isolation."""
    _noise_tracker.reset()


# --- Phase 5: Improved Preamble Detection ---

def check_preamble(mag: np.ndarray, pos: int, min_level: float | None = None) -> float | None:
    """Check if a valid ADS-B preamble starts at position `pos`.

    Ported from dump1090 with three additional checks:
    1. Strict ordering: each pulse must individually exceed adjacent gaps
    2. Quiet zone: samples 10-15 must be below 2/3 of pulse average
    3. SNR check: signal * 2 >= 3 * noise (3.5 dB minimum)

    Args:
        mag: Squared magnitude array.
        pos: Start index to check.
        min_level: Minimum signal level override (default: adaptive threshold).

    Returns:
        Signal level (average pulse magnitude) if valid preamble, None otherwise.
    """
    if pos + WINDOW_SIZE > len(mag):
        return None

    effective_min = min_level if min_level is not None else _noise_tracker.threshold

    pulse_values = [float(mag[pos + p]) for p in PULSE_POSITIONS]
    gap_values = [float(mag[pos + g]) for g in GAP_POSITIONS]

    pulse_avg = sum(pulse_values) / len(pulse_values)
    gap_avg = sum(gap_values) / len(gap_values) if sum(gap_values) > 0 else 0.001

    if pulse_avg < effective_min:
        return None

    if pulse_avg / gap_avg < MIN_PREAMBLE_RATIO:
        return None

    # All pulses should be roughly similar amplitude
    if max(pulse_values) > 6 * min(pulse_values):
        return None

    # Phase 5: Strict ordering — each pulse must exceed its adjacent gaps
    # Pulse at 0 > gap at 1, pulse at 2 > gaps at 1 and 3, etc.
    if pulse_values[0] <= gap_values[0]:  # pulse[0] vs gap[1]
        return None
    if pulse_values[1] <= gap_values[0] or pulse_values[1] <= gap_values[2]:  # pulse[2] vs gap[1], gap[4]
        return None
    if pulse_values[2] <= gap_values[4]:  # pulse[7] vs gap[6]
        return None
    if pulse_values[3] <= gap_values[5]:  # pulse[9] vs gap[8]
        return None

    # Phase 5: Quiet zone — samples 10-15 should be low (< 2/3 pulse average)
    quiet_limit = pulse_avg * (2.0 / 3.0)
    for qp in QUIET_ZONE_POSITIONS:
        if pos + qp < len(mag) and mag[pos + qp] > quiet_limit:
            return None

    # Phase 5: SNR check — 3.5 dB minimum
    # signal * SNR_SIGNAL_FACTOR >= SNR_NOISE_FACTOR * noise
    if pulse_avg * SNR_SIGNAL_FACTOR < SNR_NOISE_FACTOR * gap_avg:
        return None

    return float(pulse_avg)


# --- Phase 6: Better Bit Recovery with Confidence ---

def recover_bits(
    mag: np.ndarray,
    pos: int,
    n_bits: int,
    track_confidence: bool = False,
) -> list[int] | tuple[list[int], int]:
    """Recover bits from magnitude signal using PPM decoding with continuity check.

    Each bit occupies 2 samples (1 µs at 2 MHz). Pulse Position Modulation:
    - Bit '1': energy in first sample > energy in second sample
    - Bit '0': energy in second sample >= energy in first sample

    When the delta between samples is below BIT_DELTA_THRESHOLD of the signal
    level, the decision is uncertain. In that case, use the previous bit value
    (continuity check from dump1090).

    Args:
        mag: Squared magnitude array.
        pos: Start index (first bit after preamble).
        n_bits: Number of bits to recover.
        track_confidence: If True, return (bits, uncertain_count) tuple.

    Returns:
        List of 0/1 integers, or (bits, uncertain_count) if track_confidence=True.
    """
    bits = []
    uncertain_count = 0
    prev_bit = 0

    for i in range(n_bits):
        sample_pos = pos + i * SAMPLES_PER_BIT
        if sample_pos + 1 >= len(mag):
            break

        high = float(mag[sample_pos])
        low = float(mag[sample_pos + 1])
        signal = max(high, low)

        if signal > 0 and abs(high - low) / signal < BIT_DELTA_THRESHOLD:
            # Weak transition — use previous bit value (continuity)
            bits.append(prev_bit)
            uncertain_count += 1
        elif high > low:
            bits.append(1)
        else:
            bits.append(0)

        prev_bit = bits[-1]

    if track_confidence:
        return bits, uncertain_count
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
    bits with confidence tracking and produces hex frame strings.

    Args:
        mag: Squared magnitude array from iq_to_magnitude().
        timestamp: Base timestamp for the buffer.

    Returns:
        List of RawFrame objects with hex strings.
    """
    # Update adaptive threshold from this buffer
    _noise_tracker.update(mag)

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
            bits, uncertain = recover_bits(mag, msg_start, LONG_MSG_BITS, track_confidence=True)
            if len(bits) == LONG_MSG_BITS:
                # Reject high-uncertainty frames
                if uncertain / LONG_MSG_BITS <= MAX_UNCERTAIN_RATIO:
                    hex_str = bits_to_hex(bits)
                    if len(hex_str) == 28:
                        df = (bits[0] << 4) | (bits[1] << 3) | (bits[2] << 2) | (bits[3] << 1) | bits[4]
                        if df in (16, 17, 18, 19, 20, 21):
                            frame_time = timestamp + i / sample_rate
                            frames.append(RawFrame(
                                hex_str=hex_str,
                                timestamp=frame_time,
                                signal_level=signal_level,
                                source="demodulator",
                            ))
                            i = msg_start + LONG_MSG_SAMPLES
                            continue

        # Try short message (56 bits)
        if msg_start + SHORT_MSG_SAMPLES <= len(mag):
            bits, uncertain = recover_bits(mag, msg_start, SHORT_MSG_BITS, track_confidence=True)
            if len(bits) == SHORT_MSG_BITS:
                if uncertain / SHORT_MSG_BITS <= MAX_UNCERTAIN_RATIO:
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
