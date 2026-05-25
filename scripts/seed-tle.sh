#!/usr/bin/env bash
# Fetch fresh TLEs from CelesTrak (residential IP works; VPS IP gets 403'd)
# and push them to the adsb-decode server's cache.
#
# Usage:
#   bash scripts/seed-tle.sh                       # all default groups
#   bash scripts/seed-tle.sh starlink              # one group
#   ADSB_HOST=https://adsb.blueoctopustechnology.com \
#   ADSB_TOKEN=... bash scripts/seed-tle.sh        # custom host + bearer
#
# Run on cron from a residential IP (e.g. weekly) to keep the cache fresh.

set -euo pipefail

ADSB_HOST="${ADSB_HOST:-https://adsb.blueoctopustechnology.com}"
ADSB_TOKEN="${ADSB_TOKEN:-}"
GROUPS=("${@:-starlink gps-ops stations}")

# Expand single string arg into array
if [ "${#GROUPS[@]}" -eq 1 ] && [[ "${GROUPS[0]}" == *" "* ]]; then
  read -ra GROUPS <<< "${GROUPS[0]}"
fi

UA="Mozilla/5.0 (compatible; adsb-decode-seed/1.0)"

for group in "${GROUPS[@]}"; do
  echo "→ ${group}: fetching from CelesTrak…"
  tle=$(curl -sS --max-time 20 -A "$UA" \
    "https://celestrak.org/NORAD/elements/gp.php?GROUP=${group}&FORMAT=tle")
  bytes=${#tle}
  if [ "$bytes" -lt 500 ]; then
    echo "  ✗ ${group}: response too short (${bytes} bytes), skipping"
    continue
  fi
  echo "  ${group}: got ${bytes} bytes, pushing to ${ADSB_HOST}"
  auth_args=()
  if [ -n "$ADSB_TOKEN" ]; then
    auth_args=(-H "Authorization: Bearer ${ADSB_TOKEN}")
  fi
  status=$(curl -sS --max-time 20 -o /dev/null -w '%{http_code}' \
    -X POST "${auth_args[@]}" \
    -H 'Content-Type: text/plain' \
    --data-binary "$tle" \
    "${ADSB_HOST}/api/v1/tle/${group}")
  if [ "$status" = "204" ]; then
    echo "  ✓ ${group} seeded (${bytes} bytes)"
  else
    echo "  ✗ ${group}: server returned HTTP ${status}"
  fi
done
