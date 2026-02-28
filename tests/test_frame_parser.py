"""Tests for frame parsing — hex strings to ModeFrame objects."""

import pytest

from src.frame_parser import ModeFrame, parse_frame, DF_INFO
from tests.fixtures.known_frames import (
    CRC_VECTORS,
    IDENTIFICATION_FRAMES,
    POSITION_FRAMES,
    VELOCITY_FRAMES,
)


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

    def test_corrupted_frame_rejected(self):
        """Corrupted DF17 frame should have crc_ok=False."""
        corrupted = "8D4840D6202CC371C32CE0576099"  # Last digit changed
        frame = parse_frame(corrupted)
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
