"""Tests for hardware detection."""

from src.hardware import find_dongles, check_rtl_tools, RTL_SDR_DEVICES


class TestHardware:
    def test_find_dongles_returns_list(self):
        result = find_dongles()
        assert isinstance(result, list)

    def test_check_rtl_tools_returns_dict(self):
        result = check_rtl_tools()
        assert isinstance(result, dict)
        assert "pyrtlsdr" in result
        assert "rtl_test" in result
        assert "rtl_adsb" in result

    def test_known_devices(self):
        assert "0bda:2838" in RTL_SDR_DEVICES
        assert "RTL2838" in RTL_SDR_DEVICES["0bda:2838"]
