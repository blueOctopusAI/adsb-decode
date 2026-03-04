use reqwest::Client;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time;

use crate::stats::Stats;

const MAX_BUFFER: usize = 1000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// A single frame as expected by the server's FrameData struct.
/// Matches: { hex: String, timestamp?: f64, signal_level?: f64 }
#[derive(Debug, Clone, Serialize)]
pub struct FramePayload {
    pub hex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_level: Option<f64>,
}

/// Batch ingest payload matching the server's IngestRequest.
/// Matches: { receiver, lat?, lon?, frames, timestamp? }
#[derive(Debug, Serialize)]
struct IngestRequest {
    receiver: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    lat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lon: Option<f64>,
    frames: Vec<FramePayload>,
    timestamp: f64,
}

/// Heartbeat payload matching the server's HeartbeatRequest.
/// Matches: { receiver, lat?, lon?, frames_captured?, frames_sent?, uptime_sec? }
#[derive(Debug, Serialize)]
struct HeartbeatRequest {
    receiver: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    lat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lon: Option<f64>,
    frames_captured: u64,
    frames_sent: u64,
    uptime_sec: f64,
}

pub struct SenderConfig {
    pub server_url: String,
    pub receiver_name: String,
    pub api_key: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub batch_interval: Duration,
    pub heartbeat_interval: Duration,
}

pub struct Sender {
    config: SenderConfig,
    client: Client,
    buffer: VecDeque<FramePayload>,
    stats: Arc<Stats>,
}

impl Sender {
    pub fn new(config: SenderConfig, stats: Arc<Stats>) -> Self {
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("failed to create HTTP client");

        Self {
            config,
            client,
            buffer: VecDeque::with_capacity(MAX_BUFFER),
            stats,
        }
    }

    pub async fn run(mut self, mut rx: mpsc::Receiver<String>) {
        let mut batch_interval = time::interval(self.config.batch_interval);
        let mut heartbeat_interval = time::interval(self.config.heartbeat_interval);
        // Don't send immediately on first tick
        batch_interval.tick().await;
        heartbeat_interval.tick().await;

        loop {
            tokio::select! {
                frame = rx.recv() => {
                    match frame {
                        Some(hex) => {
                            self.buffer_frame(hex);
                        }
                        None => {
                            // Channel closed — flush and exit
                            eprintln!(
                                "[sender] channel closed, flushing {} buffered frames",
                                self.buffer.len()
                            );
                            self.flush_batch().await;
                            break;
                        }
                    }
                }
                _ = batch_interval.tick() => {
                    self.flush_batch().await;
                }
                _ = heartbeat_interval.tick() => {
                    self.send_heartbeat().await;
                }
            }
        }
    }

    fn buffer_frame(&mut self, hex: String) {
        if self.buffer.len() >= MAX_BUFFER {
            self.buffer.pop_front();
        }
        self.buffer.push_back(FramePayload {
            hex,
            timestamp: None,
            signal_level: None,
        });
    }

    async fn flush_batch(&mut self) {
        if self.buffer.is_empty() {
            return;
        }

        let frames: Vec<FramePayload> = self.buffer.drain(..).collect();
        let count = frames.len() as u64;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();

        let payload = IngestRequest {
            receiver: self.config.receiver_name.clone(),
            lat: self.config.lat,
            lon: self.config.lon,
            frames,
            timestamp: now,
        };

        let url = format!("{}/api/v1/frames", self.config.server_url);
        let mut req = self.client.post(&url).json(&payload);
        if let Some(ref key) = self.config.api_key {
            req = req.bearer_auth(key);
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                self.stats.frames_sent.fetch_add(count, Ordering::Relaxed);
            }
            Ok(resp) => {
                eprintln!(
                    "[sender] server returned {}, re-buffering {} frames",
                    resp.status(),
                    count
                );
                self.re_buffer(payload.frames);
            }
            Err(e) => {
                eprintln!(
                    "[sender] request failed: {}, re-buffering {} frames",
                    e, count
                );
                self.re_buffer(payload.frames);
            }
        }
    }

    fn re_buffer(&mut self, frames: Vec<FramePayload>) {
        // Put frames back at the front of the buffer
        for frame in frames.into_iter().rev() {
            if self.buffer.len() >= MAX_BUFFER {
                break;
            }
            self.buffer.push_front(frame);
        }
    }

    async fn send_heartbeat(&self) {
        let payload = HeartbeatRequest {
            receiver: self.config.receiver_name.clone(),
            lat: self.config.lat,
            lon: self.config.lon,
            frames_captured: self.stats.frames_captured.load(Ordering::Relaxed),
            frames_sent: self.stats.frames_sent.load(Ordering::Relaxed),
            uptime_sec: self.stats.uptime_secs(),
        };

        let url = format!("{}/api/v1/heartbeat", self.config.server_url);
        let mut req = self.client.post(&url).json(&payload);
        if let Some(ref key) = self.config.api_key {
            req = req.bearer_auth(key);
        }

        if let Err(e) = req.send().await {
            eprintln!("[sender] heartbeat failed: {}", e);
        }
    }
}

// Standalone functions for testing serialization

#[cfg(test)]
pub fn build_ingest_payload(
    receiver: &str,
    lat: Option<f64>,
    lon: Option<f64>,
    frames: Vec<FramePayload>,
    timestamp: f64,
) -> serde_json::Value {
    serde_json::json!({
        "receiver": receiver,
        "lat": lat,
        "lon": lon,
        "frames": frames,
        "timestamp": timestamp,
    })
}

