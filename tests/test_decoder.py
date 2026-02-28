"""Tests for message decoder — callsign, position, velocity, squawk."""

import math
import pytest

from src.decoder import (
    decode,
    decode_altitude,
    decode_identification,
    decode_position,
    decode_squawk,
    decode_velocity,
    IdentificationMsg,
    PositionMsg,
    VelocityMsg,
)
from src.frame_parser import parse_frame
from tests.fixtures.known_frames import (
    IDENTIFICATION_FRAMES,
    POSITION_FRAMES,
    VELOCITY_FRAMES,
    POSITION_DECODED,
)


class TestDecodeIdentification:
    """TC 1-4: Callsign decoding."""

    def test_known_callsigns(self):
        for hex_str, expected_icao, expected_callsign in IDENTIFICATION_FRAMES:
            frame = parse_frame(hex_str)
            msg = decode_identification(frame)
            assert msg is not None, f"Failed to decode {hex_str}"
            assert msg.icao == expected_icao
            assert msg.callsign == expected_callsign
            assert isinstance(msg, IdentificationMsg)

    def test_callsign_klm1023(self):
        frame = parse_frame("8D4840D6202CC371C32CE0576098")
        msg = decode_identification(frame)
        assert msg.callsign == "KLM1023 "
        assert msg.icao == "4840D6"

    def test_callsign_ezy85mh(self):
        frame = parse_frame("8D406B902015A678D4D220AA4BDA")
        msg = decode_identification(frame)
        assert msg.callsign == "EZY85MH "

    def test_non_identification_returns_none(self):
        """Position frame should not decode as identification."""
        frame = parse_frame("8D40621D58C382D690C8AC2863A7")
        msg = decode_identification(frame)
        assert msg is None

    def test_category_extracted(self):
        frame = parse_frame("8D4840D6202CC371C32CE0576098")
        msg = decode_identification(frame)
        assert isinstance(msg.category, int)


class TestDecodePosition:
    """TC 9-18: Airborne position with CPR encoding."""

    def test_position_frames_parsed(self):
        for hex_str, expected_icao, expected_alt, cpr_fmt, cpr_lat, cpr_lon in POSITION_FRAMES:
            frame = parse_frame(hex_str)
            msg = decode_position(frame)
            assert msg is not None, f"Failed to decode position {hex_str}"
            assert msg.icao == expected_icao
            assert isinstance(msg, PositionMsg)

    def test_even_frame_cpr_values(self):
        """Verify CPR values from even frame test vector."""
        hex_str, _, _, cpr_fmt, expected_lat, expected_lon = POSITION_FRAMES[0]
        frame = parse_frame(hex_str)
        msg = decode_position(frame)
        assert msg.cpr_odd is False  # Even frame
        assert msg.cpr_lat == expected_lat
        assert msg.cpr_lon == expected_lon

    def test_odd_frame_cpr_values(self):
        """Verify CPR values from odd frame test vector."""
        hex_str, _, _, cpr_fmt, expected_lat, expected_lon = POSITION_FRAMES[1]
        frame = parse_frame(hex_str)
        msg = decode_position(frame)
        assert msg.cpr_odd is True  # Odd frame
        assert msg.cpr_lat == expected_lat
        assert msg.cpr_lon == expected_lon

    def test_altitude_decoded(self):
        """Check altitude extraction from position frames."""
        hex_str, _, expected_alt, _, _, _ = POSITION_FRAMES[0]
        frame = parse_frame(hex_str)
        msg = decode_position(frame)
        assert msg.altitude_ft == expected_alt

    def test_non_position_returns_none(self):
        frame = parse_frame("8D4840D6202CC371C32CE0576098")  # TC 4 (identification)
        msg = decode_position(frame)
        assert msg is None


