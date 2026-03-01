#!/bin/bash
# Deploy adsb-decode Rust binary to VPS
# Usage: ADSB_VPS_HOST=ubuntu@1.2.3.4 bash deploy/deploy.sh
set -e

HOST="${ADSB_VPS_HOST:?Set ADSB_VPS_HOST=user@host}"

# Detect target architecture
ARCH=$(ssh "$HOST" uname -m)
case "$ARCH" in
    aarch64) TARGET="aarch64-unknown-linux-musl" ;;
    x86_64)  TARGET="x86_64-unknown-linux-musl" ;;
    *)       echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

REPO="blueOctopusAI/adsb-decode"
ASSET="adsb-${TARGET}"

echo "Deploying to ${HOST} (${ARCH})..."

# Get latest release download URL
DOWNLOAD_URL=$(curl -s "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep "browser_download_url.*${ASSET}" \
    | head -1 \
    | cut -d '"' -f 4)

if [ -z "$DOWNLOAD_URL" ]; then
    echo "Error: no release binary found for ${ASSET}"
    echo "Build with: cross build --release --target ${TARGET}"
    exit 1
fi

echo "Downloading ${DOWNLOAD_URL}..."

ssh "$HOST" bash -s <<REMOTE
set -e
cd /opt/adsb-decode

# Download new binary
sudo curl -sL -o adsb.new "${DOWNLOAD_URL}"
sudo chmod +x adsb.new

# Atomic swap
sudo systemctl stop adsb-decode || true
sudo mv adsb.new adsb
sudo systemctl start adsb-decode

echo "Deploy complete. Binary updated and service restarted."
REMOTE

echo "Done."
