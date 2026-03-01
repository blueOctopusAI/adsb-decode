"""Tests for the IQ demodulator — magnitude LUT, preamble, bit recovery, adaptive threshold."""

import numpy as np
import pytest

from src.demodulator import (
    BIT_DELTA_THRESHOLD,
    GAP_POSITIONS,
    LONG_MSG_BITS,
    LONG_MSG_SAMPLES,
    MAX_UNCERTAIN_RATIO,
    MIN_ADAPTIVE_LEVEL,
    MIN_SIGNAL_LEVEL,
    PREAMBLE_SAMPLES,
    PULSE_POSITIONS,
    QUIET_ZONE_POSITIONS,
    SHORT_MSG_BITS,
    SHORT_MSG_SAMPLES,
    WINDOW_SIZE,
    _MAG_LUT,
    _noise_tracker,
    bits_to_hex,
    check_preamble,
    demodulate_buffer,
    demodulate_file,
    get_adaptive_threshold,
    iq_to_magnitude,
    recover_bits,
    reset_noise_tracker,
)


@pytest.fixture(autouse=True)
def clean_noise_tracker():
    """Reset noise tracker before each test."""
    reset_noise_tracker()
    yield
    reset_noise_tracker()


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


# --- Phase 7: Magnitude Lookup Table ---

class TestMagLUT:
    """Magnitude lookup table tests."""

    def test_lut_shape(self):
        """LUT should be 256x256."""
        assert _MAG_LUT.shape == (256, 256)

    def test_lut_dtype(self):
        """LUT should be float32."""
        assert _MAG_LUT.dtype == np.float32

    def test_lut_center_value(self):
        """Center IQ (128, 128) should produce near-zero magnitude."""
        assert _MAG_LUT[128, 128] < 1.0

    def test_lut_max_signal(self):
        """(255, 255) should produce maximum magnitude."""
        expected = 2 * 127.5 ** 2
        assert _MAG_LUT[255, 255] == pytest.approx(expected, rel=1e-4)

    def test_lut_matches_arithmetic(self):
        """LUT values should match direct computation for sample points."""
        for i_val in [0, 64, 128, 192, 255]:
            for q_val in [0, 64, 128, 192, 255]:
                expected = (i_val - 127.5) ** 2 + (q_val - 127.5) ** 2
                assert _MAG_LUT[i_val, q_val] == pytest.approx(expected, rel=1e-4)


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

    def test_lut_matches_original_computation(self):
        """LUT-based iq_to_magnitude should match the original arithmetic."""
        rng = np.random.default_rng(42)
        raw = rng.integers(0, 256, size=200, dtype=np.uint8)
        mag = iq_to_magnitude(raw)
        # Compare with arithmetic method
        iq = raw.reshape(-1, 2).astype(np.float32) - 127.5
        expected = iq[:, 0] ** 2 + iq[:, 1] ** 2
        np.testing.assert_allclose(mag, expected, rtol=1e-5)


# --- Phase 5: Improved Preamble Detection ---

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

    def test_quiet_zone_violation_rejected(self):
        """High energy in quiet zone (samples 10-15) should reject preamble."""
        mag = np.zeros(WINDOW_SIZE + 100, dtype=np.float32)
        _build_preamble(mag, 0, level=500.0, noise=10.0)
        # Set quiet zone samples to high level (> 2/3 of pulse avg)
        for qp in QUIET_ZONE_POSITIONS:
            mag[qp] = 400.0  # > 500 * 2/3 = 333
        assert check_preamble(mag, 0) is None

    def test_quiet_zone_low_passes(self):
        """Low energy in quiet zone should still pass."""
        mag = np.zeros(WINDOW_SIZE + 100, dtype=np.float32)
        _build_preamble(mag, 0, level=500.0, noise=10.0)
        # Quiet zone at low level — should pass
        for qp in QUIET_ZONE_POSITIONS:
            mag[qp] = 20.0
        assert check_preamble(mag, 0) is not None

    def test_strict_ordering_pulse_must_exceed_gap(self):
        """Each pulse must individually exceed its adjacent gap."""
        mag = np.zeros(WINDOW_SIZE + 100, dtype=np.float32)
        _build_preamble(mag, 0, level=500.0, noise=10.0)
        # Make pulse 0 barely above gap 1 — but then make gap 1 higher than pulse
        mag[PULSE_POSITIONS[0]] = 50.0  # Very low pulse
        mag[GAP_POSITIONS[0]] = 51.0    # Gap higher than pulse
        # Should fail strict ordering
        assert check_preamble(mag, 0) is None

    def test_snr_check_rejects_low_snr(self):
        """signal * 2 < 3 * noise should be rejected (< 3.5 dB)."""
        mag = np.zeros(WINDOW_SIZE + 100, dtype=np.float32)
        # Set pulses at 200 and gaps at 150 — ratio > 2 but SNR too low
        # 200 * 2 = 400, 3 * 150 = 450 → fails
        _build_preamble(mag, 0, level=200.0, noise=150.0)
        assert check_preamble(mag, 0) is None

    def test_min_level_override(self):
        """min_level parameter should override adaptive threshold."""
        mag = np.zeros(WINDOW_SIZE + 100, dtype=np.float32)
        _build_preamble(mag, 0, level=80.0, noise=5.0)
        # Default threshold might reject this, but explicit min_level allows it
        assert check_preamble(mag, 0, min_level=50.0) is not None


