#!/usr/bin/env bash
set -euo pipefail

# adsb-receiver setup script.
#
# One-command form (auto-enables service if ADSB_API_KEY is set):
#   ADSB_API_KEY=xxx ADSB_NAME=my-pi \
#     curl -sL https://raw.githubusercontent.com/blueOctopusAI/adsb-decode/main/deploy/receiver-setup.sh \
#     | sudo -E bash
#
# Manual form (writes env-file template, you edit and start the service):
#   curl -sL https://raw.githubusercontent.com/blueOctopusAI/adsb-decode/main/deploy/receiver-setup.sh | sudo bash
#
# Env vars:
#   ADSB_API_KEY    Key from /register. If set, env file is fully populated and the
#                   service is enabled+started automatically.
#   ADSB_NAME       Receiver name (default: hostname).
#   ADSB_SERVER     Server URL (default: https://adsb.blueoctopustechnology.com).
#   ADSB_LAT        Receiver latitude (improves local CPR decoding).
#   ADSB_LON        Receiver longitude.
#   ADSB_DEVICE     RTL-SDR device index (default 0).
#   ADSB_GAIN       RTL-SDR gain × 10 (default 400 = 40.0 dB).
#   ADSB_PPM        RTL-SDR ppm correction (default 0).
#   ADSB_AUTOSTART  Force service enable+start regardless of API_KEY (set to 1).
#
# Flags:
#   --dry-run       Print what would happen; touch nothing.
#   --help          Show this usage.
#
# Tunables (mostly for tests):
#   INSTALL_DIR     Default /opt/adsb-receiver.
#   ENV_FILE        Default /etc/adsb-receiver.env.
#   SERVICE_FILE    Default /etc/systemd/system/adsb-receiver.service.
#   SKIP_ROOT_CHECK If 1, skip the EUID check (test harness only).

INSTALL_DIR="${INSTALL_DIR:-/opt/adsb-receiver}"
ENV_FILE="${ENV_FILE:-/etc/adsb-receiver.env}"
SERVICE_NAME="adsb-receiver"
SERVICE_FILE="${SERVICE_FILE:-/etc/systemd/system/${SERVICE_NAME}.service}"
REPO="blueOctopusAI/adsb-decode"
SKIP_ROOT_CHECK="${SKIP_ROOT_CHECK:-0}"
DRY_RUN=0

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

info() { echo -e "${GREEN}[+]${NC} $1"; }
warn() { echo -e "${YELLOW}[!]${NC} $1"; }
error() { echo -e "${RED}[x]${NC} $1" >&2; exit 1; }

usage() {
    sed -n '3,28p' "$0" | sed 's/^# \{0,1\}//'
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) DRY_RUN=1; shift ;;
        --help|-h) usage; exit 0 ;;
        *) error "Unknown argument: $1 (try --help)" ;;
    esac
done

run() {
    if [ "$DRY_RUN" = "1" ]; then
        printf '[dry-run]'
        printf ' %q' "$@"
        printf '\n'
    else
        "$@"
    fi
}

write_file() {
    local path="$1"
    local mode="$2"
    local content="$3"
    if [ "$DRY_RUN" = "1" ]; then
        echo "[dry-run] would write $path (mode $mode):"
        printf '%s\n' "$content" | sed 's/^/    | /'
    else
        printf '%s\n' "$content" > "$path"
        chmod "$mode" "$path"
    fi
}

detect_arch() {
    if [ -n "${TARGET_OVERRIDE:-}" ]; then
        echo "$TARGET_OVERRIDE"
        return
    fi
    local arch
    arch=$(uname -m)
    case "$arch" in
        x86_64)  echo "x86_64-unknown-linux-gnu" ;;
        aarch64) echo "aarch64-unknown-linux-gnu" ;;
        armv7l)  echo "armv7-unknown-linux-gnueabihf" ;;
        *)       error "Unsupported architecture: $arch" ;;
    esac
}

if [ "$SKIP_ROOT_CHECK" != "1" ] && [ "$DRY_RUN" != "1" ] && [ "$EUID" -ne 0 ]; then
    error "This script must be run as root (sudo)"
fi

TARGET=$(detect_arch)
info "Detected architecture: $TARGET"

ADSB_SERVER="${ADSB_SERVER:-https://adsb.blueoctopustechnology.com}"
ADSB_NAME="${ADSB_NAME:-$(hostname -s 2>/dev/null || echo my-receiver)}"
ADSB_API_KEY="${ADSB_API_KEY:-}"
ADSB_LAT="${ADSB_LAT:-}"
ADSB_LON="${ADSB_LON:-}"
ADSB_DEVICE="${ADSB_DEVICE:-}"
ADSB_GAIN="${ADSB_GAIN:-}"
ADSB_PPM="${ADSB_PPM:-}"
ADSB_AUTOSTART="${ADSB_AUTOSTART:-}"

# Step 1: Install RTL-SDR drivers
info "Installing RTL-SDR drivers..."
if command -v apt-get &>/dev/null; then
    run apt-get update -qq
    run apt-get install -y -qq rtl-sdr librtlsdr-dev
elif command -v dnf &>/dev/null; then
    run dnf install -y rtl-sdr rtl-sdr-devel
elif command -v pacman &>/dev/null; then
    run pacman -S --noconfirm rtl-sdr
else
    warn "Could not detect package manager. Install rtl-sdr manually."
fi

