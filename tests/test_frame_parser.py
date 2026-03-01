"""Tests for frame parsing — hex strings to ModeFrame objects, ICAO cache, error correction."""

import pytest

from src.crc import _crc24_raw, crc24
from src.frame_parser import IcaoCache, ModeFrame, parse_frame, DF_INFO, reset_icao_cache
from tests.fixtures.known_frames import (
    CRC_VECTORS,
    IDENTIFICATION_FRAMES,
    POSITION_FRAMES,
    VELOCITY_FRAMES,
)


@pytest.fixture(autouse=True)
def clean_icao_cache():
    """Reset ICAO cache before each test for isolation."""
    reset_icao_cache()
    yield
    reset_icao_cache()


class TestParseFrame:
    """Core frame parsing."""

    def test_df17_identification(self):
        """Parse DF17 identification frames from test vectors."""
        for hex_str, expected_icao, expected_callsign in IDENTIFICATION_FRAMES:
            frame = parse_frame(hex_str)
            assert frame is not None, f"Failed to parse {hex_str}"
            assert frame.df == 17
            assert frame.icao == expected_icao
            assert frame.msg_bits == 112
            assert frame.crc_ok is True
            assert frame.is_adsb is True

    def test_df17_position(self):
        """Parse DF17 position frames."""
        for hex_str, expected_icao, expected_alt, cpr_fmt, _, _ in POSITION_FRAMES:
            frame = parse_frame(hex_str)
            assert frame is not None
            assert frame.df == 17
            assert frame.icao == expected_icao
            assert frame.crc_ok is True

    def test_df17_velocity(self):
        """Parse DF17 velocity frames."""
        for hex_str, expected_icao, _, _, _ in VELOCITY_FRAMES:
            frame = parse_frame(hex_str)
            assert frame is not None
            assert frame.df == 17
            assert frame.icao == expected_icao
            assert frame.crc_ok is True

    def test_single_bit_corrupted_frame_corrected(self):
        """Single-bit corrupted DF17 frame should be corrected by syndrome table."""
        corrupted = "8D4840D6202CC371C32CE0576099"  # Last digit changed (1 bit)
        frame = parse_frame(corrupted, validate_icao=False)
        assert frame is not None
        assert frame.crc_ok is True
        assert frame.corrected is True
        assert frame.icao == "4840D6"

    def test_heavily_corrupted_frame_rejected(self):
        """Frame with 3+ bit errors should still be rejected."""
        corrupted = "8D4840D6202CC371C32CE0576000"  # Multiple bits changed
        frame = parse_frame(corrupted, validate_icao=False)
        assert frame is not None
        assert frame.crc_ok is False

    def test_invalid_hex_returns_none(self):
        assert parse_frame("not_hex") is None

    def test_wrong_length_returns_none(self):
        assert parse_frame("8D4840D6") is None  # Too short
        assert parse_frame("8D4840D6202CC371C32CE057609800") is None  # Too long

    def test_empty_string_returns_none(self):
        assert parse_frame("") is None

    def test_timestamp_preserved(self):
        frame = parse_frame("8D4840D6202CC371C32CE0576098", timestamp=1234567890.5)
        assert frame.timestamp == 1234567890.5

    def test_signal_level_preserved(self):
        frame = parse_frame("8D4840D6202CC371C32CE0576098", signal_level=-42.5)
        assert frame.signal_level == -42.5

    def test_valid_frame_not_marked_corrected(self):
        """Clean frames should have corrected=False."""
        frame = parse_frame("8D4840D6202CC371C32CE0576098")
        assert frame.corrected is False


