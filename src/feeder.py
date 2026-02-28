"""Feeder agent — lightweight frame forwarder for remote receivers.

Runs on any machine with an RTL-SDR dongle. Captures frames from rtl_adsb
and POSTs them to a central adsb-decode server in batches.

Usage:
    python -m src.feeder --server https://adsb.example.com --name roof-antenna --key abc123

Architecture:
    [Pi + Dongle] → feeder.py → HTTP POST /api/v1/frames → [Central Server]

    Feeder buffers frames and sends them in batches every few seconds.
    Heartbeat is sent every 30s with receiver status.
"""

from __future__ import annotations

import json
import subprocess
import sys
import time
from collections import deque
from threading import Thread

try:
    import requests
except ImportError:
    requests = None  # type: ignore[assignment]


class Feeder:
    """Captures ADS-B frames and forwards to a central server."""

    def __init__(
        self,
        server_url: str,
        receiver_name: str,
        api_key: str = "",
        lat: float | None = None,
        lon: float | None = None,
        batch_interval: float = 2.0,
    ):
        self.server_url = server_url.rstrip("/")
        self.receiver_name = receiver_name
        self.api_key = api_key
        self.lat = lat
        self.lon = lon
        self.batch_interval = batch_interval

        self.buffer: deque[str] = deque(maxlen=1000)
        self.frames_sent = 0
        self.frames_captured = 0
        self.running = False
        self.start_time = time.time()

    def _headers(self) -> dict:
        h = {"Content-Type": "application/json"}
        if self.api_key:
            h["Authorization"] = f"Bearer {self.api_key}"
        return h

    def _send_batch(self):
        """Send buffered frames to server."""
        if not self.buffer:
            return

        frames = []
        while self.buffer:
            frames.append(self.buffer.popleft())

        payload = {
            "receiver": self.receiver_name,
            "lat": self.lat,
            "lon": self.lon,
            "frames": frames,
            "timestamp": time.time(),
        }

        try:
            resp = requests.post(
                f"{self.server_url}/api/v1/frames",
                json=payload,
                headers=self._headers(),
                timeout=5,
            )
            if resp.status_code == 200:
                self.frames_sent += len(frames)
            else:
                print(f"Server error {resp.status_code}: {resp.text[:100]}", file=sys.stderr)
        except requests.RequestException as e:
            print(f"Send failed: {e}", file=sys.stderr)
            # Re-buffer unsent frames (front of queue)
            for f in reversed(frames):
                self.buffer.appendleft(f)

    def _send_heartbeat(self):
        """Send receiver status heartbeat."""
        payload = {
            "receiver": self.receiver_name,
            "lat": self.lat,
            "lon": self.lon,
            "frames_captured": self.frames_captured,
            "frames_sent": self.frames_sent,
            "uptime_sec": time.time() - self.start_time,
        }
        try:
            requests.post(
                f"{self.server_url}/api/v1/heartbeat",
                json=payload,
                headers=self._headers(),
                timeout=5,
            )
        except requests.RequestException:
            pass

    def _batch_sender(self):
        """Background thread that sends batches and heartbeats."""
        last_heartbeat = 0.0
        while self.running:
            self._send_batch()
            now = time.time()
            if now - last_heartbeat > 30:
                self._send_heartbeat()
                last_heartbeat = now
            time.sleep(self.batch_interval)
        # Final flush
        self._send_batch()

    def capture_and_forward(self):
        """Start capturing from rtl_adsb and forwarding to server."""
        if requests is None:
            print("Error: 'requests' package required. Install with: pip install requests", file=sys.stderr)
            sys.exit(1)
        self.running = True
        self.start_time = time.time()

        sender = Thread(target=self._batch_sender, daemon=True)
        sender.start()

        print(f"Feeder '{self.receiver_name}' → {self.server_url}")
        print("Capturing from rtl_adsb...")

        try:
            proc = subprocess.Popen(
                ["rtl_adsb"],
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
            )
            for line in proc.stdout:
                line = line.strip()
                if line.startswith("*") and line.endswith(";"):
                    hex_str = line[1:-1]
                    self.buffer.append(hex_str)
                    self.frames_captured += 1
        except KeyboardInterrupt:
            print("\nStopping feeder...")
        except FileNotFoundError:
            print("Error: rtl_adsb not found. Install rtl-sdr tools first.", file=sys.stderr)
            sys.exit(1)
        finally:
            self.running = False
            sender.join(timeout=5)
            print(f"Captured: {self.frames_captured}, Sent: {self.frames_sent}")


def main():
    """CLI entry point for feeder."""
    import argparse

    parser = argparse.ArgumentParser(description="ADS-B feeder agent")
    parser.add_argument("--server", required=True, help="Central server URL")
    parser.add_argument("--name", required=True, help="Receiver name")
    parser.add_argument("--key", default="", help="API key for authentication")
    parser.add_argument("--lat", type=float, default=None, help="Receiver latitude")
    parser.add_argument("--lon", type=float, default=None, help="Receiver longitude")
    parser.add_argument("--interval", type=float, default=2.0, help="Batch send interval (seconds)")
    args = parser.parse_args()

    feeder = Feeder(
        server_url=args.server,
        receiver_name=args.name,
        api_key=args.key,
        lat=args.lat,
        lon=args.lon,
        batch_interval=args.interval,
    )
    feeder.capture_and_forward()


if __name__ == "__main__":
    main()
