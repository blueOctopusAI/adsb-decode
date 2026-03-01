"""Notification dispatch — webhook delivery for geofence and event alerts.

Sends JSON POST to configured webhook URLs when events fire.
Used by the CLI tracker to push real-time alerts to Slack, Discord, IFTTT, etc.
"""

from __future__ import annotations

import json
import logging
import threading
from dataclasses import dataclass

logger = logging.getLogger(__name__)


@dataclass
class WebhookConfig:
    """A configured webhook endpoint."""
    url: str
    events: list[str] | None = None  # None = all events


class NotificationDispatcher:
    """Dispatches event notifications to configured webhooks.

    Sends are non-blocking (fire-and-forget in background threads)
    so they don't slow down the tracking loop.
    """

    def __init__(self, webhooks: list[WebhookConfig] | None = None):
        self.webhooks = webhooks or []

    def notify(self, event_data: dict) -> None:
        """Send event to all matching webhooks (non-blocking)."""
        if not self.webhooks:
            return

        event_type = event_data.get("event_type", "")
        for wh in self.webhooks:
            if wh.events and event_type not in wh.events:
                continue
            threading.Thread(
                target=self._send,
                args=(wh.url, event_data),
                daemon=True,
            ).start()

    def _send(self, url: str, payload: dict) -> None:
        """POST JSON payload to webhook URL."""
        import urllib.request

        body = json.dumps(payload).encode("utf-8")
        req = urllib.request.Request(
            url,
            data=body,
            headers={
                "Content-Type": "application/json",
                "User-Agent": "adsb-decode/1.0",
            },
        )
        try:
            with urllib.request.urlopen(req, timeout=10):
                pass
        except Exception as e:
            logger.warning("Webhook delivery failed to %s: %s", url, e)