class TestDecodeVelocity:
    """TC 19: Velocity decoding."""

    def test_velocity_frame_parsed(self):
        for hex_str, expected_icao, expected_speed, expected_hdg, expected_vrate in VELOCITY_FRAMES:
            frame = parse_frame(hex_str)
            msg = decode_velocity(frame)
            assert msg is not None, f"Failed to decode velocity {hex_str}"
            assert msg.icao == expected_icao
            assert isinstance(msg, VelocityMsg)

    def test_ground_speed(self):
        hex_str, _, expected_speed, _, _ = VELOCITY_FRAMES[0]
        frame = parse_frame(hex_str)
        msg = decode_velocity(frame)
        assert msg.speed_kts is not None
        # Allow some tolerance in speed calculation
        assert abs(msg.speed_kts - expected_speed) < 2.0

    def test_heading(self):
        hex_str, _, _, expected_hdg, _ = VELOCITY_FRAMES[0]
        frame = parse_frame(hex_str)
        msg = decode_velocity(frame)
        assert msg.heading_deg is not None
        assert abs(msg.heading_deg - expected_hdg) < 1.0

    def test_vertical_rate(self):
        hex_str, _, _, _, expected_vrate = VELOCITY_FRAMES[0]
        frame = parse_frame(hex_str)
        msg = decode_velocity(frame)
        assert msg.vertical_rate_fpm is not None
        assert abs(msg.vertical_rate_fpm - expected_vrate) < 128  # 64 fpm resolution

    def test_speed_type_ground(self):
        frame = parse_frame(VELOCITY_FRAMES[0][0])
        msg = decode_velocity(frame)
        assert msg.speed_type == "ground"

    def test_non_velocity_returns_none(self):
        frame = parse_frame("8D4840D6202CC371C32CE0576098")  # Identification
        msg = decode_velocity(frame)
        assert msg is None


class TestDecodeAltitude:
    """12-bit altitude field decoding."""

    def test_25ft_mode(self):
        """Test 25-ft resolution altitude decode."""
        # Q-bit set, altitude = N * 25 - 1000
        # For altitude 38000 ft: N = (38000 + 1000) / 25 = 1560
        # 1560 = 0x618, with Q-bit at position 4:
        # Upper 7 bits: 0x618 >> 4 = 0x61 (97), lower 4 bits: 0x618 & 0xF = 8
        # alt_code = (97 << 5) | (1 << 4) | 8 = 3104 | 16 | 8 = 3128 = 0xC38
        alt = decode_altitude(0xC38)
        assert alt == 38000

    def test_zero_returns_none(self):
        assert decode_altitude(0) is None


class TestDecodeSquawk:
    """Squawk code decoding."""

    def test_emergency_7700(self):
        """Squawk 7700 emergency encoding."""
        # 7700 octal: A=7, B=7, C=0, D=0
        # Bit layout: C1 A1 C2 A2 C4 A4 SPI B1 D1 B2 D2 B4 D4
        #              0  1  0  1  0  1   0  1  0  1  0  1  0
        code = 0b0101010101010
        squawk = decode_squawk(code)
        assert squawk == "7700"

    def test_hijack_7500(self):
        # 7500: A=7, B=5, C=0, D=0
        # Bit layout: C1 A1 C2 A2 C4 A4 SPI B1 D1 B2 D2 B4 D4
        #              0  1  0  1  0  1   0  1  0  0  0  1  0
        code = 0b0101010100010
        squawk = decode_squawk(code)
        assert squawk == "7500"


class TestDecodeRouter:
    """The top-level decode() function routes to correct decoder."""

    def test_routes_identification(self):
        frame = parse_frame("8D4840D6202CC371C32CE0576098")
        msg = decode(frame)
        assert isinstance(msg, IdentificationMsg)

    def test_routes_position(self):
        frame = parse_frame("8D40621D58C382D690C8AC2863A7")
        msg = decode(frame)
        assert isinstance(msg, PositionMsg)

    def test_routes_velocity(self):
        frame = parse_frame("8D485020994409940838175B284F")
        msg = decode(frame)
        assert isinstance(msg, VelocityMsg)

    def test_corrupted_returns_none(self):
        frame = parse_frame("8D4840D6202CC371C32CE0576099")
        msg = decode(frame)
        assert msg is None  # CRC failed