# Step 2: Blacklist DVB kernel module (conflicts with RTL-SDR)
BLACKLIST_FILE="/etc/modprobe.d/blacklist-rtlsdr.conf"
if [ "$DRY_RUN" = "1" ] || [ ! -f "$BLACKLIST_FILE" ]; then
    info "Blacklisting dvb_usb_rtl28xxu kernel module..."
    write_file "$BLACKLIST_FILE" 644 "blacklist dvb_usb_rtl28xxu"
    if [ "$DRY_RUN" != "1" ]; then
        modprobe -r dvb_usb_rtl28xxu 2>/dev/null || true
    fi
else
    info "DVB kernel module already blacklisted"
fi

# Step 3: Download release binary
info "Downloading adsb-receiver for $TARGET..."
run mkdir -p "$INSTALL_DIR"
DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/adsb-receiver-${TARGET}.tar.gz"
if [ "$DRY_RUN" = "1" ]; then
    echo "[dry-run] would download $DOWNLOAD_URL into $INSTALL_DIR"
else
    if ! curl -sL --fail "$DOWNLOAD_URL" | tar xz -C "$INSTALL_DIR"; then
        error "Failed to download binary. Check https://github.com/${REPO}/releases for available builds."
    fi
    chmod +x "${INSTALL_DIR}/adsb-receiver"
fi
info "Installed to ${INSTALL_DIR}/adsb-receiver"

# Step 4: Render env file
build_env_file() {
    local out=""
    out+="# adsb-receiver configuration"$'\n'
    out+="# Generated by receiver-setup.sh on $(date -u +%Y-%m-%dT%H:%M:%SZ)"$'\n'
    out+="ADSB_SERVER=${ADSB_SERVER}"$'\n'
    out+="ADSB_NAME=${ADSB_NAME}"$'\n'
    if [ -n "$ADSB_API_KEY" ]; then
        out+="ADSB_API_KEY=${ADSB_API_KEY}"$'\n'
    else
        out+="# Required: get an API key from ${ADSB_SERVER}/register"$'\n'
        out+="# ADSB_API_KEY=your-api-key-here"$'\n'
    fi
    [ -n "$ADSB_LAT" ]    && out+="ADSB_LAT=${ADSB_LAT}"$'\n'
    [ -n "$ADSB_LON" ]    && out+="ADSB_LON=${ADSB_LON}"$'\n'
    [ -n "$ADSB_DEVICE" ] && out+="ADSB_DEVICE=${ADSB_DEVICE}"$'\n'
    [ -n "$ADSB_GAIN" ]   && out+="ADSB_GAIN=${ADSB_GAIN}"$'\n'
    [ -n "$ADSB_PPM" ]    && out+="ADSB_PPM=${ADSB_PPM}"$'\n'
    printf '%s' "$out"
}

if [ "$DRY_RUN" = "1" ] || [ ! -f "$ENV_FILE" ]; then
    info "Writing environment file at $ENV_FILE..."
    write_file "$ENV_FILE" 600 "$(build_env_file)"
else
    info "Environment file already exists at $ENV_FILE — leaving it alone"
fi

# Step 5: Install systemd service
info "Installing systemd service at $SERVICE_FILE..."
SERVICE_CONTENT="[Unit]
Description=adsb-decode receiver
After=network-online.target
Wants=network-online.target

[Service]
EnvironmentFile=${ENV_FILE}
ExecStart=${INSTALL_DIR}/adsb-receiver
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target"
write_file "$SERVICE_FILE" 644 "$SERVICE_CONTENT"
run systemctl daemon-reload

# Step 6: Auto-enable when API key is set (or ADSB_AUTOSTART=1)
SHOULD_AUTOSTART=0
if [ -n "$ADSB_API_KEY" ] || [ "$ADSB_AUTOSTART" = "1" ]; then
    SHOULD_AUTOSTART=1
fi

if [ "$SHOULD_AUTOSTART" = "1" ]; then
    info "Enabling and starting $SERVICE_NAME..."
    run systemctl enable --now "$SERVICE_NAME"
fi

# Step 7: Verify
if [ "$DRY_RUN" != "1" ]; then
    if "${INSTALL_DIR}/adsb-receiver" --help &>/dev/null; then
        info "Binary runs successfully"
    else
        warn "Binary may not be compatible with this system"
    fi
    command -v rtl_adsb &>/dev/null && info "rtl_adsb found in PATH" \
        || warn "rtl_adsb not found — make sure rtl-sdr is installed"
fi

echo ""
info "Setup complete!"
echo ""

if [ "$SHOULD_AUTOSTART" = "1" ]; then
    echo "  Service enabled and started. Verify with:"
    echo "    sudo systemctl status $SERVICE_NAME"
    echo "    sudo journalctl -u $SERVICE_NAME -f"
    echo "    open ${ADSB_SERVER}/receivers"
else
    echo "  Next steps:"
    echo "    1. Get an API key at ${ADSB_SERVER}/register"
    echo "    2. Edit $ENV_FILE — set ADSB_API_KEY"
    echo "    3. Plug in your RTL-SDR dongle"
    echo "    4. Start the service:"
    echo "         sudo systemctl enable --now $SERVICE_NAME"
    echo ""
    echo "  Or re-run with the API key to skip the manual edit:"
    echo "    ADSB_API_KEY=xxx ADSB_NAME=my-pi sudo -E bash $0"
fi
echo ""
