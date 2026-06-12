# Client-error → server logging

Uncaught browser errors on the live adsb-decode UI (map / 3D Cesium globe /
replay / NLP query) are POSTed to the server so the full stack is readable in
the systemd journal — no screenshot or user report needed.

## How it flows

```
browser error (map / 3D globe / replay)
   │  window 'error' / 'unhandledrejection'
   ▼
templates/error-report.js   ── structured JSON payload, dedupes/throttles, fails silently
   │  POST (sendBeacon / fetch keepalive)
   ▼
POST /api/clientlog  (rust/adsb-server src/web/routes.rs)
   │  validate, 32 KB cap, field whitelist, per-IP + global rate limit
   │  println!('[clientlog] {…}')  → stdout
   ▼
systemd journal (adsb-decode unit)  ── greppable, structured
```

- The reporter is served via `include_str!` like `map.js`
  (`GET /assets/error-report.js`) and injected first in `<head>` by
  `render_page_with_meta`, so it loads on **every** page before the app.
- The Cesium 3D globe is the most likely WebGL/shader failure surface. The
  payload includes a sniffed `shaderType` (vertex/fragment/both/unknown). To
  capture Cesium's own `scene.renderError` (which carries the GLSL compile log),
  wire one line in `map.js`:
  `viewer.scene.renderError.addEventListener(e => window.__reportError({ source: 'cesium-render', error: e }));`
  (the global `window.__reportError` is exposed by the reporter).

## Payload shape

```json
{
  "ts": 1234567890,
  "source": "window-error",        // window-error | unhandledrejection | cesium-render
  "url": "https://adsb.blueoctopustechnology.com/",
  "message": "TypeError: …",
  "name": "TypeError",
  "stack": "…",
  "shaderType": "fragment",        // vertex | fragment | both | unknown (sniffed)
  "ua": "Mozilla/5.0 …",
  "sessionId": "…"                 // random, non-PII, per page load
}
```

## Read the logs (the exact commands)

adsb-decode runs as the `adsb-decode` systemd unit on Lightsail (stdout →
journal). SSH to the box, then:

```bash
# Tail live (leave running while reproducing):
journalctl -u adsb-decode -f | grep '\[clientlog\]'

# Last hour:
journalctl -u adsb-decode --since '1 hour ago' | grep '\[clientlog\]'

# Pretty-print the JSON payloads:
journalctl -u adsb-decode --since '1 hour ago' \
  | grep -o '\[clientlog\] .*' | sed 's/^\[clientlog\] //' | jq .
```

## Safety notes

- **Fails silently** — every reporter entry point is try/catch-wrapped.
- **Dedupe + throttle** — first occurrence per signature, repeats suppressed for
  60 s, hard-cap 50 POSTs per page load.
- **Server caps** — 32 KB body cap (413, plus the global 512 KB body limit),
  per-IP rate limit 30/min + global 600/min (429, spoofed XFF can't bypass the
  global cap), field whitelist + length caps, POST-only, no storage/forwarding
  (the journal is the only record).
- **No PII** — UA + a random per-page sessionId only. No accounts. Client IP is
  only in the journal's own metadata, never in the payload.

## Tests

`rust/adsb-server/src/web/routes.rs` covers: valid payload → 204, bad JSON →
400, and the reporter asset is served as JavaScript and points at
`/api/clientlog`. `node -c templates/error-report.js` validates the JS parses.
