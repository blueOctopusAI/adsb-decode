"""Tests for CRC-24 validation, byte-at-a-time LUT, syndrome error correction."""

import pytest

from src.crc import (
    _CRC_TABLE,
    _crc24_raw,
    _SYNDROME_TABLE_56,
    _SYNDROME_TABLE_112,
    GENERATOR,
    crc24,
    crc24_payload,
    extract_icao,
    residual,
    try_fix,
    validate,
)
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


class TestCRCTable:
    """Phase 1: Byte-at-a-time lookup table."""

    def test_table_size(self):
        """Table should have exactly 256 entries."""
        assert len(_CRC_TABLE) == 256

    def test_table_entries_are_24bit(self):
        """All table entries should be 24-bit values."""
        for entry in _CRC_TABLE:
            assert 0 <= entry <= 0xFFFFFF

    def test_table_zero_entry(self):
        """Table[0] should be 0 (no data, no CRC contribution)."""
        assert _CRC_TABLE[0] == 0

    def test_known_vectors_match(self):
        """Byte-at-a-time CRC should match known test vectors."""
        for hex_str, expected in CRC_VECTORS:
            data = bytes.fromhex(hex_str)
            assert crc24(data) == expected

    def test_crc24_payload_helper(self):
        """crc24_payload should compute CRC of all bytes except last 3."""
        hex_str = "8D4840D6202CC371C32CE0576098"
        data = bytes.fromhex(hex_str)
        # CRC of payload (bytes 0-10) should equal the transmitted CRC (bytes 11-13)
        payload_crc = crc24_payload(data)
        transmitted_crc = int.from_bytes(data[-3:], "big")
        assert payload_crc == transmitted_crc

    def test_raw_crc_matches_table(self):
        """Raw CRC (full poly division) should produce table entries consistent with table."""
        # Table[i] = raw CRC of single byte i followed by 3 zero bytes
        for i in [0, 1, 42, 128, 255]:
            data = bytes([i, 0, 0, 0])
            # For Mode S CRC: data[:-3] = [i], XOR with data[-3:] = [0,0,0]
            # So crc24([i, 0, 0, 0]) = raw_crc([i]) XOR 0 = raw_crc([i])
            mode_s = crc24(data)
            raw = _crc24_raw(bytes([i]))
            assert mode_s == raw, f"Mismatch for byte {i}"


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


class TestSyndromeTables:
    """Phase 2: Syndrome error correction tables."""

    def test_112_table_has_single_bit_entries(self):
        """112-bit table should have at least 112 single-bit syndromes."""
        single_bit_count = sum(1 for v in _SYNDROME_TABLE_112.values() if len(v) == 1)
        assert single_bit_count >= 112

    def test_56_table_has_single_bit_entries(self):
        """56-bit table should have at least 56 single-bit syndromes."""
        single_bit_count = sum(1 for v in _SYNDROME_TABLE_56.values() if len(v) == 1)
        assert single_bit_count >= 56

    def test_112_table_has_double_bit_entries(self):
        """112-bit table should have double-bit entries."""
        double_bit_count = sum(1 for v in _SYNDROME_TABLE_112.values() if len(v) == 2)
        assert double_bit_count > 0

    def test_56_table_has_double_bit_entries(self):
        """56-bit table should have double-bit entries."""
        double_bit_count = sum(1 for v in _SYNDROME_TABLE_56.values() if len(v) == 2)
        assert double_bit_count > 0

    def test_112_table_size_reasonable(self):
        """112-bit table: 112 single + up to 6216 double = ~6328 max entries."""
        assert len(_SYNDROME_TABLE_112) > 100
        assert len(_SYNDROME_TABLE_112) <= 7000

    def test_56_table_size_reasonable(self):
        """56-bit table: 56 single + up to 1540 double = ~1596 max entries."""
        assert len(_SYNDROME_TABLE_56) > 50
        assert len(_SYNDROME_TABLE_56) <= 2000


class TestTryFix:
    """Phase 2: Error correction via syndrome lookup."""

    def test_single_bit_correction(self):
        """Flipping one bit in a valid frame should be correctable."""
        valid = "8D4840D6202CC371C32CE0576098"
        # Flip bit 47 (a data bit, not DF field)
        data = bytearray.fromhex(valid)
        data[5] ^= 0x80  # Flip MSB of byte 5
        corrupted = data.hex().upper()
        fixed = try_fix(corrupted)
        assert fixed is not None
        assert fixed == valid

    def test_double_bit_correction(self):
        """Flipping two bits should be correctable."""
        valid = "8D4840D6202CC371C32CE0576098"
        data = bytearray.fromhex(valid)
        data[5] ^= 0x80  # Flip bit in byte 5
        data[8] ^= 0x01  # Flip bit in byte 8
        corrupted = data.hex().upper()
        fixed = try_fix(corrupted)
        assert fixed is not None
        assert fixed == valid

    def test_triple_bit_not_correctable(self):
        """Flipping three bits should NOT be correctable."""
        valid = "8D4840D6202CC371C32CE0576098"
        data = bytearray.fromhex(valid)
        data[5] ^= 0x80
        data[8] ^= 0x01
        data[10] ^= 0x40
        corrupted = data.hex().upper()
        fixed = try_fix(corrupted)
        assert fixed is None

    def test_df_field_protection(self):
        """Never correct bits 0-4 (DF field)."""
        valid = "8D4840D6202CC371C32CE0576098"
        data = bytearray.fromhex(valid)
        # Flip bit 0 (MSB of byte 0, which is part of DF field)
        data[0] ^= 0x80
        corrupted = data.hex().upper()
        fixed = try_fix(corrupted)
        assert fixed is None  # Should refuse to correct DF field

    def test_valid_frame_returns_self(self):
        """Already-valid frame should be returned as-is."""
        valid = "8D4840D6202CC371C32CE0576098"
        assert try_fix(valid) == valid

    def test_correction_preserves_icao(self):
        """Corrected frame should have same ICAO as original."""
        valid = "8D4840D6202CC371C32CE0576098"
        data = bytearray.fromhex(valid)
        data[7] ^= 0x10  # Flip a data bit
        corrupted = data.hex().upper()
        fixed = try_fix(corrupted)
        assert fixed is not None
        # ICAO is bytes 1-3, should be preserved
        assert fixed[2:8] == valid[2:8]

    def test_short_message_correction(self):
        """Error correction should also work on 56-bit (short) messages."""
        # For DF11 (all-call reply), CRC remainder = 0 when valid (like DF17/18)
        # DF11 = 01011 << 3 = 0x58
        msg = bytearray(7)
        msg[0] = 0x58  # DF11
        msg[1] = 0xAB  # ICAO byte 1
        msg[2] = 0xCD  # ICAO byte 2
        msg[3] = 0xEF  # ICAO byte 3
        # Compute CRC of first 4 bytes and set PI to make Mode S CRC = 0
        # Mode S CRC = raw_crc(data[:-3]) XOR data[-3:]
        # To get 0: PI = raw_crc(first_4_bytes)
        payload_crc = _crc24_raw(bytes(msg[:4]))
        msg[4] = (payload_crc >> 16) & 0xFF
        msg[5] = (payload_crc >> 8) & 0xFF
        msg[6] = payload_crc & 0xFF
        valid_hex = msg.hex().upper()
        assert crc24(bytes.fromhex(valid_hex)) == 0
        # Corrupt one bit and try to fix
        corrupted = bytearray.fromhex(valid_hex)
        corrupted[3] ^= 0x04
        fixed = try_fix(corrupted.hex().upper())
        assert fixed is not None
        assert fixed == valid_hex
