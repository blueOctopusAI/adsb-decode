#!/usr/bin/env bash
set -euo pipefail

# adsb-receiver setup script
# Downloads the latest release binary, installs RTL-SDR drivers,
# and configures the receiver as a systemd service.

INSTALL_DIR="/opt/adsb-receiver"
SERVICE_NAME="adsb-receiver"
ENV_FILE="/etc/adsb-receiver.env"
REPO="blueOctopusAI/adsb-decode"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

info() { echo -e "${GREEN}[+]${NC} $1"; }
warn() { echo -e "${YELLOW}[!]${NC} $1"; }
error() { echo -e "${RED}[x]${NC} $1"; exit 1; }

# Detect architecture
detect_arch() {
    local arch
    arch=$(uname -m)
    case "$arch" in
        x86_64)  echo "x86_64-unknown-linux-gnu" ;;
        aarch64) echo "aarch64-unknown-linux-gnu" ;;
        armv7l)  echo "armv7-unknown-linux-gnueabihf" ;;
        *)       error "Unsupported architecture: $arch" ;;
    esac
}

# Check for root
if [ "$EUID" -ne 0 ]; then
    error "This script must be run as root (sudo)"
fi

TARGET=$(detect_arch)
info "Detected architecture: $TARGET"

# Step 1: Install RTL-SDR drivers
info "Installing RTL-SDR drivers..."
if command -v apt-get &>/dev/null; then
    apt-get update -qq
    apt-get install -y -qq rtl-sdr librtlsdr-dev
elif command -v dnf &>/dev/null; then
    dnf install -y rtl-sdr rtl-sdr-devel
elif command -v pacman &>/dev/null; then
    pacman -S --noconfirm rtl-sdr
else
    warn "Could not detect package manager. Install rtl-sdr manually."
fi

# Step 2: Blacklist DVB kernel module (conflicts with RTL-SDR)
BLACKLIST_FILE="/etc/modprobe.d/blacklist-rtlsdr.conf"
if [ ! -f "$BLACKLIST_FILE" ]; then
    info "Blacklisting dvb_usb_rtl28xxu kernel module..."
    echo "blacklist dvb_usb_rtl28xxu" > "$BLACKLIST_FILE"
    # Unload if currently loaded
    modprobe -r dvb_usb_rtl28xxu 2>/dev/null || true
else
    info "DVB kernel module already blacklisted"
fi

# Step 3: Download latest release binary
info "Downloading adsb-receiver for $TARGET..."
mkdir -p "$INSTALL_DIR"

DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/adsb-receiver-${TARGET}.tar.gz"
if ! curl -sL "$DOWNLOAD_URL" | tar xz -C "$INSTALL_DIR"; then
    error "Failed to download binary. Check https://github.com/${REPO}/releases for available builds."
fi
chmod +x "${INSTALL_DIR}/adsb-receiver"
info "Installed to ${INSTALL_DIR}/adsb-receiver"

# Step 4: Create environment file (if not exists)
if [ ! -f "$ENV_FILE" ]; then
    info "Creating environment file at $ENV_FILE..."
    cat > "$ENV_FILE" << 'ENVEOF'
# adsb-receiver configuration
# Edit these values for your setup

# Required: server URL and receiver name
ADSB_SERVER=https://adsb.blueoctopustechnology.com
ADSB_NAME=my-receiver

# Optional: API key from /register page
# ADSB_API_KEY=your-api-key-here

# Optional: receiver location (improves local CPR decoding)
# ADSB_LAT=35.0
# ADSB_LON=-83.0

# Optional: RTL-SDR settings
# ADSB_DEVICE=0
# ADSB_GAIN=400
# ADSB_PPM=0
ENVEOF
    chmod 600 "$ENV_FILE"
    warn "Edit $ENV_FILE with your server URL, receiver name, and API key"
else
    info "Environment file already exists at $ENV_FILE"
fi

# Step 5: Install systemd service
info "Installing systemd service..."
cp "$(dirname "$0")/adsb-receiver.service" "/etc/systemd/system/${SERVICE_NAME}.service" 2>/dev/null || \
cat > "/etc/systemd/system/${SERVICE_NAME}.service" << 'SVCEOF'
[Unit]
Description=adsb-decode receiver
After=network-online.target
Wants=network-online.target

[Service]
EnvironmentFile=/etc/adsb-receiver.env
ExecStart=/opt/adsb-receiver/adsb-receiver
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
SVCEOF

systemctl daemon-reload

# Step 6: Verify
info "Verifying installation..."
if "${INSTALL_DIR}/adsb-receiver" --help &>/dev/null; then
    info "Binary runs successfully"
else
    warn "Binary may not be compatible with this system"
fi

if command -v rtl_adsb &>/dev/null; then
    info "rtl_adsb found in PATH"
else
    warn "rtl_adsb not found — make sure rtl-sdr is installed"
fi

echo ""
info "Setup complete!"
echo ""
echo "  Next steps:"
echo "  1. Register your receiver at https://adsb.blueoctopustechnology.com/register"
echo "  2. Edit $ENV_FILE with your API key and receiver name"
echo "  3. Plug in your RTL-SDR dongle"
echo "  4. Start the service:"
echo ""
echo "     sudo systemctl enable --now $SERVICE_NAME"
echo ""
echo "  5. Check status:"
echo ""
echo "     sudo systemctl status $SERVICE_NAME"
echo "     sudo journalctl -u $SERVICE_NAME -f"
echo ""
echo "  6. Verify at https://adsb.blueoctopustechnology.com/receivers"
echo ""
