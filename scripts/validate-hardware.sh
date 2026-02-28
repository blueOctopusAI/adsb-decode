#!/usr/bin/env bash
# Check if RTL-SDR dongle is detected and working.
# Usage: bash scripts/validate-hardware.sh

set -euo pipefail

echo "=== RTL-SDR Hardware Validation ==="

# Check drivers installed
if ! command -v rtl_test &>/dev/null; then
    echo "ERROR: rtl_test not found. Run: bash scripts/setup-rtlsdr.sh"
    exit 1
fi

# Check USB device
echo "Checking for RTL-SDR USB device..."
if system_profiler SPUSBDataType 2>/dev/null | grep -qi "RTL"; then
    echo "OK: RTL-SDR USB device detected"
else
    echo "WARN: No RTL-SDR device found in USB tree. Is it plugged in?"
fi

# Try rtl_test (brief — just check device opens)
echo ""
echo "Testing device access (2 second test)..."
if timeout 3 rtl_test -t 2>/dev/null; then
    echo "OK: Device opened successfully"
else
    RTL_EXIT=$?
    if [ "$RTL_EXIT" -eq 124 ]; then
        # timeout killed it — that's actually fine, means it was running
        echo "OK: Device responding (test timed out normally)"
    else
        echo "ERROR: Could not open device (exit code $RTL_EXIT)"
        echo "  - Is the dongle plugged in?"
        echo "  - Is another program using it?"
        echo "  - On macOS, you may need to unload the kernel extension:"
        echo "    sudo kextunload -b com.apple.driver.AppleUSBFTDI"
        exit 1
    fi
fi

echo ""
echo "Hardware validation complete. Ready to capture."