#[cfg(test)]
pub fn build_heartbeat_payload(
    receiver: &str,
    lat: Option<f64>,
    lon: Option<f64>,
    captured: u64,
    sent: u64,
    uptime: f64,
) -> serde_json::Value {
    serde_json::json!({
        "receiver": receiver,
        "lat": lat,
        "lon": lon,
        "frames_captured": captured,
        "frames_sent": sent,
        "uptime_sec": uptime,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_payload_serialization() {
        let frame = FramePayload {
            hex: "8D4840D6202CC371C32CE0576098".to_string(),
            timestamp: None,
            signal_level: None,
        };
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["hex"], "8D4840D6202CC371C32CE0576098");
        // Optional fields should be absent when None
        assert!(json.get("timestamp").is_none());
        assert!(json.get("signal_level").is_none());
    }

    #[test]
    fn test_frame_payload_with_optional_fields() {
        let frame = FramePayload {
            hex: "8D4840D6202CC371C32CE0576098".to_string(),
            timestamp: Some(1709500000.123),
            signal_level: Some(-8.5),
        };
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["hex"], "8D4840D6202CC371C32CE0576098");
        assert_eq!(json["timestamp"], 1709500000.123);
        assert_eq!(json["signal_level"], -8.5);
    }

    #[test]
    fn test_ingest_payload_format() {
        let frames = vec![
            FramePayload {
                hex: "8D4840D6202CC371C32CE0576098".to_string(),
                timestamp: None,
                signal_level: None,
            },
            FramePayload {
                hex: "8D4CA251204994B1C36E60A5343D".to_string(),
                timestamp: None,
                signal_level: None,
            },
        ];
        let payload =
            build_ingest_payload("pi5-test", Some(35.5), Some(-82.5), frames, 1709500000.0);
        assert_eq!(payload["receiver"], "pi5-test");
        assert_eq!(payload["lat"], 35.5);
        assert_eq!(payload["lon"], -82.5);
        assert_eq!(payload["timestamp"], 1709500000.0);
        assert_eq!(payload["frames"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_ingest_payload_no_location() {
        let frames = vec![FramePayload {
            hex: "AABBCCDD11223344AABBCCDD1122".to_string(),
            timestamp: None,
            signal_level: None,
        }];
        let payload = build_ingest_payload("test", None, None, frames, 1709500000.0);
        assert!(payload["lat"].is_null());
        assert!(payload["lon"].is_null());
    }

    #[test]
    fn test_heartbeat_payload_format() {
        let payload =
            build_heartbeat_payload("pi5-test", Some(35.5), Some(-82.5), 1000, 950, 120.5);
        assert_eq!(payload["receiver"], "pi5-test");
        assert_eq!(payload["lat"], 35.5);
        assert_eq!(payload["lon"], -82.5);
        assert_eq!(payload["frames_captured"], 1000);
        assert_eq!(payload["frames_sent"], 950);
        assert_eq!(payload["uptime_sec"], 120.5);
    }

    #[test]
    fn test_heartbeat_payload_no_location() {
        let payload = build_heartbeat_payload("test", None, None, 0, 0, 0.0);
        assert!(payload["lat"].is_null());
        assert!(payload["lon"].is_null());
    }

    #[test]
    fn test_buffer_cap() {
        let stats = Arc::new(Stats::new());
        let config = SenderConfig {
            server_url: "http://localhost".to_string(),
            receiver_name: "test".to_string(),
            api_key: None,
            lat: None,
            lon: None,
            batch_interval: Duration::from_secs(2),
            heartbeat_interval: Duration::from_secs(30),
        };
        let mut sender = Sender::new(config, stats);

        // Fill beyond MAX_BUFFER
        for i in 0..1050 {
            sender.buffer_frame(format!("{:04X}", i));
        }
        assert_eq!(sender.buffer.len(), MAX_BUFFER);
        // Oldest frames should have been dropped — first 50 evicted
        // 1050 - 1000 = 50, so oldest remaining is index 50 = 0x0032
        assert_eq!(sender.buffer.front().unwrap().hex, "0032");
    }

    #[test]
    fn test_re_buffer() {
        let stats = Arc::new(Stats::new());
        let config = SenderConfig {
            server_url: "http://localhost".to_string(),
            receiver_name: "test".to_string(),
            api_key: None,
            lat: None,
            lon: None,
            batch_interval: Duration::from_secs(2),
            heartbeat_interval: Duration::from_secs(30),
        };
        let mut sender = Sender::new(config, stats);

        // Add some frames to buffer
        sender.buffer_frame("AAAA".to_string());
        sender.buffer_frame("BBBB".to_string());

        // Simulate re-buffering failed frames
        let failed = vec![
            FramePayload {
                hex: "1111".to_string(),
                timestamp: None,
                signal_level: None,
            },
            FramePayload {
                hex: "2222".to_string(),
                timestamp: None,
                signal_level: None,
            },
        ];
        sender.re_buffer(failed);

        // Failed frames should be at the front
        assert_eq!(sender.buffer.len(), 4);
        assert_eq!(sender.buffer[0].hex, "1111");
        assert_eq!(sender.buffer[1].hex, "2222");
        assert_eq!(sender.buffer[2].hex, "AAAA");
        assert_eq!(sender.buffer[3].hex, "BBBB");
    }
}
