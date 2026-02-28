#!/usr/bin/env bash
# Install RTL-SDR drivers and tools on macOS.
# Usage: bash scripts/setup-rtlsdr.sh

set -euo pipefail

echo "=== RTL-SDR Setup ==="

# Check for Homebrew
if ! command -v brew &>/dev/null; then
    echo "ERROR: Homebrew not found. Install from https://brew.sh"
    exit 1
fi

# Install librtlsdr (includes rtl_adsb, rtl_test, rtl_sdr)
echo "Installing librtlsdr..."
brew install librtlsdr

# Verify installation
echo ""
echo "Checking installation..."
if command -v rtl_test &>/dev/null; then
    echo "OK: rtl_test found at $(which rtl_test)"
else
    echo "ERROR: rtl_test not found after install"
    exit 1
fi

if command -v rtl_adsb &>/dev/null; then
    echo "OK: rtl_adsb found at $(which rtl_adsb)"
else
    echo "WARN: rtl_adsb not found — may need to build from source"
fi

echo ""
echo "Done. Plug in your RTL-SDR dongle and run: bash scripts/validate-hardware.sh"
