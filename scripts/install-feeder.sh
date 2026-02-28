#!/bin/bash
# Install adsb-decode feeder agent on a Raspberry Pi or similar device
# Usage: bash install-feeder.sh
set -e

echo "=== adsb-decode feeder setup ==="

# Install rtl-sdr
if ! command -v rtl_adsb &>/dev/null; then
    echo "Installing rtl-sdr..."
    if [[ "$(uname)" == "Darwin" ]]; then
        brew install librtlsdr
    else
        sudo apt-get update
        sudo apt-get install -y rtl-sdr python3 python3-pip
    fi
fi

# Test dongle
echo "Testing RTL-SDR dongle..."
if rtl_test -t 2>&1 | grep -q "Found"; then
    echo "Dongle detected."
else
    echo "WARNING: No RTL-SDR dongle detected. Plug one in and try again."
fi

# Install Python deps
pip3 install requests

echo ""
echo "=== Feeder ready ==="
echo ""
echo "Start with:"
echo "  python3 -m src.feeder \\"
echo "    --server https://your-server.com \\"
echo "    --name my-receiver \\"
echo "    --key your-api-key \\"
echo "    --lat 35.18 --lon -83.38"
echo ""
echo "Or as a systemd service — see deploy/feeder.service"
