#!/bin/bash
# Deploy adsb-decode Rust binary to VPS
# Usage: ADSB_VPS_HOST=ubuntu@1.2.3.4 bash deploy/deploy.sh
set -e

HOST="${ADSB_VPS_HOST:?Set ADSB_VPS_HOST=user@host}"

# Detect target architecture
ARCH=$(ssh "$HOST" uname -m)
case "$ARCH" in
    aarch64) ASSET="adsb-server-aarch64-unknown-linux-gnu" ;;
    x86_64)  ASSET="adsb-server-x86_64-unknown-linux-gnu-timescaledb" ;;
    *)       echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

REPO="blueOctopusAI/adsb-decode"

echo "Deploying to ${HOST} (${ARCH})..."

# Get latest release download URL
DOWNLOAD_URL=$(curl -s "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep "browser_download_url.*${ASSET}.tar.gz" \
    | head -1 \
    | cut -d '"' -f 4)

if [ -z "$DOWNLOAD_URL" ]; then
    echo "Error: no release binary found for ${ASSET}"
    exit 1
fi

echo "Downloading ${DOWNLOAD_URL}..."

ssh "$HOST" bash -s <<REMOTE
set -e
cd /opt/adsb-decode

# Download and extract new binary
sudo curl -sL -o /tmp/adsb-release.tar.gz "${DOWNLOAD_URL}"
sudo tar -xzf /tmp/adsb-release.tar.gz -C /tmp/
sudo chmod +x /tmp/adsb

# Atomic swap
sudo systemctl stop adsb-decode || true
sudo mv /tmp/adsb ./adsb
sudo systemctl start adsb-decode
sudo rm -f /tmp/adsb-release.tar.gz

echo "Deploy complete. Binary updated and service restarted."
REMOTE

echo "Done."