class TestModeFrameProperties:
    """ModeFrame dataclass properties."""

    @pytest.fixture
    def adsb_frame(self):
        return parse_frame("8D4840D6202CC371C32CE0576098")

    def test_df_name(self, adsb_frame):
        assert adsb_frame.df_name == "ADS-B extended squitter"

    def test_is_adsb(self, adsb_frame):
        assert adsb_frame.is_adsb is True

    def test_is_long(self, adsb_frame):
        assert adsb_frame.is_long is True

    def test_me_field(self, adsb_frame):
        """ME field should be 7 bytes from the middle of the message."""
        assert len(adsb_frame.me) == 7

    def test_type_code_identification(self):
        """TC 1-4 for identification frames."""
        frame = parse_frame("8D4840D6202CC371C32CE0576098")
        tc = frame.type_code
        assert tc is not None
        assert 1 <= tc <= 4  # Aircraft identification

    def test_type_code_position(self):
        """TC 9-18 for airborne position frames."""
        frame = parse_frame("8D40621D58C382D690C8AC2863A7")
        tc = frame.type_code
        assert tc is not None
        assert 9 <= tc <= 18  # Airborne position

    def test_type_code_velocity(self):
        """TC 19 for velocity frames."""
        frame = parse_frame("8D485020994409940838175B284F")
        tc = frame.type_code
        assert tc == 19

    def test_frozen_dataclass(self, adsb_frame):
        """ModeFrame should be immutable."""
        with pytest.raises(AttributeError):
            adsb_frame.df = 0


class TestDFClassification:
    """Downlink Format recognition."""

    def test_all_known_dfs_in_info(self):
        """All recognized DFs should be in DF_INFO."""
        expected_dfs = {0, 4, 5, 11, 16, 17, 18, 20, 21}
        assert set(DF_INFO.keys()) == expected_dfs

    def test_df17_is_112_bits(self):
        assert DF_INFO[17][1] == 112

    def test_df11_is_56_bits(self):
        assert DF_INFO[11][1] == 56

    def test_df0_is_56_bits(self):
        assert DF_INFO[0][1] == 56


class TestIntegrationPipeline:
    """Test the capture -> parse pipeline."""

    def test_hex_to_parsed_frame(self):
        """Simulate reading hex strings and parsing them."""
        hex_frames = [hex_str for hex_str, _, _ in IDENTIFICATION_FRAMES]
        hex_frames += [hex_str for hex_str, _, _, _, _, _ in POSITION_FRAMES]
        hex_frames += [hex_str for hex_str, _, _, _, _ in VELOCITY_FRAMES]

        parsed = []
        for h in hex_frames:
            frame = parse_frame(h)
            if frame and frame.crc_ok:
                parsed.append(frame)

        assert len(parsed) == len(hex_frames)
        assert all(f.df == 17 for f in parsed)
        assert all(f.is_adsb for f in parsed)


class TestIcaoCache:
    """Phase 3: ICAO address cache unit tests."""

    def test_register_and_lookup(self):
        cache = IcaoCache(ttl=60.0)
        cache.register("AABBCC", 1000.0)
        assert cache.is_known("AABBCC", 1000.0) is True

    def test_unknown_icao_rejected(self):
        cache = IcaoCache(ttl=60.0)
        assert cache.is_known("AABBCC", 1000.0) is False

    def test_ttl_expiry(self):
        cache = IcaoCache(ttl=60.0)
        cache.register("AABBCC", 1000.0)
        # Within TTL
        assert cache.is_known("AABBCC", 1050.0) is True
        # After TTL
        assert cache.is_known("AABBCC", 1061.0) is False

    def test_register_updates_timestamp(self):
        cache = IcaoCache(ttl=60.0)
        cache.register("AABBCC", 1000.0)
        cache.register("AABBCC", 1050.0)  # Refresh
        # Should still be known at 1100 (50s after refresh)
        assert cache.is_known("AABBCC", 1100.0) is True

    def test_prune_removes_expired(self):
        cache = IcaoCache(ttl=60.0)
        cache.register("OLD", 1000.0)
        cache.register("NEW", 1050.0)
        cache.prune(1070.0)  # OLD expired (70s), NEW still valid (20s)
        assert len(cache) == 1
        assert cache.is_known("NEW", 1070.0) is True

    def test_multiple_icaos(self):
        cache = IcaoCache(ttl=60.0)
        cache.register("AAA111", 1000.0)
        cache.register("BBB222", 1000.0)
        cache.register("CCC333", 1000.0)
        assert len(cache) == 3
        assert cache.is_known("BBB222", 1000.0) is True

    def test_len(self):
        cache = IcaoCache(ttl=60.0)
        assert len(cache) == 0
        cache.register("AABBCC", 1000.0)
        assert len(cache) == 1


