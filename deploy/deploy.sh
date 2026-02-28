#!/bin/bash
# Deploy updated code to VPS
# Usage: ADSB_VPS_HOST=ubuntu@1.2.3.4 bash deploy/deploy.sh
set -e

HOST="${ADSB_VPS_HOST:?Set ADSB_VPS_HOST=user@host}"

echo "Deploying to ${HOST}..."

ssh "$HOST" bash -s <<'REMOTE'
cd /opt/adsb-decode
git pull origin main
./venv/bin/pip install -e ".[web]" --quiet
sudo systemctl restart adsb-decode
echo "Deploy complete. Service restarted."
REMOTE

echo "Done."
