//! Multi-receiver ingest API — feeders POST frames here.
//!
//! Each feeder identifies by name. The server maintains a tracker per
//! feeder and merges decoded data into the shared database via the
//! AdsbDatabase trait.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use adsb_core::frame::{self, IcaoCache};
use adsb_core::tracker::{TrackEvent, Tracker};
use adsb_core::types::icao_to_string;

use crate::web::AppState;

// ---------------------------------------------------------------------------
// Per-feeder state (module-level, protected by RwLock)
// ---------------------------------------------------------------------------

use std::sync::LazyLock;

static FEEDER_TRACKERS: LazyLock<RwLock<HashMap<String, FeederState>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

static RECEIVER_STATUS: LazyLock<RwLock<HashMap<String, ReceiverStatus>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

struct FeederState {
    tracker: Tracker,
    icao_cache: IcaoCache,
}

#[derive(Clone, serde::Serialize)]
struct ReceiverStatus {
    name: String,
    lat: Option<f64>,
    lon: Option<f64>,
    last_heartbeat: f64,
    frames_captured: u64,
    frames_sent: u64,
    uptime_sec: f64,
    active_aircraft: usize,
    online: bool,
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct IngestRequest {
    receiver: String,
    lat: Option<f64>,
    lon: Option<f64>,
    frames: Vec<FrameData>,
    timestamp: Option<f64>,
}

#[derive(Deserialize)]
pub struct FrameData {
    hex: String,
    timestamp: Option<f64>,
    signal_level: Option<f64>,
}

#[derive(Deserialize)]
pub struct HeartbeatRequest {
    receiver: String,
    lat: Option<f64>,
    lon: Option<f64>,
    frames_captured: Option<u64>,
    frames_sent: Option<u64>,
    uptime_sec: Option<f64>,
}

// ---------------------------------------------------------------------------
// Auth helper
// ---------------------------------------------------------------------------

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

/// Validate bearer token if auth is configured. Returns Err response on failure.
fn check_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<Value>)> {
    let expected = match &state.auth_token {
        Some(t) => t,
        None => return Ok(()), // No auth configured — accept all
    };

    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if let Some(token) = auth_header.strip_prefix("Bearer ") {
        if token == expected {
            return Ok(());
        }
    }

    Err((
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "invalid or missing bearer token"})),
    ))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/v1/frames — batch ingest from a feeder.
pub async fn api_ingest_frames(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<IngestRequest>,
) -> (StatusCode, Json<Value>) {
    if let Err(resp) = check_auth(&state, &headers) {
        return resp;
    }

    let base_ts = body.timestamp.unwrap_or_else(now);

    // Sync section: process frames under FEEDER_TRACKERS lock, collect results.
    // The lock MUST be dropped before any .await to keep the future Send.
    let (accepted, decoded, positions, events_out, all_track_events, active_count) = {
        let mut feeders = FEEDER_TRACKERS.write().unwrap();
        let feeder = feeders
            .entry(body.receiver.clone())
            .or_insert_with(|| FeederState {
                tracker: Tracker::new(None, None, body.lat, body.lon, 2.0),
                icao_cache: IcaoCache::new(60.0),
            });

        let mut accepted = 0u64;
        let mut decoded = 0u64;
        let mut positions = 0u64;
        let mut events_out: Vec<Value> = Vec::new();
        let mut all_track_events: Vec<TrackEvent> = Vec::new();

        for (i, frame_data) in body.frames.iter().enumerate() {
            let ts = frame_data.timestamp.unwrap_or(base_ts + i as f64 * 0.001);
            let parsed = frame::parse_frame(
                &frame_data.hex,
                ts,
                frame_data.signal_level,
                true,
                &mut feeder.icao_cache,
            );

            if let Some(f) = parsed {
                accepted += 1;
                let (msg, track_events) = feeder.tracker.update(&f);
                if msg.is_some() {
                    decoded += 1;
                }
                for te in &track_events {
                    match te {
                        TrackEvent::PositionUpdate { .. } => positions += 1,
                        TrackEvent::NewAircraft {
                            icao, timestamp, ..
                        } => {
                            events_out.push(json!({
                                "type": "new_aircraft",
                                "icao": icao_to_string(icao),
                                "timestamp": timestamp,
                            }));
                        }
                        _ => {}
                    }
                }
                all_track_events.extend(track_events);
            }
        }

        let active_count = feeder.tracker.aircraft.len();
        (accepted, decoded, positions, events_out, all_track_events, active_count)
    }; // lock dropped here

    // Async section: persist to database (no locks held)
    for te in &all_track_events {
        match te {
            TrackEvent::NewAircraft {
                icao,
                country,
                registration,
                is_military,
                timestamp,
            } => {
                let icao_str = icao_to_string(icao);
                state
                    .db
                    .upsert_aircraft(
                        &icao_str,
                        *country,
                        registration.as_deref(),
                        *is_military,
                        *timestamp,
                    )
                    .await;
            }
            TrackEvent::AircraftUpdate { icao, timestamp } => {
                let icao_str = icao_to_string(icao);
                state
                    .db
                    .upsert_aircraft(&icao_str, None, None, false, *timestamp)
                    .await;
            }
            TrackEvent::SightingUpdate {
                icao,
                capture_id,
                callsign,
                squawk,
                altitude_ft,
                timestamp,
            } => {
                let icao_str = icao_to_string(icao);
                state
                    .db
                    .upsert_sighting(
                        &icao_str,
                        *capture_id,
                        callsign.as_deref(),
                        squawk.as_deref(),
                        *altitude_ft,
                        *timestamp,
                    )
                    .await;
            }
            TrackEvent::PositionUpdate {
                icao,
                lat,
                lon,
                altitude_ft,
                speed_kts,
                heading_deg,
                vertical_rate_fpm,
                receiver_id,
                timestamp,
            } => {
                let icao_str = icao_to_string(icao);
                state
                    .db
                    .add_position(
                        &icao_str,
                        *lat,
                        *lon,
                        *altitude_ft,
                        *speed_kts,
                        *heading_deg,
                        *vertical_rate_fpm,
                        *receiver_id,
                        *timestamp,
                    )
                    .await;
            }
        }
    }

    // Update receiver status
    {
        let mut status = RECEIVER_STATUS.write().unwrap();
        let entry = status
            .entry(body.receiver.clone())
            .or_insert_with(|| ReceiverStatus {
                name: body.receiver.clone(),
                lat: body.lat,
                lon: body.lon,
                last_heartbeat: now(),
                frames_captured: 0,
                frames_sent: 0,
                uptime_sec: 0.0,
                active_aircraft: 0,
                online: true,
            });
        entry.last_heartbeat = now();
        entry.lat = body.lat.or(entry.lat);
        entry.lon = body.lon.or(entry.lon);
        entry.active_aircraft = active_count;
    }

    (
        StatusCode::OK,
        Json(json!({
            "accepted": accepted,
            "decoded": decoded,
            "positions": positions,
            "events": events_out,
        })),
    )
}

