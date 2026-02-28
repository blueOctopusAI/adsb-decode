"""Shared test fixtures for adsb-decode.

Provides:
- tmp_path database instances
- Known ADS-B frame hex strings with expected decode results
- Sample IQ data fragments
- Receiver location fixtures
"""

import pytest


# Asheville, NC — default receiver location for tests
RECEIVER_LAT = 35.5951
RECEIVER_LON = -82.5515


@pytest.fixture
def receiver_location():
    """Default receiver coordinates (Asheville, NC)."""
    return (RECEIVER_LAT, RECEIVER_LON)
