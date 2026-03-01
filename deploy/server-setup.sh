#!/bin/bash
# Server setup for adsb-decode on Ubuntu (Lightsail / EC2 / VPS)
# Run as root or with sudo
set -e

echo "=== adsb-decode server setup ==="

# System packages
apt-get update
apt-get install -y curl git ufw fail2ban

# Install Caddy
apt-get install -y debian-keyring debian-archive-keyring apt-transport-https
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | tee /etc/apt/sources.list.d/caddy-stable.list
apt-get update
apt-get install -y caddy

# Create service user
useradd -r -s /bin/false adsb || true

# Setup directories
mkdir -p /opt/adsb-decode/data
chown -R adsb:adsb /opt/adsb-decode

# Download latest release binary
ARCH=$(uname -m)
case "$ARCH" in
    aarch64) TARGET="aarch64-unknown-linux-musl" ;;
    x86_64)  TARGET="x86_64-unknown-linux-musl" ;;
    *)       echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

REPO="blueOctopusAI/adsb-decode"
ASSET="adsb-${TARGET}"

DOWNLOAD_URL=$(curl -s "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep "browser_download_url.*${ASSET}" \
    | head -1 \
    | cut -d '"' -f 4)

if [ -n "$DOWNLOAD_URL" ]; then
    echo "Downloading binary..."
    curl -sL -o /opt/adsb-decode/adsb "$DOWNLOAD_URL"
    chmod +x /opt/adsb-decode/adsb
else
    echo "Warning: no release binary found. Upload manually to /opt/adsb-decode/adsb"
fi

# Install systemd service
cp deploy/adsb-decode.service /etc/systemd/system/ 2>/dev/null || \
    curl -sL "https://raw.githubusercontent.com/${REPO}/main/deploy/adsb-decode.service" \
        -o /etc/systemd/system/adsb-decode.service
systemctl daemon-reload
systemctl enable adsb-decode

# Install Caddy config
cp deploy/Caddyfile /etc/caddy/Caddyfile 2>/dev/null || \
    curl -sL "https://raw.githubusercontent.com/${REPO}/main/deploy/Caddyfile" \
        -o /etc/caddy/Caddyfile
# IMPORTANT: Edit /etc/caddy/Caddyfile to set your domain before starting

# Firewall
ufw allow 22/tcp
ufw allow 80/tcp
ufw allow 443/tcp
ufw --force enable

# Enable unattended upgrades
apt-get install -y unattended-upgrades
dpkg-reconfigure -f noninteractive unattended-upgrades

echo ""
echo "=== Setup complete ==="
echo "Next steps:"
echo "  1. Edit /etc/caddy/Caddyfile — set your domain"
echo "  2. sudo systemctl start caddy"
echo "  3. sudo systemctl start adsb-decode"
echo "  4. Point your domain's DNS A record to this server's IP"