class TestIcaoCacheIntegration:
    """Phase 3: ICAO cache integrated with parse_frame."""

    def _build_df4_frame(self, icao_hex: str) -> str:
        """Build a DF4 (56-bit) frame where CRC residual = given ICAO.

        Mode S CRC: raw_crc(data[:-3]) XOR data[-3:]
        For DF4: PI = raw_crc(first_4_bytes) XOR ICAO
        So: crc24(full) = raw_crc(first_4) XOR PI = raw_crc(first_4) XOR raw_crc(first_4) XOR ICAO = ICAO
        """
        # DF4 = 00100 << 3 = 0x20
        msg = bytearray(7)
        msg[0] = 0x20  # DF4
        # Raw CRC of data portion (first 4 bytes)
        raw = _crc24_raw(bytes(msg[:4]))
        # PI = raw_crc XOR ICAO
        icao_int = int(icao_hex, 16)
        pi = raw ^ icao_int
        msg[4] = (pi >> 16) & 0xFF
        msg[5] = (pi >> 8) & 0xFF
        msg[6] = pi & 0xFF
        # Verify: residual should equal ICAO
        assert crc24(bytes(msg)) == icao_int, f"Residual {crc24(bytes(msg)):06X} != ICAO {icao_hex}"
        return msg.hex().upper()

    def test_df4_rejected_without_prior_df17(self):
        """DF4 frame should be rejected if ICAO not in cache."""
        df4_hex = self._build_df4_frame("4840D6")
        frame = parse_frame(df4_hex, timestamp=1000.0, validate_icao=True)
        assert frame is None

    def test_df4_accepted_after_df17(self):
        """DF4 frame should be accepted if ICAO was registered by a prior DF17."""
        # First, send a DF17 to register the ICAO
        df17_hex = "8D4840D6202CC371C32CE0576098"
        frame17 = parse_frame(df17_hex, timestamp=1000.0, validate_icao=True)
        assert frame17 is not None
        assert frame17.icao == "4840D6"

        # Now the DF4 frame with same ICAO should be accepted
        df4_hex = self._build_df4_frame("4840D6")
        frame4 = parse_frame(df4_hex, timestamp=1001.0, validate_icao=True)
        assert frame4 is not None
        assert frame4.icao == "4840D6"

    def test_df4_rejected_after_ttl_expires(self):
        """DF4 should be rejected after the ICAO cache TTL expires."""
        df17_hex = "8D4840D6202CC371C32CE0576098"
        parse_frame(df17_hex, timestamp=1000.0, validate_icao=True)

        df4_hex = self._build_df4_frame("4840D6")
        # 120 seconds later — beyond 60s TTL
        frame = parse_frame(df4_hex, timestamp=1120.0, validate_icao=True)
        assert frame is None

    def test_validate_icao_bypass(self):
        """With validate_icao=False, DF4 should be accepted without cache."""
        df4_hex = self._build_df4_frame("4840D6")
        frame = parse_frame(df4_hex, timestamp=1000.0, validate_icao=False)
        assert frame is not None
        assert frame.icao == "4840D6"

    def test_reset_clears_cache(self):
        """reset_icao_cache should clear all entries."""
        df17_hex = "8D4840D6202CC371C32CE0576098"
        parse_frame(df17_hex, timestamp=1000.0, validate_icao=True)
        reset_icao_cache()
        df4_hex = self._build_df4_frame("4840D6")
        frame = parse_frame(df4_hex, timestamp=1001.0, validate_icao=True)
        assert frame is None