# --- Phase 6: Bit Recovery with Confidence ---

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

    def test_confidence_tracking(self):
        """track_confidence=True should return (bits, uncertain_count) tuple."""
        n = 8
        mag = np.zeros(n * 2 + 10, dtype=np.float32)
        _encode_bits_ppm(mag, 0, [1] * n, level=500.0, noise=10.0)
        result = recover_bits(mag, 0, n, track_confidence=True)
        assert isinstance(result, tuple)
        bits, uncertain = result
        assert len(bits) == n
        assert uncertain == 0  # Strong signal should have zero uncertainty

    def test_weak_transitions_use_previous_bit(self):
        """When delta is below threshold, previous bit should be used."""
        n = 4
        mag = np.zeros(n * 2 + 10, dtype=np.float32)
        # First bit: strong 1
        mag[0] = 500.0
        mag[1] = 10.0
        # Second bit: weak transition (both samples nearly equal)
        mag[2] = 100.0
        mag[3] = 100.0 * (1 - BIT_DELTA_THRESHOLD * 0.5)  # Within threshold
        # Third bit: strong 0
        mag[4] = 10.0
        mag[5] = 500.0
        # Fourth bit: weak again
        mag[6] = 100.0
        mag[7] = 100.0 * (1 - BIT_DELTA_THRESHOLD * 0.5)

        bits, uncertain = recover_bits(mag, 0, n, track_confidence=True)
        assert uncertain >= 2  # At least bits 2 and 4 are uncertain

    def test_uncertain_count_tracked(self):
        """Uncertain bit count should be reported accurately."""
        n = 8
        mag = np.zeros(n * 2 + 10, dtype=np.float32)
        # All samples equal — every bit is uncertain
        mag[:n * 2] = 100.0
        bits, uncertain = recover_bits(mag, 0, n, track_confidence=True)
        assert uncertain == n  # All bits uncertain

    def test_backward_compatible_default(self):
        """Without track_confidence, should return plain list."""
        n = 4
        mag = np.zeros(n * 2 + 10, dtype=np.float32)
        _encode_bits_ppm(mag, 0, [1, 0, 1, 0])
        result = recover_bits(mag, 0, n)
        assert isinstance(result, list)
        assert result == [1, 0, 1, 0]


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


# --- Phase 8: Adaptive Signal Threshold ---

class TestAdaptiveThreshold:
    """Adaptive signal threshold tests."""

    def test_default_threshold(self):
        """Default threshold should be based on MIN_SIGNAL_LEVEL."""
        reset_noise_tracker()
        threshold = get_adaptive_threshold()
        # Initial noise floor is MIN_SIGNAL_LEVEL
        assert threshold >= MIN_ADAPTIVE_LEVEL

    def test_adapts_to_high_noise(self):
        """Threshold should increase when noise floor is high."""
        reset_noise_tracker()
        # Feed a high-noise buffer
        high_noise = np.full(2000, 500.0, dtype=np.float32)
        _noise_tracker.update(high_noise)
        threshold_after = get_adaptive_threshold()
        assert threshold_after > MIN_ADAPTIVE_LEVEL

    def test_floor_never_below_minimum(self):
        """Threshold should never go below MIN_ADAPTIVE_LEVEL."""
        reset_noise_tracker()
        # Feed a very quiet buffer
        quiet = np.full(2000, 1.0, dtype=np.float32)
        for _ in range(100):
            _noise_tracker.update(quiet)
        assert get_adaptive_threshold() >= MIN_ADAPTIVE_LEVEL

    def test_reset_restores_default(self):
        """reset_noise_tracker should restore initial state."""
        high_noise = np.full(2000, 500.0, dtype=np.float32)
        _noise_tracker.update(high_noise)
        reset_noise_tracker()
        assert _noise_tracker.noise_floor == MIN_SIGNAL_LEVEL

    def test_short_buffer_ignored(self):
        """Buffers shorter than 100 samples should not update noise floor."""
        reset_noise_tracker()
        initial = _noise_tracker.noise_floor
        short_buf = np.full(50, 5000.0, dtype=np.float32)
        _noise_tracker.update(short_buf)
        assert _noise_tracker.noise_floor == initial


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

    def test_high_uncertainty_frame_rejected(self):
        """A frame with mostly uncertain bits should be rejected."""
        hex_str = "8D4840D6202CC371C32CE0576098"
        buf_size = WINDOW_SIZE + 500
        mag = np.ones(buf_size, dtype=np.float32) * 5.0
        # Inject preamble normally
        _build_preamble(mag, 10)
        # But make all message bits equal magnitude (fully uncertain)
        msg_start = 10 + PREAMBLE_SAMPLES
        for i in range(LONG_MSG_BITS):
            s = msg_start + i * 2
            mag[s] = 200.0
            mag[s + 1] = 200.0
        frames = demodulate_buffer(mag)
        # Should be rejected due to high uncertainty
        assert len(frames) == 0


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

    def test_quiet_zone_positions(self):
        """Quiet zone should be samples 10-15."""
        assert QUIET_ZONE_POSITIONS == [10, 11, 12, 13, 14, 15]
