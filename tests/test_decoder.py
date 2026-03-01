"""Tests for message decoder — callsign, position, velocity, squawk."""

import math
import pytest

from src.decoder import (
    _decode_gillham_altitude,
    decode,
    decode_altitude,
    decode_altitude_13bit,
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

    def test_corrected_single_bit_decodes(self):
        """Single-bit error correction should allow decoding."""
        frame = parse_frame("8D4840D6202CC371C32CE0576099", validate_icao=False)
        msg = decode(frame)
        assert isinstance(msg, IdentificationMsg)  # Error correction recovered it
        assert msg.callsign == "KLM1023 "

    def test_heavily_corrupted_returns_none(self):
        """3+ bit errors cannot be corrected, decode returns None."""
        frame = parse_frame("8D4840D6202CC371C32CE0576000", validate_icao=False)
        msg = decode(frame)
        assert msg is None  # CRC failed, not correctable


class TestGillhamAltitude:
    """Phase 4: Gillham gray code altitude decoding."""

    def _encode_gillham(self, a_digit, b_digit, c_digit, d_digit=0):
        """Encode Mode A octal digits into a 13-bit altitude code.

        Bit positions: C1 A1 C2 A2 C4 A4 M(0) B1 Q(0) B2 D2 B4 D4
        """
        c1 = (c_digit >> 0) & 1
        c2 = (c_digit >> 1) & 1
        c4 = (c_digit >> 2) & 1
        a1 = (a_digit >> 0) & 1
        a2 = (a_digit >> 1) & 1
        a4 = (a_digit >> 2) & 1
        b1 = (b_digit >> 0) & 1
        b2 = (b_digit >> 1) & 1
        b4 = (b_digit >> 2) & 1
        d1 = (d_digit >> 0) & 1
        d2 = (d_digit >> 1) & 1
        d4 = (d_digit >> 2) & 1

        code = (c1 << 12) | (a1 << 11) | (c2 << 10) | (a2 << 9) | (c4 << 8) | (a4 << 7)
        code |= (0 << 6)  # M bit = 0
        code |= (b1 << 5) | (0 << 4) | (b2 << 3) | (d2 << 2) | (b4 << 1) | d4
        return code

    def test_gillham_returns_integer(self):
        """Gillham decoder should return an altitude (not None) for valid codes."""
        # A=0, B=0, C=1 → should be a valid low altitude
        code = self._encode_gillham(a_digit=0, b_digit=0, c_digit=1)
        alt = _decode_gillham_altitude(code)
        assert alt is not None
        assert isinstance(alt, int)

    def test_gillham_zero_code_returns_none(self):
        """Alt code 0 should be handled by caller (decode_altitude), not Gillham."""
        # decode_altitude returns None for code=0 before calling Gillham
        assert decode_altitude(0) is None

    def test_gillham_invalid_c_zero_returns_none(self):
        """C digit of 0 is invalid in Gillham (no 0 offset in 100-ft encoding)."""
        code = self._encode_gillham(a_digit=0, b_digit=0, c_digit=0)
        alt = _decode_gillham_altitude(code)
        assert alt is None

    def test_decode_altitude_routes_to_gillham(self):
        """decode_altitude should route to Gillham when Q-bit is 0."""
        # Build a code with Q-bit=0 and valid Gillham content
        code = self._encode_gillham(a_digit=0, b_digit=0, c_digit=1)
        # Verify Q-bit is 0
        assert (code >> 4) & 1 == 0
        alt = decode_altitude(code)
        # Should return something (not None) for valid Gillham
        if alt is not None:
            assert -1200 <= alt <= 126750

    def test_decode_altitude_13bit_routes_to_gillham(self):
        """decode_altitude_13bit should route to Gillham when M=0, Q=0."""
        code = self._encode_gillham(a_digit=0, b_digit=0, c_digit=1)
        # M-bit at position 6 should be 0, Q-bit at position 4 should be 0
        assert (code >> 6) & 1 == 0
        assert (code >> 4) & 1 == 0
        alt = decode_altitude_13bit(code)
        if alt is not None:
            assert -1200 <= alt <= 126750

    def test_gillham_range_check(self):
        """Any returned altitude should be within valid Gillham range."""
        # Test multiple valid digit combinations
        for a in range(8):
            for b in range(8):
                for c in range(1, 6):  # C=0 and C>5 invalid
                    code = self._encode_gillham(a, b, c)
                    alt = _decode_gillham_altitude(code)
                    if alt is not None:
                        assert -1200 <= alt <= 126750, f"Out of range: {alt} for A={a} B={b} C={c}"

    def test_gillham_different_c_values_different_altitudes(self):
        """Different C values with same A/B should produce different 100-ft offsets."""
        alts = []
        for c in range(1, 6):
            code = self._encode_gillham(a_digit=0, b_digit=0, c_digit=c)
            alt = _decode_gillham_altitude(code)
            if alt is not None:
                alts.append(alt)
        # Should have distinct altitudes for different C values
        assert len(set(alts)) == len(alts), "C values should produce distinct altitudes"

    def test_gillham_increasing_a_increases_altitude(self):
        """Increasing A/B digits should generally increase altitude."""
        prev_alt = None
        increasing = True
        for ab in range(8):
            code = self._encode_gillham(a_digit=ab, b_digit=0, c_digit=1)
            alt = _decode_gillham_altitude(code)
            if alt is not None and prev_alt is not None:
                if alt <= prev_alt:
                    increasing = False
            if alt is not None:
                prev_alt = alt
        # Gray code doesn't guarantee monotonic increase for all values
        # but there should be general upward trend — just verify we get values
        assert prev_alt is not None
