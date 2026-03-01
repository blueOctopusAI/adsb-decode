"""Tests for notification dispatch."""

import json
from http.server import HTTPServer, BaseHTTPRequestHandler
import threading
import time

from src.notifications import NotificationDispatcher, WebhookConfig


class _RecordingHandler(BaseHTTPRequestHandler):
    """HTTP handler that records POST payloads."""
    received = []

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length)
        _RecordingHandler.received.append(json.loads(body))
        self.send_response(200)
        self.end_headers()

    def log_message(self, *args):
        pass  # Suppress output


class TestNotificationDispatcher:
    def test_no_webhooks_noop(self):
        d = NotificationDispatcher()
        d.notify({"event_type": "test", "description": "noop"})

    def test_event_filter(self):
        wh = WebhookConfig(url="http://localhost:1", events=["military_detected"])
        d = NotificationDispatcher([wh])
        # Should not crash even though URL is invalid — filter excludes it
        d.notify({"event_type": "other_event", "description": "filtered out"})

    def test_webhook_delivery(self):
        _RecordingHandler.received = []
        server = HTTPServer(("127.0.0.1", 0), _RecordingHandler)
        port = server.server_address[1]
        thread = threading.Thread(target=server.handle_request, daemon=True)
        thread.start()

        wh = WebhookConfig(url=f"http://127.0.0.1:{port}/hook")
        d = NotificationDispatcher([wh])
        d.notify({"event_type": "test", "icao": "A00001", "description": "Test event"})

        thread.join(timeout=5)
        server.server_close()

        assert len(_RecordingHandler.received) == 1
        assert _RecordingHandler.received[0]["icao"] == "A00001"

    def test_event_type_match(self):
        _RecordingHandler.received = []
        server = HTTPServer(("127.0.0.1", 0), _RecordingHandler)
        port = server.server_address[1]
        thread = threading.Thread(target=server.handle_request, daemon=True)
        thread.start()

        wh = WebhookConfig(url=f"http://127.0.0.1:{port}/hook", events=["emergency_squawk"])
        d = NotificationDispatcher([wh])
        d.notify({"event_type": "emergency_squawk", "description": "Match"})

        thread.join(timeout=5)
        server.server_close()

        assert len(_RecordingHandler.received) == 1
