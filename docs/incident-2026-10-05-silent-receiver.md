# Incident: silent receiver, 2026-10-05 → 2026-10-29

The Splatlas map (splatlas.live) showed no aircraft for 23 days. The hub at
adsb.blueoctopustechnology.com reported the `blue-octopus` receiver online the
whole time. Nothing was down; the dongle had simply stopped producing frames
and no component treated that as a fault.

## Timeline (EDT)

| When | What |
|------|------|
| 2026-10-05 22:54 | Dongle (AirNav ADSB_1090, RTL2838) drops off USB on the Pi 5. Kernel: "Cannot enable. Maybe the USB cable is bad?", power-cycles the port. `rtl_adsb` exits; the agent exits (crash-only); systemd restarts it. |
| 2026-10-05 22:56 | Second drop, same sequence. Dongle re-enumerates at **full speed** ("not running at top speed") instead of high speed. |
| 2026-10-05 22:56 | Third agent instance starts. `rtl_adsb` opens the degraded device without error and runs. It produces **zero frames** from here on. |
| 2026-10-06 → 10-29 | Heartbeats every 30 s with `frames_captured: 0`. `/api/v1/receivers` says `online: true`. `/api/stats` says `positions: 0`, `feed_age_seconds: null`. Map empty. |
| 2026-10-17 | Another USB drop and re-enumeration. Nobody notices: the receiver already looked exactly like this. |
| 2026-10-29 | Manual `systemctl restart adsb-receiver`. Fresh `rtl_adsb` reopens the device: 7,750 frames in the first 12 minutes. |

Power was clean (`vcgencmd get_throttled` → `0x0`). The hardware suspect is
the USB cable or port; a full-speed re-enumeration is the classic signature.

## Why it hid

Three layers each had a blind spot, and they lined up.

1. **The agent only exits when `rtl_adsb` exits.** Crash-only is the right
   design for a dongle that disappears, and it worked twice that night. But a
   dongle that re-enumerates in a bad state *stays open*. `rtl_adsb` sat in
   its read loop forever with nothing to demodulate, so the agent never had a
   reason to die, and systemd's `Restart=always` never had a reason to fire.
2. **The hub equated heartbeats with health.** `online` was
   "heartbeat within 120 s". The agent kept heartbeating because the agent was
   fine. `frames_captured: 0` was on the wire the whole time and nothing read
   it.
3. **"No feed" and "no traffic" looked identical.** `feed_age_seconds` is the
   age of the newest *position*. Null means "no positions" — which is also
   what a quiet night or a CPR pairing gap looks like. There was no field
   that said "no receiver has delivered a *frame* in 23 days".

## What changed

### Receiver: frame-stall watchdog (`rust/adsb-receiver/src/watchdog.rs`)

The agent now watches its own `frames_captured` counter. If it has not
advanced for the stall window (`--stall-timeout` / `ADSB_STALL_TIMEOUT`,
default 600 s), the agent logs:

```
[watchdog] no frame captured for 600 s (frames_captured stuck at 0) — rtl_adsb is running but silent, usually a dongle that re-enumerated in a bad state; exiting so systemd restarts the agent and rtl_adsb reopens the device
```

kills `rtl_adsb`, and exits with status 2. systemd restarts it, and the fresh
`rtl_adsb` re-initialises the device — the same thing the manual restart did
on 2026-10-29, ten minutes after the fault instead of 23 days.

The crash-only design is unchanged; this is a second trigger for it. No new
dependencies, still one binary. The clock starts at process start, so an
agent that opens an already-dead dongle also exits after one window.

**Why 10 minutes is safe at night.** `rtl_adsb` emits every Mode S frame it
demodulates: DF11 all-call replies, DF4/DF5 surveillance replies, DF17
extended squitter. Any transponder within the radio horizon counts, and at
FL350 that horizon is roughly 230 nm. The ridgeline receiver normally sees
several frames per second; the quietest hour of the night still carries
transcontinental overflights. Ten consecutive minutes with not one frame is
a silent device, not a quiet sky. If the dongle is permanently dead the agent
restarts every ~10 minutes, which is cheap and is also the recovery attempt.

Had the watchdog been in place, the 2026-10-05 22:56 instance would have
exited at about 23:06 and the restart would have reopened the device. The
2026-10-17 drop would have followed the same path.

### Hub: three-state receiver health (`rust/adsb-server/src/web/ingest.rs`)

The hub keeps `last_frames_at` per receiver: the last time it had evidence
frames were flowing (a non-empty `/api/v1/frames` batch, or a heartbeat whose
`frames_captured` rose). A drop in the counter is an agent restart, not a
frame.

`/api/v1/receivers` now reports:

| Field | Meaning |
|-------|---------|
| `status` | `"online"`, `"degraded"` or `"offline"` |
| `online` | `true` only for `online` — a degraded receiver is **not** healthy |
| `heartbeat_ok` | the old liveness signal (heartbeat within 120 s) |
| `degraded` | heartbeats arriving, no frame evidence for 600 s |
| `last_frames_at`, `frames_age_seconds` | when frames last flowed |

The receivers page shows DEGRADED in amber with "no frames for 23d" and the
map draws the receiver amber in 2D and 3D. During this incident every one of
those would have read degraded from 2026-10-05 23:06 onward.

### Hub: newest frame on `/api/stats`

`newest_frame_at`, `frame_age_seconds` and `receivers_degraded` sit beside
`feed_age_seconds`. Frames arriving but no positions is a quiet sky; no
frames at all is a dead receiver. These are in-memory, so after a hub
restart they are null until the first batch lands.

## What is not fixed

- The USB cable/port. The watchdog recovers from a bad re-enumeration; it
  does not stop the next one. Replace the cable, and consider a powered hub.
- Alerting. The hub now *says* degraded; nothing yet pages anyone. A monitor
  that reads `receivers_degraded` or `frame_age_seconds` from `/api/stats`
  is the obvious next step.
