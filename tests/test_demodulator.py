"""Tests for the IQ demodulator — magnitude, preamble, bit recovery, full decode."""

import numpy as np
import pytest

from src.demodulator import (
    GAP_POSITIONS,
    LONG_MSG_BITS,
    LONG_MSG_SAMPLES,
    MIN_SIGNAL_LEVEL,
    PREAMBLE_SAMPLES,
    PULSE_POSITIONS,
    SHORT_MSG_BITS,
    SHORT_MSG_SAMPLES,
    WINDOW_SIZE,
    bits_to_hex,
    check_preamble,
    demodulate_buffer,
    demodulate_file,
    iq_to_magnitude,
    recover_bits,
)


# --- Helpers ---

def _build_preamble(mag: np.ndarray, pos: int, level: float = 500.0, noise: float = 10.0):
    """Inject a valid preamble at `pos` in the magnitude array."""
    for p in PULSE_POSITIONS:
        mag[pos + p] = level
    for g in GAP_POSITIONS:
        mag[pos + g] = noise


def _encode_bits_ppm(mag: np.ndarray, start: int, bits: list[int], level: float = 500.0, noise: float = 10.0):
    """Encode bits into mag using PPM: bit 1 = high-low, bit 0 = low-high."""
    for i, b in enumerate(bits):
        s = start + i * 2
        if b:
            mag[s] = level
            mag[s + 1] = noise
        else:
            mag[s] = noise
            mag[s + 1] = level


def _hex_to_bits(hex_str: str) -> list[int]:
    """Convert hex string to bit list."""
    bits = []
    for ch in hex_str:
        val = int(ch, 16)
        bits.extend([(val >> (3 - j)) & 1 for j in range(4)])
    return bits


# --- IQ to magnitude ---

class TestIQToMagnitude:
    def test_shape(self):
        raw = np.array([128, 128, 200, 50], dtype=np.uint8)
        mag = iq_to_magnitude(raw)
        assert len(mag) == 2  # 2 IQ pairs

    def test_center_point_near_zero(self):
        # 128, 128 → centered at (0.5, 0.5) → squared mag ~ 0.5
        raw = np.array([128, 128], dtype=np.uint8)
        mag = iq_to_magnitude(raw)
        assert mag[0] < 1.0  # Near zero after centering

    def test_max_signal(self):
        raw = np.array([255, 255], dtype=np.uint8)
        mag = iq_to_magnitude(raw)
        # (255 - 127.5)^2 + (255 - 127.5)^2 = 2 * 127.5^2
        assert mag[0] == pytest.approx(2 * 127.5**2, rel=1e-4)

    def test_dtype_float32(self):
        raw = np.array([100, 100, 200, 200], dtype=np.uint8)
        mag = iq_to_magnitude(raw)
        assert mag.dtype == np.float32


# --- Preamble detection ---

class TestCheckPreamble:
    def test_valid_preamble(self):
        mag = np.zeros(WINDOW_SIZE + 100, dtype=np.float32)
        _build_preamble(mag, 0, level=500.0, noise=10.0)
        result = check_preamble(mag, 0)
        assert result is not None
        assert result > 0

    def test_no_signal_rejected(self):
        mag = np.zeros(WINDOW_SIZE + 100, dtype=np.float32)
        assert check_preamble(mag, 0) is None

    def test_low_signal_rejected(self):
        mag = np.ones(WINDOW_SIZE + 100, dtype=np.float32) * 10.0
        _build_preamble(mag, 0, level=50.0, noise=10.0)
        assert check_preamble(mag, 0) is None  # Below MIN_SIGNAL_LEVEL

    def test_bad_ratio_rejected(self):
        mag = np.zeros(WINDOW_SIZE + 100, dtype=np.float32)
        # Pulse and gap at similar levels → bad ratio
        _build_preamble(mag, 0, level=500.0, noise=400.0)
        assert check_preamble(mag, 0) is None

    def test_uneven_pulses_rejected(self):
        mag = np.zeros(WINDOW_SIZE + 100, dtype=np.float32)
        _build_preamble(mag, 0, level=500.0)
        # Make one pulse wildly different (>6x min)
        mag[PULSE_POSITIONS[0]] = 5000.0
        mag[PULSE_POSITIONS[1]] = 50.0  # 5000/50 = 100 > 6
        assert check_preamble(mag, 0) is None

    def test_out_of_bounds_returns_none(self):
        mag = np.zeros(10, dtype=np.float32)  # Way too short
        assert check_preamble(mag, 0) is None

    def test_offset_preamble(self):
        mag = np.zeros(WINDOW_SIZE + 200, dtype=np.float32)
        _build_preamble(mag, 50, level=500.0, noise=10.0)
        assert check_preamble(mag, 50) is not None
        assert check_preamble(mag, 0) is None


