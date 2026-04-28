#!/bin/bash
# adsb-decode-healthcheck.sh — poke the API; if it hangs or 5xx's, restart the service.
# Modeled on bluepages-healthcheck.sh (BluePages, sister project).
# Triggered by adsb-decode-healthcheck.timer every 60 seconds.

set -u
URL='http://127.0.0.1:8000/api/stats'
TIMEOUT=10
LOGFILE=/var/log/adsb-decode-healthcheck.log

code=$(curl -s -o /dev/null -w '%{http_code}' --max-time $TIMEOUT "$URL" || echo 000)
ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)

if [[ "$code" == '200' ]]; then
    exit 0
fi

echo "$ts unhealthy (HTTP $code) — restarting adsb-decode.service" | tee -a "$LOGFILE"
/bin/systemctl restart adsb-decode.service
sleep 5
recheck=$(curl -s -o /dev/null -w '%{http_code}' --max-time $TIMEOUT "$URL" || echo 000)
echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) post-restart (HTTP $recheck)" | tee -a "$LOGFILE"
exit 1
