#!/usr/bin/env bash
# Capture pre-demodulated ADS-B frames using rtl_adsb.
# Usage: bash scripts/capture-frames.sh [duration_seconds]
#
# Output: data/frames_YYYYMMDD_HHMMSS.txt
# Format: One hex frame string per line (already demodulated + CRC checked by rtl_adsb)
#
# This bypasses our demodulator — useful for testing the decode pipeline
# without needing to debug DSP issues first.

set -euo pipefail

DURATION=${1:-60}
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
OUTPUT="data/frames_${TIMESTAMP}.txt"

mkdir -p data

if ! command -v rtl_adsb &>/dev/null; then
    echo "ERROR: rtl_adsb not found. Run: bash scripts/setup-rtlsdr.sh"
    exit 1
fi

echo "=== Frame Capture ==="
echo "Duration: ${DURATION}s"
echo "Output:   ${OUTPUT}"
echo ""
echo "Listening for ADS-B frames on 1090 MHz..."

# rtl_adsb outputs hex frame strings, one per line
# timeout kills it after DURATION seconds
timeout "${DURATION}" rtl_adsb 2>/dev/null > "${OUTPUT}" || true

FRAME_COUNT=$(wc -l < "${OUTPUT}" | tr -d ' ')
echo ""
echo "Capture complete: ${FRAME_COUNT} frames in ${OUTPUT}"
echo "Decode with: adsb decode ${OUTPUT}"