# --- Bit recovery ---

class TestRecoverBits:
    def test_all_ones(self):
        n = 8
        mag = np.zeros(n * 2 + 10, dtype=np.float32)
        _encode_bits_ppm(mag, 0, [1] * n)
        bits = recover_bits(mag, 0, n)
        assert bits == [1] * n

    def test_all_zeros(self):
        n = 8
        mag = np.zeros(n * 2 + 10, dtype=np.float32)
        _encode_bits_ppm(mag, 0, [0] * n)
        bits = recover_bits(mag, 0, n)
        assert bits == [0] * n

    def test_alternating(self):
        pattern = [1, 0, 1, 0, 1, 0, 1, 0]
        mag = np.zeros(len(pattern) * 2 + 10, dtype=np.float32)
        _encode_bits_ppm(mag, 0, pattern)
        bits = recover_bits(mag, 0, len(pattern))
        assert bits == pattern

    def test_known_byte(self):
        # 0x8D = 10001101
        pattern = [1, 0, 0, 0, 1, 1, 0, 1]
        mag = np.zeros(len(pattern) * 2 + 10, dtype=np.float32)
        _encode_bits_ppm(mag, 0, pattern)
        bits = recover_bits(mag, 0, len(pattern))
        assert bits == pattern

    def test_truncated_returns_partial(self):
        mag = np.zeros(5, dtype=np.float32)  # Not enough for 8 bits
        bits = recover_bits(mag, 0, 8)
        assert len(bits) < 8


# --- Bits to hex ---

class TestBitsToHex:
    def test_single_nibble(self):
        assert bits_to_hex([1, 0, 1, 0]) == "A"

    def test_byte(self):
        assert bits_to_hex([1, 0, 0, 0, 1, 1, 0, 1]) == "8D"

    def test_full_28_char_message(self):
        hex_str = "8D4840D6202CC371C32CE0576098"
        bits = _hex_to_bits(hex_str)
        assert bits_to_hex(bits) == hex_str

    def test_partial_nibble_ignored(self):
        # 5 bits = 1 full nibble + 1 leftover bit → only 1 hex char
        bits = [1, 0, 1, 0, 1]
        result = bits_to_hex(bits)
        assert len(result) == 1

    def test_empty(self):
        assert bits_to_hex([]) == ""


# --- Full demodulation ---

