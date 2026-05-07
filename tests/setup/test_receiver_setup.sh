#!/usr/bin/env bash
# Smoke tests for deploy/receiver-setup.sh.
#
# Runs the script with --dry-run and overridden file paths so the test
# never touches /opt, /etc, or systemd. Asserts on the expected behavior:
# argument parsing, env-file rendering for both API-key-set and template
# paths, service file path correctness, and auto-start gating.
#
# Run from repo root:   bash tests/setup/test_receiver_setup.sh

set -euo pipefail

SCRIPT="$(cd "$(dirname "$0")/../.." && pwd)/deploy/receiver-setup.sh"
[ -f "$SCRIPT" ] || { echo "FATAL: cannot find $SCRIPT" >&2; exit 1; }

PASS=0
FAIL=0
FAILED_TESTS=()

assert_contains() {
    local haystack="$1" needle="$2" name="$3"
    if grep -qF -- "$needle" <<<"$haystack"; then
        PASS=$((PASS+1))
        echo "  ok  $name"
    else
        FAIL=$((FAIL+1))
        FAILED_TESTS+=("$name")
        echo "  FAIL $name"
        echo "    expected substring: $needle"
        echo "    in output:"
        sed 's/^/      /' <<<"$haystack"
    fi
}

assert_not_contains() {
    local haystack="$1" needle="$2" name="$3"
    if ! grep -qF -- "$needle" <<<"$haystack"; then
        PASS=$((PASS+1))
        echo "  ok  $name"
    else
        FAIL=$((FAIL+1))
        FAILED_TESTS+=("$name")
        echo "  FAIL $name"
        echo "    forbidden substring: $needle"
    fi
}

assert_eq() {
    local actual="$1" expected="$2" name="$3"
    if [ "$actual" = "$expected" ]; then
        PASS=$((PASS+1))
        echo "  ok  $name"
    else
        FAIL=$((FAIL+1))
        FAILED_TESTS+=("$name")
        echo "  FAIL $name"
        echo "    expected: $expected"
        echo "    actual:   $actual"
    fi
}

run_dryrun() {
    # shellcheck disable=SC2068
    env -i HOME="$HOME" PATH="$PATH" TARGET_OVERRIDE=aarch64-unknown-linux-gnu $@ bash "$SCRIPT" --dry-run 2>&1 || true
}

echo "=== bash syntax check ==="
bash -n "$SCRIPT" && echo "  ok  syntax"

echo "=== test: --help ==="
help_out=$(bash "$SCRIPT" --help 2>&1)
help_exit=$?
assert_eq "$help_exit" "0" "exit code 0 on --help"
assert_contains "$help_out" "ADSB_API_KEY" "help mentions ADSB_API_KEY"
assert_contains "$help_out" "--dry-run" "help mentions --dry-run"

echo "=== test: unknown arg fails ==="
unknown_out=$(bash "$SCRIPT" --bogus 2>&1 || true)
assert_contains "$unknown_out" "Unknown argument" "unknown arg produces error"

echo "=== test: --dry-run touches no system files ==="
out=$(run_dryrun)
assert_contains "$out" "[dry-run]" "dry-run output prefix appears"
assert_contains "$out" "would write" "dry-run announces file writes"
assert_not_contains "$out" "Permission denied" "no permission errors"

echo "=== test: dry-run with no API_KEY writes template env ==="
out=$(run_dryrun ADSB_NAME=test-pi)
assert_contains "$out" "ADSB_SERVER=https://adsb.blueoctopustechnology.com" "default server URL set"
assert_contains "$out" "ADSB_NAME=test-pi" "receiver name passed through"
assert_contains "$out" "# ADSB_API_KEY=your-api-key-here" "template comment present when no key"
assert_not_contains "$out" "Enabling and starting" "no auto-start without API key"
assert_contains "$out" "Get an API key" "next-steps message shown"

echo "=== test: dry-run with API_KEY writes populated env + auto-starts ==="
out=$(run_dryrun ADSB_API_KEY=secret-key-xyz ADSB_NAME=production-pi ADSB_LAT=35.18 ADSB_LON=-83.38)
assert_contains "$out" "ADSB_API_KEY=secret-key-xyz" "API key written into env file"
assert_contains "$out" "ADSB_NAME=production-pi" "receiver name written"
assert_contains "$out" "ADSB_LAT=35.18" "latitude written"
assert_contains "$out" "ADSB_LON=-83.38" "longitude written"
assert_not_contains "$out" "ADSB_API_KEY=your-api-key-here" "no template placeholder when key set"
assert_contains "$out" "Enabling and starting" "auto-start triggered when API_KEY set"
assert_contains "$out" "systemctl enable --now adsb-receiver" "systemctl enable command issued"

echo "=== test: dry-run with ADSB_AUTOSTART=1 forces enable ==="
out=$(run_dryrun ADSB_AUTOSTART=1)
assert_contains "$out" "Enabling and starting" "auto-start triggered by ADSB_AUTOSTART"

echo "=== test: optional vars are omitted when unset ==="
out=$(run_dryrun ADSB_API_KEY=k1)
assert_not_contains "$out" "ADSB_LAT=" "ADSB_LAT omitted when unset"
assert_not_contains "$out" "ADSB_GAIN=" "ADSB_GAIN omitted when unset"

echo "=== test: optional vars are emitted when set ==="
out=$(run_dryrun ADSB_API_KEY=k1 ADSB_GAIN=400 ADSB_PPM=2 ADSB_DEVICE=1)
assert_contains "$out" "ADSB_GAIN=400" "ADSB_GAIN written"
assert_contains "$out" "ADSB_PPM=2" "ADSB_PPM written"
assert_contains "$out" "ADSB_DEVICE=1" "ADSB_DEVICE written"

echo "=== test: file paths honor INSTALL_DIR / ENV_FILE / SERVICE_FILE overrides ==="
out=$(run_dryrun INSTALL_DIR=/tmp/adsb-r ENV_FILE=/tmp/adsb.env SERVICE_FILE=/tmp/adsb.service ADSB_API_KEY=k1)
assert_contains "$out" "/tmp/adsb-r" "INSTALL_DIR override honored"
assert_contains "$out" "/tmp/adsb.env" "ENV_FILE override honored"
assert_contains "$out" "/tmp/adsb.service" "SERVICE_FILE override honored"
assert_contains "$out" "ExecStart=/tmp/adsb-r/adsb-receiver" "service unit ExecStart uses override"
assert_contains "$out" "EnvironmentFile=/tmp/adsb.env" "service unit EnvironmentFile uses override"

echo "=== test: non-root, non-dry-run, non-skip exits with error ==="
nonroot_out=$(env -i PATH="$PATH" SKIP_ROOT_CHECK=0 bash "$SCRIPT" 2>&1 || true)
# Only meaningful if we're actually not root; CI containers may run as root.
if [ "$EUID" -ne 0 ]; then
    assert_contains "$nonroot_out" "must be run as root" "non-root invocation refused"
else
    echo "  skip non-root test (running as root, EUID=$EUID)"
fi

echo ""
echo "=================================================="
echo "  $PASS passed, $FAIL failed"
echo "=================================================="

if [ "$FAIL" -gt 0 ]; then
    echo "Failed tests:"
    for t in "${FAILED_TESTS[@]}"; do
        echo "  - $t"
    done
    exit 1
fi
