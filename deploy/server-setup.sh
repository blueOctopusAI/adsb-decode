#!/bin/bash
# Server setup for adsb-decode on Ubuntu (Lightsail / EC2 / VPS)
# Run as root or with sudo
set -e

echo "=== adsb-decode server setup ==="

# System packages
apt-get update
apt-get install -y python3 python3-venv python3-pip git ufw fail2ban

# Install Caddy
apt-get install -y debian-keyring debian-archive-keyring apt-transport-https
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | tee /etc/apt/sources.list.d/caddy-stable.list
apt-get update
apt-get install -y caddy

# Create service user
useradd -r -s /bin/false adsb || true

# Clone repo
mkdir -p /opt/adsb-decode
cd /opt/adsb-decode
if [ ! -d .git ]; then
    git clone https://github.com/blueOctopusAI/adsb-decode.git .
else
    git pull origin main
fi

# Python environment
python3 -m venv venv
./venv/bin/pip install -e ".[web]"

# Data directory
mkdir -p /opt/adsb-decode/data
chown -R adsb:adsb /opt/adsb-decode

# Install service
cp deploy/adsb-decode.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable adsb-decode

# Install Caddy config
cp deploy/Caddyfile /etc/caddy/Caddyfile
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
echo "  2. Edit /etc/systemd/system/adsb-decode.service — set ADSB_INGEST_KEY"
echo "  3. sudo systemctl daemon-reload"
echo "  4. sudo systemctl start caddy"
echo "  5. sudo systemctl start adsb-decode"
echo "  6. Point your domain's DNS A record to this server's IP"