/// POST /api/v1/heartbeat — receiver status update.
pub async fn api_heartbeat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<HeartbeatRequest>,
) -> (StatusCode, Json<Value>) {
    if let Err(resp) = check_auth(&state, &headers) {
        return resp;
    }

    let mut status = RECEIVER_STATUS.write().unwrap();
    let entry = status
        .entry(body.receiver.clone())
        .or_insert_with(|| ReceiverStatus {
            name: body.receiver.clone(),
            lat: body.lat,
            lon: body.lon,
            last_heartbeat: now(),
            frames_captured: 0,
            frames_sent: 0,
            uptime_sec: 0.0,
            active_aircraft: 0,
            online: true,
        });

    entry.last_heartbeat = now();
    entry.lat = body.lat.or(entry.lat);
    entry.lon = body.lon.or(entry.lon);
    entry.frames_captured = body.frames_captured.unwrap_or(entry.frames_captured);
    entry.frames_sent = body.frames_sent.unwrap_or(entry.frames_sent);
    entry.uptime_sec = body.uptime_sec.unwrap_or(entry.uptime_sec);
    entry.online = true;

    (StatusCode::OK, Json(json!({"ok": true})))
}

/// GET /api/v1/receivers — list all receivers with status.
pub async fn api_receivers(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let status = RECEIVER_STATUS.read().unwrap();
    let current = now();

    let receivers: Vec<Value> = status
        .values()
        .map(|s| {
            let online = (current - s.last_heartbeat) < 120.0; // 2 min timeout
            json!({
                "name": s.name,
                "lat": s.lat,
                "lon": s.lon,
                "last_heartbeat": s.last_heartbeat,
                "frames_captured": s.frames_captured,
                "frames_sent": s.frames_sent,
                "uptime_sec": s.uptime_sec,
                "active_aircraft": s.active_aircraft,
                "online": online,
            })
        })
        .collect();

    Json(json!(receivers))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::RwLock;
    use tower::ServiceExt;

    use crate::db::SqliteDb;

    fn test_state() -> (Arc<AppState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_str().unwrap().to_string();
        let state = Arc::new(AppState {
            db: Arc::new(SqliteDb::new(db_path)),
            tracker: None,
            geofences: RwLock::new(Vec::new()),
            geofence_next_id: RwLock::new(1),
            auth_token: None,
        });
        (state, dir)
    }

    fn test_state_with_auth(token: &str) -> (Arc<AppState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_str().unwrap().to_string();
        let state = Arc::new(AppState {
            db: Arc::new(SqliteDb::new(db_path)),
            tracker: None,
            geofences: RwLock::new(Vec::new()),
            geofence_next_id: RwLock::new(1),
            auth_token: Some(token.to_string()),
        });
        (state, dir)
    }

    #[tokio::test]
    async fn test_api_receivers_empty() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/receivers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_heartbeat() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/heartbeat")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"receiver":"test-rx","lat":35.5,"lon":-82.5,"uptime_sec":100}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_ingest_frames() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/frames")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"receiver":"test","frames":[{"hex":"8D4840D6202CC371C32CE0576098"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json["accepted"].as_u64().unwrap() >= 1);
    }

    #[tokio::test]
    async fn test_auth_reject_without_token() {
        let (state, _dir) = test_state_with_auth("secret-token-123");
        let app = crate::web::build_router(state, None);

        // No auth header — should get 401
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/frames")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"receiver":"test","frames":[{"hex":"8D4840D6202CC371C32CE0576098"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_reject_wrong_token() {
        let (state, _dir) = test_state_with_auth("secret-token-123");
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/frames")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer wrong-token")
                    .body(Body::from(
                        r#"{"receiver":"test","frames":[{"hex":"8D4840D6202CC371C32CE0576098"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_accept_correct_token() {
        let (state, _dir) = test_state_with_auth("secret-token-123");
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/frames")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer secret-token-123")
                    .body(Body::from(
                        r#"{"receiver":"test","frames":[{"hex":"8D4840D6202CC371C32CE0576098"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_heartbeat_reject() {
        let (state, _dir) = test_state_with_auth("secret-token-123");
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/heartbeat")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"receiver":"test-rx","lat":35.5,"lon":-82.5}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_no_auth_accepts_all() {
        // No auth_token configured — should accept without any header
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/frames")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"receiver":"test","frames":[{"hex":"8D4840D6202CC371C32CE0576098"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
