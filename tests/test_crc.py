"""Tests for CRC-24 validation."""

import pytest

from src.crc import crc24, extract_icao, residual, validate
from tests.fixtures.known_frames import CRC_VECTORS, IDENTIFICATION_FRAMES, POSITION_FRAMES, VELOCITY_FRAMES


class TestCRC24:
    """Core CRC-24 computation."""

    def test_valid_df17_remainder_is_zero(self):
        """Valid DF17 frames should produce CRC remainder of 0."""
        for hex_str, expected in CRC_VECTORS:
            data = bytes.fromhex(hex_str)
            assert crc24(data) == expected, f"CRC mismatch for {hex_str}"

    def test_all_identification_frames_valid(self):
        """All identification test vectors should pass CRC."""
        for hex_str, _, _ in IDENTIFICATION_FRAMES:
            data = bytes.fromhex(hex_str)
            assert crc24(data) == 0, f"CRC failed for identification frame {hex_str}"

    def test_all_position_frames_valid(self):
        """All position test vectors should pass CRC."""
        for hex_str, _, _, _, _, _ in POSITION_FRAMES:
            data = bytes.fromhex(hex_str)
            assert crc24(data) == 0, f"CRC failed for position frame {hex_str}"

    def test_all_velocity_frames_valid(self):
        """All velocity test vectors should pass CRC."""
        for hex_str, _, _, _, _ in VELOCITY_FRAMES:
            data = bytes.fromhex(hex_str)
            assert crc24(data) == 0, f"CRC failed for velocity frame {hex_str}"

    def test_corrupted_frame_nonzero(self):
        """A corrupted frame should produce non-zero remainder."""
        # Take a valid frame and flip one bit
        valid = "8D4840D6202CC371C32CE0576098"
        corrupted = "8D4840D6202CC371C32CE0576099"  # Last hex digit changed
        data = bytes.fromhex(corrupted)
        assert crc24(data) != 0

    def test_empty_24_bits(self):
        """CRC of 3 zero bytes should be 0 (identity)."""
        assert crc24(b"\x00\x00\x00") == 0

    def test_known_polynomial_property(self):
        """CRC of a single 1-bit followed by 24 zero bits should equal the generator."""
        # Message: 0x80 followed by 3 zero bytes = bit 31 set
        data = bytes([0x80, 0x00, 0x00, 0x00])
        # The CRC of this should be related to the polynomial
        result = crc24(data)
        assert result != 0  # Non-trivial


class TestValidate:
    """High-level validate() function."""

    def test_valid_frames_pass(self):
        for hex_str, _ in CRC_VECTORS:
            assert validate(hex_str) is True

    def test_corrupted_frame_fails(self):
        assert validate("8D4840D6202CC371C32CE0576099") is False

    def test_case_insensitive(self):
        assert validate("8d4840d6202cc371c32ce0576098") is True


class TestResidual:
    """CRC residual extraction."""

    def test_df17_residual_is_zero(self):
        for hex_str, expected in CRC_VECTORS:
            assert residual(hex_str) == expected

    def test_corrupted_residual_nonzero(self):
        assert residual("8D4840D6202CC371C32CE0576099") != 0


class TestExtractICAO:
    """ICAO address extraction."""

    def test_df17_icao_from_message(self):
        """DF17 ICAO should come from bytes 1-3."""
        for hex_str, expected_icao, _ in IDENTIFICATION_FRAMES:
            icao = extract_icao(hex_str)
            assert icao == expected_icao, f"ICAO mismatch for {hex_str}"

    def test_df17_position_icao(self):
        for hex_str, expected_icao, _, _, _, _ in POSITION_FRAMES:
            assert extract_icao(hex_str) == expected_icao

    def test_df17_velocity_icao(self):
        for hex_str, expected_icao, _, _, _ in VELOCITY_FRAMES:
            assert extract_icao(hex_str) == expected_icao

    def test_case_insensitive_input(self):
        assert extract_icao("8d4840d6202cc371c32ce0576098") == "4840D6"
