# AIS Ingester — Runbook

How to run the maritime feed locally on Mac, verify it works, and deploy it alongside the live ADS-B service when ready.

The ingester is a separate binary (`ais-ingester`) inside the `adsb-server` crate. It connects to AISStream.io's WebSocket feed, parses position + ship-static messages, and writes them into the same TimescaleDB instance the ADS-B server uses. The web dashboard reads from the shared `vessel_positions` + `vessels` tables via the existing `/api/vessels*` routes — no protocol coupling between ingester and server beyond the database.

*Last updated: 2026-04-26.*

---

## Step 1 — Get an AISStream API key (one-time, free)

1. Open https://aisstream.io/apikeys
2. Log in with GitHub
3. Click "Create API Key"
4. Copy the key. Save it somewhere private — it doesn't display again.

The free tier has no documented rate limit beyond:
- 1 subscription update per second
- ~300 messages/sec if you subscribe globally (we'll use a bounding box to cut this)
- AISStream is **beta**, no SLA. Treat it as best-effort, not safety-critical.

## Step 2 — Run locally on Mac

```bash
cd ~/Developer/projects/adsb-decode/rust

# Build (one-time, ~20s)
cargo build --bin ais-ingester --features timescaledb

# Run it pointed at a local Postgres (or the production one if you want)
AISSTREAM_API_KEY="<your key>" \
DATABASE_URL="postgresql://localhost/adsb" \
AIS_BOUNDING_BOX="[[[24,-82],[45,-65]]]" \
./target/debug/ais-ingester
```

`AIS_BOUNDING_BOX` defaults to global `[[[-90,-180],[90,180]]]` — that's the whole world (~300 msg/sec). The example above (`[[[24,-82],[45,-65]]]`) covers the US East Coast from Florida to Maine + a slice of the Atlantic, which gives you a much smaller and more relevant stream (~5-50 msg/sec depending on traffic).

What you should see within ~10 seconds:

```
ais-ingester starting
  bounding box: [[[24,-82],[45,-65]]]
  reconnect: 5000 ms
  log interval: 60 s
  connected to database
subscribed to AISStream
[60s] pos+1234 stat+45 | total pos=1234 stat=45 dropped=0 reconnects=0
[120s] pos+1287 stat+38 | total pos=2521 stat=83 dropped=0 reconnects=0
```

The first stats line tells you whether the feed is alive: `pos+N` should be a positive number that scales with how busy your bounding box is.

If you see `pos+0` indefinitely, either the bounding box is empty of traffic or the API key is wrong. The subscription doesn't error visibly — AISStream just closes the connection silently after a few seconds, which the ingester logs as `websocket loop error: ... ; reconnecting`.

## Step 3 — Verify ships are in the database

In another terminal:

```bash
psql adsb -c "SELECT count(DISTINCT mmsi), count(*) FROM vessel_positions WHERE time > NOW() - INTERVAL '5 minutes';"
psql adsb -c "SELECT mmsi, name, vessel_type FROM vessels ORDER BY last_seen DESC LIMIT 10;"
```

You should see vessel positions accumulating and a handful of ship metadata rows.

## Step 4 — Verify via the existing web API

The endpoints are already wired up in `adsb-server`:

```bash
curl -s "http://localhost:8000/api/vessel_positions_latest?limit=20" | jq '.[] | {mmsi, lat, lon}' | head -20
curl -s "http://localhost:8000/api/vessels?limit=20" | jq '.[] | {mmsi, name, vessel_type}'
```

If those return data, the production-side wiring works.

## Step 5 — Production deploy (when ready)

The ingester is a single Rust binary. Deploy approach:

1. Copy `target/release/ais-ingester` to the VPS (cross-compile with `cargo build --release --target x86_64-unknown-linux-gnu --bin ais-ingester --features timescaledb` from a Linux box, or build on the VPS itself).
2. Drop a systemd unit at `/etc/systemd/system/ais-ingester.service`:

```ini
[Unit]
Description=AIS Ingester (AISStream.io → TimescaleDB)
After=network-online.target postgresql.service
Wants=network-online.target

[Service]
Type=simple
User=adsb
ExecStart=/opt/adsb-decode/ais-ingester
Restart=always
RestartSec=10
Environment="AISSTREAM_API_KEY=YOUR_KEY"
Environment="DATABASE_URL=postgresql://adsb:PASSWORD@localhost/adsb"
Environment="AIS_BOUNDING_BOX=[[[24,-82],[45,-65]]]"
Environment="AIS_LOG_INTERVAL_S=300"

[Install]
WantedBy=multi-user.target
```

3. `sudo systemctl daemon-reload && sudo systemctl enable --now ais-ingester`
4. Watch the log: `sudo journalctl -u ais-ingester -f`

Memory footprint is small (single connection, no large buffers) — should fit comfortably in our existing Lightsail headroom.

## Step 6 — Add the dashboard toggle (separate, after ingester is proven)

The `/api/vessel_positions_latest` endpoint is already there. The remaining UI work is:

1. Add a "Show vessels" toggle to the map header (mirror the existing layer toggles).
2. When toggled on, fetch `/api/vessel_positions_latest?limit=500` every 10 seconds.
3. Render each position as a ship-shaped marker on the existing Leaflet map (rotate the marker by `course_deg` if available).
4. On marker click, show MMSI + ship name + type + speed/course in a popup, similar to the existing aircraft popup.

~100 lines of HTML/JS. Should land in a separate commit after the ingester is verified live.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| "AISSTREAM_API_KEY not set" on startup | Env var unset | Export it; check `echo $AISSTREAM_API_KEY` |
| Subscribes but no `pos+N` after 60s | Empty bounding box, or API key invalid | Verify key at https://aisstream.io/apikeys; widen the bounding box |
| `websocket loop error: WebSocket protocol error` | AISStream closed the connection (hot day or invalid subscription) | Reconnect is automatic. If it loops indefinitely, check the subscription JSON |
| `dropped` counter rising | DB write failures | Check Postgres logs; usually means schema mismatch |
| Postgres connection refused | DB URL wrong or Postgres not listening | `psql adsb -c 'SELECT 1'` to confirm |
| Lots of `Other` messages skipped | Normal — AISStream relays 24 message types, we only parse 4 | No action; those are non-positional broadcasts |

## Related

- `rust/adsb-server/src/ais.rs` — message parser (11 tests)
- `rust/adsb-server/src/bin/ais-ingester.rs` — the binary
- `rust/adsb-server/src/db_pg.rs` — `add_vessel_position` + `upsert_vessel`
- `rust/adsb-server/src/web/routes.rs` — `/api/vessels`, `/api/vessel_positions*` (already in place)
- `rust/adsb-server/src/demo.rs` — the existing fake-vessel generator (still fine for offline testing without an API key)
