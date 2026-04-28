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

### Step 2a — Dry-run first (no database needed)

This is the fastest way to verify the WebSocket + parsing actually works on real ship data. No Postgres, no schema setup. Just the API key:

```bash
cd ~/Developer/projects/adsb-decode/rust

# Build (one-time, ~20s)
cargo build --bin ais-ingester --features timescaledb

# Dry-run: skips DB, prints parsed messages to stdout
AISSTREAM_API_KEY="<your key>" \
AIS_DRY_RUN=1 \
AIS_BOUNDING_BOX="[[[24,-82],[45,-65]]]" \
./target/debug/ais-ingester
```

What you'll see within ~10 seconds:

```
ais-ingester starting (dry-run, no DB)
  bounding box: [[[24,-82],[45,-65]]]
  ...
subscribed to AISStream
POS  mmsi=367123456 lat=33.45123 lon=-79.12345 sog=Some(8.4) cog=Some(180.0) hdg=Some(178.0) name=Some("ATLANTIC PIONEER")
POS  mmsi=338901234 lat=32.78912 lon=-79.93456 sog=Some(0.0) cog=None hdg=None name=None
STAT mmsi=367123456 name=Some("ATLANTIC PIONEER") type=Some("Cargo")
...
[60s] pos+1234 stat+45 | total pos=1234 stat=45 dropped=0 reconnects=0
```

If `pos+N` is positive after one minute, the parser + WebSocket are healthy. Press Ctrl-C when satisfied; the next step writes to a real DB.

### Step 2b — Real run with database

```bash
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

The ingester is a single Rust binary. Deploy approach used on the live Lightsail VPS as of 2026-04-28:

### 5a — Build the binary

Easiest: build on the VPS itself (one-time Rust toolchain install, ~6-10 min build on a 4 GB tier):

```bash
# On the VPS
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal --no-modify-path
source $HOME/.cargo/env

git clone --depth 1 https://github.com/blueOctopusAI/adsb-decode.git ~/adsb-decode-src
cd ~/adsb-decode-src/rust
cargo build --release -p adsb-server --bin ais-ingester --features adsb-server/timescaledb
```

Alternative: cross-compile from Mac via `docker run --platform linux/amd64 rust:1.90 ...` — works if Docker Desktop is running. Native cross from macOS without Docker fails because the GCC linker for `x86_64-unknown-linux-gnu` isn't installed.

### 5b — Install the binary

```bash
sudo install -m 755 -o adsb -g adsb \
  ~/adsb-decode-src/rust/target/release/ais-ingester \
  /opt/adsb-decode/ais-ingester
```

### 5c — Set up secrets (separate file, mode 600)

The systemd unit pulls config from two `EnvironmentFile=` paths so the AISStream API key isn't mixed with the shared `adsb-decode` config (and isn't visible in the world-readable unit file).

`DATABASE_URL` already lives in `/opt/adsb-decode/.env`. The new file is `/opt/adsb-decode/ais.env`:

```bash
# On the VPS, paste this with your real key in place of YOUR_KEY_HERE
sudo tee /opt/adsb-decode/ais.env >/dev/null <<'EOF'
AISSTREAM_API_KEY=YOUR_KEY_HERE
AIS_BOUNDING_BOX=[[[24,-82],[45,-65]]]
AIS_LOG_INTERVAL_S=300
EOF
sudo chown adsb:adsb /opt/adsb-decode/ais.env
sudo chmod 600 /opt/adsb-decode/ais.env
```

The single-quoted heredoc (`<<'EOF'`) keeps the bracket characters in the bounding box literal.

### 5d — Install + start the systemd unit

The unit lives in `deploy/ais-ingester.service` in this repo:

```bash
sudo install -m 644 deploy/ais-ingester.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now ais-ingester.service
```

### 5e — Verify

```bash
sudo journalctl -u ais-ingester -f
```

Expected output within ~10 seconds:

```
ais-ingester starting
  bounding box: [[[24,-82],[45,-65]]]
  reconnect: 5000 ms
  log interval: 300 s
  connected to database
subscribed to AISStream
```

After the first log interval (default 300s), you'll see `[300s] pos+N stat+N | total pos=N stat=N dropped=0 reconnects=0`. On the US East Coast bounding box, expect ~4 ships/sec ingest, settling toward a few hundred unique vessels.

Verify via the public API:

```bash
curl -s "https://adsb.blueoctopustechnology.com/api/vessels?limit=5" | jq '.[] | {mmsi, name, vessel_type}'
```

Memory footprint is small (single WebSocket, no large buffers) — comfortable on the 4 GB Lightsail tier alongside the existing services.

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
