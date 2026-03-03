#!/bin/bash
# Server setup for adsb-decode on Ubuntu 22.04 (Lightsail / EC2 / VPS)
# Installs: PostgreSQL 16 + TimescaleDB, Caddy, adsb binary
# Run as root or with sudo
set -e

echo "=== adsb-decode server setup ==="

# System packages
apt-get update
apt-get install -y curl git ufw fail2ban gnupg lsb-release

# -----------------------------------------------------------------------
# PostgreSQL 16
# -----------------------------------------------------------------------
echo "=== Installing PostgreSQL 16 ==="
sh -c 'echo "deb http://apt.postgresql.org/pub/repos/apt $(lsb_release -cs)-pgdg main" > /etc/apt/sources.list.d/pgdg.list'
curl -fsSL https://www.postgresql.org/media/keys/ACCC4CF8.asc | gpg --dearmor -o /etc/apt/trusted.gpg.d/postgresql.gpg
apt-get update
apt-get install -y postgresql-16

# -----------------------------------------------------------------------
# TimescaleDB 2
# -----------------------------------------------------------------------
echo "=== Installing TimescaleDB ==="
echo "deb https://packagecloud.io/timescale/timescaledb/ubuntu/ $(lsb_release -cs) main" > /etc/apt/sources.list.d/timescaledb.list
curl -fsSL https://packagecloud.io/timescale/timescaledb/gpgkey | gpg --dearmor -o /etc/apt/trusted.gpg.d/timescaledb.gpg
apt-get update
apt-get install -y timescaledb-2-postgresql-16

# Configure TimescaleDB
timescaledb-tune --quiet --yes

systemctl restart postgresql

# Create database user and database
sudo -u postgres psql -c "CREATE USER adsb WITH PASSWORD 'changeme';" 2>/dev/null || true
sudo -u postgres psql -c "CREATE DATABASE adsb OWNER adsb;" 2>/dev/null || true
sudo -u postgres psql -d adsb -c "CREATE EXTENSION IF NOT EXISTS timescaledb;"

echo "IMPORTANT: Change the 'adsb' database password!"
echo "  sudo -u postgres psql -c \"ALTER USER adsb PASSWORD 'your-secure-password';\""

# -----------------------------------------------------------------------
# Caddy (reverse proxy with auto-HTTPS)
# -----------------------------------------------------------------------
echo "=== Installing Caddy ==="
apt-get install -y debian-keyring debian-archive-keyring apt-transport-https
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | tee /etc/apt/sources.list.d/caddy-stable.list
apt-get update
apt-get install -y caddy

# -----------------------------------------------------------------------
# Application setup
# -----------------------------------------------------------------------

# Create service user
useradd -r -s /bin/false adsb || true

# Setup directories
mkdir -p /opt/adsb-decode/data
chown -R adsb:adsb /opt/adsb-decode

# Create environment file
cat > /opt/adsb-decode/.env <<'ENVEOF'
DATABASE_URL=postgres://adsb:changeme@localhost/adsb
ENVEOF
chmod 600 /opt/adsb-decode/.env
chown adsb:adsb /opt/adsb-decode/.env

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
echo "  1. Set a secure database password:"
echo "     sudo -u postgres psql -c \"ALTER USER adsb PASSWORD 'your-password';\""
echo "     Then update /opt/adsb-decode/.env with the new password"
echo "  2. Edit /etc/caddy/Caddyfile — verify domain is correct"
echo "  3. sudo systemctl start caddy"
echo "  4. sudo systemctl start adsb-decode"
echo "  5. Point your domain's DNS A record to this server's IP"
echo "  6. Register your first receiver at https://your-domain/register"