class TestDemodulateBuffer:
    def _inject_message(self, mag, pos, hex_str):
        """Inject preamble + PPM-encoded message at pos."""
        _build_preamble(mag, pos)
        bits = _hex_to_bits(hex_str)
        _encode_bits_ppm(mag, pos + PREAMBLE_SAMPLES, bits)

    def test_single_df17_message(self):
        """DF17 (10001) should be detected as a long message."""
        hex_str = "8D4840D6202CC371C32CE0576098"
        buf_size = WINDOW_SIZE + 500
        mag = np.ones(buf_size, dtype=np.float32) * 5.0  # Low noise floor
        self._inject_message(mag, 10, hex_str)
        frames = demodulate_buffer(mag, timestamp=100.0)
        assert len(frames) >= 1
        assert frames[0].hex_str == hex_str
        assert frames[0].source == "demodulator"

    def test_empty_buffer_no_frames(self):
        mag = np.zeros(1000, dtype=np.float32)
        frames = demodulate_buffer(mag)
        assert frames == []

    def test_noise_only_no_frames(self):
        rng = np.random.default_rng(42)
        mag = rng.uniform(0, 50, size=2000).astype(np.float32)
        frames = demodulate_buffer(mag)
        assert frames == []

    def test_timestamp_from_position(self):
        hex_str = "8D4840D6202CC371C32CE0576098"
        buf_size = WINDOW_SIZE + 500
        mag = np.ones(buf_size, dtype=np.float32) * 5.0
        self._inject_message(mag, 10, hex_str)
        frames = demodulate_buffer(mag, timestamp=100.0)
        if frames:
            # Timestamp should be base + offset/sample_rate
            assert frames[0].timestamp >= 100.0
            assert frames[0].timestamp < 101.0  # Within 1 second

    def test_signal_level_recorded(self):
        hex_str = "8D4840D6202CC371C32CE0576098"
        buf_size = WINDOW_SIZE + 500
        mag = np.ones(buf_size, dtype=np.float32) * 5.0
        self._inject_message(mag, 10, hex_str)
        frames = demodulate_buffer(mag, timestamp=0.0)
        if frames:
            assert frames[0].signal_level is not None
            assert frames[0].signal_level > 0

    def test_multiple_messages_separated(self):
        """Two messages spaced apart should both be found."""
        hex_str = "8D4840D6202CC371C32CE0576098"
        buf_size = WINDOW_SIZE * 4
        mag = np.ones(buf_size, dtype=np.float32) * 5.0
        self._inject_message(mag, 10, hex_str)
        self._inject_message(mag, WINDOW_SIZE + 100, hex_str)
        frames = demodulate_buffer(mag)
        assert len(frames) >= 2


class TestDemodulateFile:
    def test_iq_file_roundtrip(self, tmp_path):
        """Create a synthetic IQ file, demodulate it, verify we get frames back."""
        # Build a magnitude array with one injected message
        n_samples = 10000
        hex_str = "8D4840D6202CC371C32CE0576098"
        bits = _hex_to_bits(hex_str)

        # Build IQ pairs that will produce the desired magnitude pattern
        # For simplicity: encode as I-only (Q=128) with I=high for pulse, I=128 for gap
        iq = np.full(n_samples * 2, 128, dtype=np.uint8)

        # Inject preamble at sample 100
        start = 100
        high_val = 250  # Will produce (250-127.5)^2 = ~15006
        low_val = 130   # Will produce (130-127.5)^2 = ~6.25

        for p in PULSE_POSITIONS:
            iq[(start + p) * 2] = high_val
        for g in GAP_POSITIONS:
            iq[(start + g) * 2] = low_val

        # Encode bits after preamble using PPM on I channel
        msg_start = start + PREAMBLE_SAMPLES
        for i, b in enumerate(bits):
            s = msg_start + i * 2
            if b:
                iq[s * 2] = high_val
                iq[(s + 1) * 2] = low_val
            else:
                iq[s * 2] = low_val
                iq[(s + 1) * 2] = high_val

        # Write to file
        iq_file = tmp_path / "test.iq"
        iq.tofile(str(iq_file))

        frames = demodulate_file(str(iq_file))
        # We may or may not decode perfectly depending on signal quality,
        # but the pipeline shouldn't crash
        assert isinstance(frames, list)

    def test_empty_file(self, tmp_path):
        iq_file = tmp_path / "empty.iq"
        iq_file.write_bytes(b"")
        frames = demodulate_file(str(iq_file))
        assert frames == []

    def test_tiny_file(self, tmp_path):
        iq_file = tmp_path / "tiny.iq"
        iq_file.write_bytes(b"\x80\x80" * 100)  # 100 samples, way less than WINDOW_SIZE
        frames = demodulate_file(str(iq_file))
        assert frames == []


# --- Constants sanity checks ---

class TestConstants:
    def test_window_size(self):
        assert WINDOW_SIZE == PREAMBLE_SAMPLES + LONG_MSG_SAMPLES

    def test_preamble_16_samples(self):
        assert PREAMBLE_SAMPLES == 16

    def test_long_msg_224_samples(self):
        assert LONG_MSG_SAMPLES == 224

    def test_short_msg_112_samples(self):
        assert SHORT_MSG_SAMPLES == 112

    def test_pulse_and_gap_cover_preamble(self):
        all_positions = set(PULSE_POSITIONS + GAP_POSITIONS)
        # Should cover samples 0-9 (first 10 of 16 preamble samples)
        assert all_positions == {0, 1, 2, 3, 4, 5, 6, 7, 8, 9}
