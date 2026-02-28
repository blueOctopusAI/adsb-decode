#!/usr/bin/env bash
# Capture raw IQ samples from RTL-SDR at 1090 MHz.
# Usage: bash scripts/capture-iq.sh [duration_seconds]
#
# Output: data/capture_YYYYMMDD_HHMMSS.iq
# Format: Interleaved uint8 IQ pairs at 2 MHz sample rate

set -euo pipefail

DURATION=${1:-60}
FREQ=1090000000      # 1090 MHz
SAMPLE_RATE=2000000  # 2 MHz
GAIN=40              # RTL-SDR gain (dB) — adjust for local conditions

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
OUTPUT="data/capture_${TIMESTAMP}.iq"

mkdir -p data

echo "=== IQ Capture ==="
echo "Frequency:   1090 MHz"
echo "Sample rate: 2 MHz"
echo "Duration:    ${DURATION}s"
echo "Output:      ${OUTPUT}"
echo ""

# rtl_sdr captures raw IQ samples
# -f frequency, -s sample rate, -g gain, -n number of samples
NUM_SAMPLES=$((SAMPLE_RATE * DURATION))

echo "Capturing ${NUM_SAMPLES} samples ($(( NUM_SAMPLES * 2 / 1024 / 1024 )) MB)..."
rtl_sdr -f ${FREQ} -s ${SAMPLE_RATE} -g ${GAIN} -n ${NUM_SAMPLES} "${OUTPUT}"

echo ""
echo "Capture complete: ${OUTPUT} ($(du -h "${OUTPUT}" | cut -f1))"
echo "Decode with: adsb decode ${OUTPUT}"
