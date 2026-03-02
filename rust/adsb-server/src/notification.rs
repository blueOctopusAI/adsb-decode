//! Webhook notification dispatch for filter events.
//!
//! Fire-and-forget HTTP POST of filter events as JSON.

use adsb_core::filter::FilterEvent;
use adsb_core::types::icao_to_string;

/// Dispatches filter events to a webhook URL via HTTP POST.
#[derive(Clone)]
pub struct WebhookDispatcher {
    url: String,
    client: reqwest::Client,
}

impl WebhookDispatcher {
    pub fn new(url: &str) -> Self {
        WebhookDispatcher {
            url: url.to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Fire-and-forget POST of a filter event as JSON.
    pub fn notify(&self, event: &FilterEvent) {
        let payload = serde_json::json!({
            "icao": icao_to_string(&event.icao),
            "event_type": event.event_type,
            "description": event.description,
            "lat": event.lat,
            "lon": event.lon,
            "altitude_ft": event.altitude_ft,
            "timestamp": event.timestamp,
        });

        let client = self.client.clone();
        let url = self.url.clone();

        tokio::spawn(async move {
            if let Err(e) = client.post(&url).json(&payload).send().await {
                eprintln!("  [webhook] POST failed: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_dispatcher_creation() {
        let wh = WebhookDispatcher::new("https://example.com/hook");
        assert_eq!(wh.url, "https://example.com/hook");
    }

    #[test]
    fn test_filter_event_serialization() {
        let event = FilterEvent {
            icao: [0xAD, 0xF7, 0xC8],
            event_type: "military_detected",
            description: "Military aircraft detected: REACH42".to_string(),
            lat: Some(35.5),
            lon: Some(-82.5),
            altitude_ft: Some(25000),
            timestamp: 1700000000.0,
        };

        let payload = serde_json::json!({
            "icao": icao_to_string(&event.icao),
            "event_type": event.event_type,
            "description": event.description,
            "lat": event.lat,
            "lon": event.lon,
            "altitude_ft": event.altitude_ft,
            "timestamp": event.timestamp,
        });

        assert_eq!(payload["icao"], "ADF7C8");
        assert_eq!(payload["event_type"], "military_detected");
        assert!(payload["lat"].as_f64().is_some());
    }
}
