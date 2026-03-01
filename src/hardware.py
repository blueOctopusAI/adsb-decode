"""Hardware detection — find RTL-SDR dongles and verify driver setup.

Scans USB devices for known RTL-SDR chip IDs (RTL2832U, R820T, etc.)
and tests capture capability.
"""

from __future__ import annotations

import subprocess
import sys


# Known RTL-SDR USB vendor:product pairs
RTL_SDR_DEVICES = {
    "0bda:2832": "RTL2832U",
    "0bda:2838": "RTL2838 (RTL-SDR Blog v3/v4)",
    "1d50:6089": "Airspy R2",
}


def find_dongles() -> list[dict]:
    """Scan USB bus for RTL-SDR devices.

    Returns list of {vendor_product, description, bus, device} dicts.
    Works on macOS (system_profiler) and Linux (lsusb).
    """
    if sys.platform == "darwin":
        return _find_dongles_macos()
    else:
        return _find_dongles_linux()


def _find_dongles_macos() -> list[dict]:
    """Use system_profiler to find USB devices on macOS."""
    devices = []
    try:
        out = subprocess.check_output(
            ["system_profiler", "SPUSBDataType", "-detailLevel", "mini"],
            text=True, timeout=10,
        )
        # Look for RTL-SDR vendor/product IDs in output
        lines = out.splitlines()
        for i, line in enumerate(lines):
            lower = line.lower()
            if "realtek" in lower or "rtl" in lower or "0bda" in lower or "rtl-sdr" in lower:
                name = line.strip().rstrip(":")
                devices.append({
                    "description": name,
                    "vendor_product": "0bda:2838",
                    "source": "system_profiler",
                })
    except (subprocess.SubprocessError, FileNotFoundError):
        pass
    return devices


def _find_dongles_linux() -> list[dict]:
    """Use lsusb to find USB devices on Linux."""
    devices = []
    try:
        out = subprocess.check_output(["lsusb"], text=True, timeout=10)
        for line in out.splitlines():
            for vid_pid, desc in RTL_SDR_DEVICES.items():
                if vid_pid in line:
                    devices.append({
                        "description": f"{desc} ({line.strip()})",
                        "vendor_product": vid_pid,
                        "source": "lsusb",
                    })
    except (subprocess.SubprocessError, FileNotFoundError):
        pass
    return devices


def check_rtl_tools() -> dict:
    """Check if RTL-SDR tools and Python bindings are installed.

    Returns dict with: rtl_test, rtl_adsb, pyrtlsdr, librtlsdr
    Each is True/False.
    """
    results = {}

    for tool in ("rtl_test", "rtl_adsb"):
        try:
            subprocess.check_output(
                ["which", tool], text=True, timeout=5, stderr=subprocess.DEVNULL
            )
            results[tool] = True
        except (subprocess.SubprocessError, FileNotFoundError):
            results[tool] = False

    try:
        import rtlsdr  # noqa: F401
        results["pyrtlsdr"] = True
    except ImportError:
        results["pyrtlsdr"] = False

    # Check librtlsdr shared library
    try:
        if sys.platform == "darwin":
            subprocess.check_output(
                ["brew", "list", "librtlsdr"],
                text=True, timeout=5, stderr=subprocess.DEVNULL,
            )
            results["librtlsdr"] = True
        else:
            subprocess.check_output(
                ["ldconfig", "-p"],
                text=True, timeout=5, stderr=subprocess.DEVNULL,
            )
            # Just check if dpkg knows about it
            try:
                subprocess.check_output(
                    ["dpkg", "-s", "librtlsdr-dev"],
                    text=True, timeout=5, stderr=subprocess.DEVNULL,
                )
                results["librtlsdr"] = True
            except (subprocess.SubprocessError, FileNotFoundError):
                results["librtlsdr"] = False
    except (subprocess.SubprocessError, FileNotFoundError):
        results["librtlsdr"] = False

    return results


def test_capture(seconds: int = 5) -> dict:
    """Run a quick test capture and report results.

    Returns {success, frames, duration_sec, method, error}.
    """
    import time
    from .capture import LiveCapture

    result = {"success": False, "frames": 0, "duration_sec": seconds, "method": "", "error": ""}

    try:
        cap = LiveCapture()
        cap.start()
        result["method"] = cap.source_name

        start = time.time()
        count = 0
        for _frame in cap:
            count += 1
            if time.time() - start >= seconds:
                break

        cap.stop()
        result["success"] = True
        result["frames"] = count

    except ImportError as e:
        result["error"] = f"Missing dependency: {e}"
    except Exception as e:
        result["error"] = str(e)

    return result
