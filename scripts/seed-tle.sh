#!/usr/bin/env bash
# Fetch fresh TLEs and push them to the adsb-decode server's cache.
#
# Sources (tried in order):
#   1. CelesTrak (canonical, full constellation TLEs; rate-limits to 1
#      success per dataset per IP per 2 hours — the "GP data has not
#      updated since your last successful query" message means you need
#      to wait the rest of the 2-hour window before this script will
#      get a real response)
#   2. satnogs.org (last-ditch fallback for ISS / GPS / amateur sats —
#      satnogs only curates a small subset, so Starlink fallback will
#      only return a handful of sats. CelesTrak is the canonical source.)
#
# Run from a residential IP (cloud-provider IPs get 403'd outright).
# Suggested cron: weekly. The server stores results to disk and serves
# stale-with-warning if the next refresh fails, so weekly is fine.
#
# Usage:
#   bash scripts/seed-tle.sh                       # default groups
#   bash scripts/seed-tle.sh starlink              # one group
#   bash scripts/seed-tle.sh starlink gps-ops      # specific groups
#
# Env:
#   ADSB_HOST       = https://adsb.blueoctopustechnology.com  (default)
#   ADSB_TOKEN      = bearer token if server has auth_token set (optional)
#   TLE_ARCHIVE_DIR = dated TLE history root (default ~/tle-archive). Point at
#                     a NAS mount for the durable home; falls back to the
#                     default if the mount is absent (never writes into a
#                     dead /Volumes mount point).

set -euo pipefail

ADSB_HOST="${ADSB_HOST:-https://adsb.blueoctopustechnology.com}"
ADSB_TOKEN="${ADSB_TOKEN:-}"

if [ "$#" -eq 0 ]; then
  set -- starlink gps-ops stations
fi

UA="Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"

# Map group → satnogs name-substring used for filtering (satnogs returns
# all satellites; we filter client-side).
satnogs_substr() {
  case "$1" in
    starlink) echo "STARLINK" ;;
    gps-ops)  echo "GPS BIIR\\|GPS BIIF\\|GPS III\\|NAVSTAR" ;;
    stations) echo "ISS\\|TIANGONG\\|CSS" ;;
    *) echo "" ;;
  esac
}

fetch_from_celestrak() {
  local group="$1"
  local out="$2"
  local status
  status=$(curl -sS --max-time 30 -A "$UA" \
    -o "$out" -w '%{http_code}' \
    "https://celestrak.org/NORAD/elements/gp.php?GROUP=${group}&FORMAT=tle")
  local bytes; bytes=$(wc -c < "$out" | tr -d ' ')
  if [ "$status" != "200" ] || [ "$bytes" -lt 500 ]; then
    return 1
  fi
  echo "$bytes"
}

# Fetch from satnogs as JSON, filter to the desired family, write as 3-line
# TLE text. satnogs paginates so we may need multiple calls; the API
# defaults seem fine for our needs (~10k total catalog entries).
fetch_from_satnogs() {
  local group="$1"
  local out="$2"
  local substr; substr=$(satnogs_substr "$group")
  if [ -z "$substr" ]; then
    return 1
  fi
  local json; json=$(mktemp)
  trap 'rm -f "$json"' RETURN
  # satnogs serves the whole TLE table; we filter client-side. Limit is
  # currently uncapped on this endpoint but we cap retries.
  if ! curl -sS --max-time 90 -A "$UA" \
    -o "$json" \
    "https://db.satnogs.org/api/tle/?format=json"; then
    return 1
  fi
  python3 - "$json" "$substr" > "$out" <<'PY'
import json, re, sys
path, pattern = sys.argv[1], sys.argv[2]
rx = re.compile(pattern)
with open(path) as f:
    data = json.load(f)
for row in data:
    name = (row.get("tle0") or "").strip()
    if not name or not rx.search(name):
        continue
    l1 = row.get("tle1") or ""
    l2 = row.get("tle2") or ""
    if not l1.startswith("1 ") or not l2.startswith("2 "):
        continue
    print(name)
    print(l1)
    print(l2)
PY
  local bytes; bytes=$(wc -c < "$out" | tr -d ' ')
  if [ "$bytes" -lt 500 ]; then
    return 1
  fi
  echo "$bytes"
}

# Phase-0 A3: dated TLE-history archive. The server cache is last-good-only
# (tle_cache.rs overwrites <group>.tle), so without this every weekly pull
# destroys the previous epoch — irreplaceable orbital history. Archive every
# successful fetch to <root>/YYYY-MM-DD/<group>.tle; history accrues, nothing
# is overwritten within a day's snapshot. ~MB/day. Spec: phase-0-execution-spec §A3.
archive_tle() {
  local group="$1" src="$2"
  local root="${TLE_ARCHIVE_DIR:-$HOME/tle-archive}"
  case "$root" in
    /Volumes/*)
      if [ ! -d "$(dirname "$root")" ]; then
        echo "  ⚠ archive mount $(dirname "$root") absent — archiving to ~/tle-archive instead"
        root="$HOME/tle-archive"
      fi ;;
  esac
  local day; day=$(date -u +%F)
  mkdir -p "$root/$day"
  cp "$src" "$root/$day/${group}.tle"
  echo "  ↳ archived ${group} → $root/$day/${group}.tle"
}

for group in "$@"; do
  echo "→ ${group}: fetching…"
  tmp=$(mktemp)
  bytes=""
  source_label=""
  # Try CelesTrak first
  if bytes=$(fetch_from_celestrak "$group" "$tmp"); then
    source_label="CelesTrak"
  elif bytes=$(fetch_from_satnogs "$group" "$tmp"); then
    source_label="satnogs"
  else
    echo "  ✗ ${group}: both sources failed (CelesTrak likely rate-limited; satnogs unfiltered or unreachable)"
    head -c 200 "$tmp" 2>/dev/null | sed 's/^/    /'
    echo
    rm -f "$tmp"
    continue
  fi
  archive_tle "$group" "$tmp"
  echo "  ${group}: got ${bytes} bytes from ${source_label}, pushing to ${ADSB_HOST}"
  if [ -n "$ADSB_TOKEN" ]; then
    resp=$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' \
      -X POST -H "Authorization: Bearer ${ADSB_TOKEN}" \
      -H 'Content-Type: text/plain' --data-binary "@${tmp}" \
      "${ADSB_HOST}/api/v1/tle/${group}")
  else
    resp=$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' \
      -X POST -H 'Content-Type: text/plain' --data-binary "@${tmp}" \
      "${ADSB_HOST}/api/v1/tle/${group}")
  fi
  rm -f "$tmp"
  if [ "$resp" = "204" ]; then
    echo "  ✓ ${group} seeded (${bytes} bytes via ${source_label})"
  else
    echo "  ✗ ${group}: server returned HTTP ${resp}"
  fi
done
