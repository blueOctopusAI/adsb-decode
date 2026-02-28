"""Tests for CPR (Compact Position Reporting) decode."""

import pytest

from src.cpr import global_decode, local_decode, _nl
from tests.fixtures.known_frames import POSITION_FRAMES, POSITION_DECODED


class TestNLFunction:
    """Number of longitude zones at a given latitude."""

    def test_equator(self):
        """At the equator, NL should be 59."""
        assert _nl(0.0) == 59

    def test_poles(self):
        """Near the poles, NL should be 1."""
        assert _nl(87.0) == 1
        assert _nl(-87.0) == 1
        assert _nl(89.0) == 1

    def test_mid_latitude(self):
        """Mid-latitudes should have intermediate NL values."""
        nl = _nl(52.0)  # Netherlands area
        assert 1 < nl < 59

    def test_symmetric(self):
        """NL should be symmetric for positive/negative latitudes."""
        assert _nl(45.0) == _nl(-45.0)
        assert _nl(30.0) == _nl(-30.0)

    def test_monotonic_decrease(self):
        """NL should decrease as latitude increases from equator."""
        prev = _nl(0.0)
        for lat in range(10, 90, 10):
            current = _nl(float(lat))
            assert current <= prev
            prev = current


class TestGlobalDecode:
    """Global CPR decode from even/odd frame pairs."""

    def test_known_position(self):
        """Decode the published test vector position."""
        _, _, _, _, lat_even, lon_even = POSITION_FRAMES[0]
        _, _, _, _, lat_odd, lon_odd = POSITION_FRAMES[1]

        result = global_decode(
            lat_even=lat_even,
            lon_even=lon_even,
            lat_odd=lat_odd,
            lon_odd=lon_odd,
            t_even=1.0,  # Even is more recent (matches published expected values)
            t_odd=0.5,
        )

        assert result is not None
        lat, lon = result
        assert abs(lat - POSITION_DECODED["lat"]) < 0.01
        assert abs(lon - POSITION_DECODED["lon"]) < 0.01

    def test_even_more_recent(self):
        """When even frame is more recent, use it for longitude."""
        _, _, _, _, lat_even, lon_even = POSITION_FRAMES[0]
        _, _, _, _, lat_odd, lon_odd = POSITION_FRAMES[1]

        result = global_decode(
            lat_even=lat_even,
            lon_even=lon_even,
            lat_odd=lat_odd,
            lon_odd=lon_odd,
            t_even=1.0,  # Even is more recent
            t_odd=0.5,
        )

        assert result is not None
        lat, lon = result
        assert abs(lat - POSITION_DECODED["lat"]) < 0.01
        assert abs(lon - POSITION_DECODED["lon"]) < 0.01

    def test_stale_pair_rejected(self):
        """Pairs older than 10 seconds should be rejected."""
        _, _, _, _, lat_even, lon_even = POSITION_FRAMES[0]
        _, _, _, _, lat_odd, lon_odd = POSITION_FRAMES[1]

        result = global_decode(
            lat_even=lat_even,
            lon_even=lon_even,
            lat_odd=lat_odd,
            lon_odd=lon_odd,
            t_even=0.0,
            t_odd=15.0,  # 15 seconds apart
        )

        assert result is None

    def test_returns_reasonable_coordinates(self):
        """Decoded coordinates should be in valid geographic range."""
        _, _, _, _, lat_even, lon_even = POSITION_FRAMES[0]
        _, _, _, _, lat_odd, lon_odd = POSITION_FRAMES[1]

        result = global_decode(
            lat_even=lat_even,
            lon_even=lon_even,
            lat_odd=lat_odd,
            lon_odd=lon_odd,
            t_even=0.0,
            t_odd=0.5,
        )

        assert result is not None
        lat, lon = result
        assert -90 <= lat <= 90
        assert -180 <= lon <= 180


class TestLocalDecode:
    """Local CPR decode with a reference position."""

    def test_with_known_reference(self):
        """Local decode using a nearby reference should produce accurate result."""
        _, _, _, _, cpr_lat, cpr_lon = POSITION_FRAMES[0]  # Even frame

        # Use the expected decoded position as reference (within 180nm)
        ref_lat = POSITION_DECODED["lat"]
        ref_lon = POSITION_DECODED["lon"]

        lat, lon = local_decode(
            cpr_lat=cpr_lat,
            cpr_lon=cpr_lon,
            cpr_odd=False,  # Even frame
            ref_lat=ref_lat,
            ref_lon=ref_lon,
        )

        assert abs(lat - ref_lat) < 0.1
        assert abs(lon - ref_lon) < 0.1

    def test_odd_frame(self):
        """Local decode with odd frame."""
        _, _, _, _, cpr_lat, cpr_lon = POSITION_FRAMES[1]  # Odd frame

        ref_lat = POSITION_DECODED["lat"]
        ref_lon = POSITION_DECODED["lon"]

        lat, lon = local_decode(
            cpr_lat=cpr_lat,
            cpr_lon=cpr_lon,
            cpr_odd=True,
            ref_lat=ref_lat,
            ref_lon=ref_lon,
        )

        assert abs(lat - ref_lat) < 0.1
        assert abs(lon - ref_lon) < 0.1

    def test_coordinates_in_range(self):
        """Local decode should produce valid geographic coordinates."""
        _, _, _, _, cpr_lat, cpr_lon = POSITION_FRAMES[0]

        lat, lon = local_decode(
            cpr_lat=cpr_lat,
            cpr_lon=cpr_lon,
            cpr_odd=False,
            ref_lat=52.0,
            ref_lon=4.0,
        )

        assert -90 <= lat <= 90
        assert -180 <= lon <= 180
