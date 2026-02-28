"""Tests for ICAO address resolution."""

import pytest

from src.icao import lookup_country, is_military, icao_to_n_number
from tests.fixtures.known_frames import ICAO_VECTORS


class TestLookupCountry:
    """Country identification from ICAO address."""

    def test_known_vectors(self):
        for icao_hex, expected_country, _ in ICAO_VECTORS:
            country = lookup_country(icao_hex)
            assert country == expected_country, f"Country mismatch for {icao_hex}"

    def test_us_address(self):
        assert lookup_country("A00001") == "United States"
        assert lookup_country("AFFFFF") == "United States"

    def test_uk_address(self):
        assert lookup_country("400000") == "United Kingdom"
        assert lookup_country("43FFFF") == "United Kingdom"

    def test_germany(self):
        assert lookup_country("3C0000") == "Germany"

    def test_netherlands(self):
        assert lookup_country("480000") == "Netherlands"

    def test_unallocated_returns_none(self):
        assert lookup_country("FFFFFF") is None


class TestIsMilitary:
    """Military aircraft detection."""

    def test_us_military_block(self):
        assert is_military("ADF7C8") is True  # First US military address
        assert is_military("AFFFFF") is True
        assert is_military("ADF7C7") is False  # Last US civil address

    def test_known_vectors(self):
        for icao_hex, _, expected_military in ICAO_VECTORS:
            assert is_military(icao_hex) == expected_military, f"Military flag wrong for {icao_hex}"

    def test_military_callsign(self):
        assert is_military("A00001", callsign="RCH123") is True
        assert is_military("A00001", callsign="REACH01") is True
        assert is_military("A00001", callsign="DUKE42") is True
        assert is_military("A00001", callsign="DOOM11") is True

    def test_civilian_callsign(self):
        assert is_military("A00001", callsign="DAL123") is False
        assert is_military("A00001", callsign="UAL456") is False

    def test_callsign_case_insensitive(self):
        assert is_military("A00001", callsign="rch123") is True

    def test_no_callsign(self):
        assert is_military("A00001") is False


class TestNNumber:
    """US N-number (tail number) conversion."""

    def test_first_civil_address(self):
        result = icao_to_n_number("A00001")
        assert result is not None
        assert result.startswith("N")

    def test_non_us_returns_none(self):
        assert icao_to_n_number("400000") is None  # UK
        assert icao_to_n_number("3C0000") is None  # Germany

    def test_military_returns_none(self):
        assert icao_to_n_number("ADF7C8") is None  # US military

    def test_n_number_format(self):
        result = icao_to_n_number("A00001")
        assert result[0] == "N"
        # Rest should be digits and possibly one trailing letter
        assert result[1:].replace("A", "").replace("B", "").isdigit() or result[1:].isdigit()

    def test_known_address_produces_valid_nnumber(self):
        # A few spot checks on the address space
        for offset in [0, 1000, 50000, 100000, 500000]:
            addr = 0xA00001 + offset
            if addr <= 0xADF7C7:
                result = icao_to_n_number(f"{addr:06X}")
                assert result is not None, f"N-number failed for {addr:06X}"
                assert result.startswith("N")
