"""Shared test fixtures for adsb-decode.

Provides:
- tmp_path database instances
- Known ADS-B frame hex strings with expected decode results
- Sample IQ data fragments
- Receiver location fixtures
"""

import pytest


# Default receiver location for tests (Western North Carolina)
RECEIVER_LAT = 35.1826
RECEIVER_LON = -83.3813


@pytest.fixture
def receiver_location():
    """Default receiver coordinates (Western NC)."""
    return (RECEIVER_LAT, RECEIVER_LON)
