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

/// Collect all aircraft from all feeder trackers for filter processing.
pub fn collect_all_aircraft() -> Vec<adsb_core::tracker::AircraftState> {
    let feeders = FEEDER_TRACKERS.read().unwrap();
    let mut all = Vec::new();
    for feeder in feeders.values() {
        for ac in feeder.tracker.aircraft.values() {
            all.push(ac.clone());
        }
    }
    all
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

/// Extract bearer token from Authorization header.
fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
}

/// Validate auth. Returns resolved receiver_id on success.
///
/// Two modes:
/// - **Legacy**: `auth_token` is set → single global token, no receiver_id
/// - **Per-receiver**: no auth_token → look up bearer in receivers table
async fn check_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<i64>, (StatusCode, Json<Value>)> {
    // Legacy mode: single global token
    if let Some(expected) = &state.auth_token {
        if let Some(token) = extract_bearer(headers) {
            if token == expected {
                return Ok(None); // Authenticated, no specific receiver_id
            }
        }
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid or missing bearer token"})),
        ));
    }

    // Per-receiver mode: look up API key in database
    if let Some(token) = extract_bearer(headers) {
        if let Some((receiver_id, _name)) = state.db.lookup_receiver_by_api_key(token).await {
            return Ok(Some(receiver_id));
        }
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid API key"})),
        ));
    }

    // No auth configured and no token provided — accept (demo/local mode)
    Ok(None)
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
    let auth_receiver_id = match check_auth(&state, &headers).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    // Reject oversized batches to prevent DoS
    const MAX_FRAMES_PER_BATCH: usize = 5000;
    if body.frames.len() > MAX_FRAMES_PER_BATCH {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error": format!("too many frames (max {})", MAX_FRAMES_PER_BATCH)})),
        );
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
        (
            accepted,
            decoded,
            positions,
            events_out,
            all_track_events,
            active_count,
        )
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
                // Prefer auth-resolved receiver_id over tracker-assigned one
                let rid = auth_receiver_id.or(*receiver_id);
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
                        rid,
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
    if let Err(resp) = check_auth(&state, &headers).await {
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
// Registration
// ---------------------------------------------------------------------------

static REGISTER_RATE_LIMIT: LazyLock<RwLock<HashMap<String, Vec<f64>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

const MAX_REGISTRATIONS_PER_HOUR: usize = 5;
const MAX_GLOBAL_REGISTRATIONS_PER_HOUR: usize = 20;

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub name: String,
    pub email: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub description: Option<String>,
}

/// POST /api/register — self-service receiver registration.
pub async fn api_register(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RegisterRequest>,
) -> (StatusCode, Json<Value>) {
    let name = body.name.trim();
    if name.is_empty() || name.len() > 64 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name must be 1-64 characters"})),
        );
    }
    // Reject names with HTML/script content to prevent stored XSS
    if name.contains('<') || name.contains('>') {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name contains invalid characters"})),
        );
    }

    // Rate limit by IP — use X-Forwarded-For when behind reverse proxy,
    // but also enforce a global cap so header spoofing can't bypass limits.
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or("unknown").trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    {
        let current = now();
        let mut limits = REGISTER_RATE_LIMIT.write().unwrap();

        // Global rate limit — cap total registrations/hour regardless of IP
        let global = limits.entry("__global__".to_string()).or_default();
        global.retain(|t| current - t < 3600.0);
        if global.len() >= MAX_GLOBAL_REGISTRATIONS_PER_HOUR {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"error": "too many registrations, try again later"})),
            );
        }

        // Per-IP rate limit
        let entries = limits.entry(ip.clone()).or_default();
        entries.retain(|t| current - t < 3600.0);
        if entries.len() >= MAX_REGISTRATIONS_PER_HOUR {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"error": "too many registrations, try again later"})),
            );
        }

        entries.push(current);
        // Also record in global counter
        let global = limits.get_mut("__global__").unwrap();
        global.push(current);
    }

    let email = body
        .email
        .as_deref()
        .map(|e| e.trim())
        .filter(|e| !e.is_empty());

    match state
        .db
        .register_receiver(name, email, body.lat, body.lon, body.description.as_deref())
        .await
    {
        Some((id, api_key)) => {
            // Fire-and-forget POST to shared Supabase leads table
            if let Some(email_val) = email {
                if let (Ok(supa_url), Ok(supa_key)) = (
                    std::env::var("SUPABASE_URL"),
                    std::env::var("SUPABASE_ANON_KEY"),
                ) {
                    let url = format!("{}/rest/v1/leads", supa_url);
                    let body = json!({
                        "email": email_val,
                        "source": "adsb-decode-registration",
                        "product": "adsb-decode",
                        "metadata": { "receiver_name": name, "receiver_id": id }
                    });
                    tokio::spawn(async move {
                        let _ = reqwest::Client::new()
                            .post(&url)
                            .header("apikey", &supa_key)
                            .header("Authorization", format!("Bearer {}", supa_key))
                            .header("Content-Type", "application/json")
                            .json(&body)
                            .send()
                            .await;
                    });
                }
            }

            (
                StatusCode::CREATED,
                Json(json!({
                    "id": id,
                    "name": name,
                    "api_key": api_key,
                    "instructions": format!(
                        "Use this API key in the Authorization header: Bearer {}",
                        api_key
                    ),
                })),
            )
        }
        None => (
            StatusCode::CONFLICT,
            Json(json!({"error": "a receiver with that name already exists"})),
        ),
    }
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

            photo_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
            airspace_cache: std::sync::Mutex::new(None),
            ollama_url: None,
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

            photo_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
            airspace_cache: std::sync::Mutex::new(None),
            ollama_url: None,
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

    // -----------------------------------------------------------------------
    // Registration tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_register_page_loads() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/register")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_register_success() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/register")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"test-receiver","lat":35.5,"lon":-82.5,"description":"My receiver"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "test-receiver");
        assert!(json["api_key"].as_str().unwrap().len() == 36); // UUID v4
        assert!(json["id"].as_i64().unwrap() > 0);
    }

    #[tokio::test]
    async fn test_register_duplicate_name() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state.clone(), None);

        // First registration
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/register")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"dup-rx"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // Second with same name
        let app2 = crate::web::build_router(state, None);
        let response = app2
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/register")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"dup-rx"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_register_empty_name() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/register")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_register_name_too_long() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);
        let long_name = "a".repeat(65);
        let body = format!(r#"{{"name":"{}"}}"#, long_name);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/register")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // -----------------------------------------------------------------------
    // Per-receiver auth tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_per_receiver_auth_with_api_key() {
        let (state, _dir) = test_state();

        // Register a receiver to get an API key
        let api_key = state
            .db
            .register_receiver("auth-test-rx", None, Some(35.5), Some(-82.5), None)
            .await
            .unwrap()
            .1;

        let app = crate::web::build_router(state, None);

        // Use the API key to ingest — should succeed
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/frames")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {}", api_key))
                    .body(Body::from(
                        r#"{"receiver":"auth-test-rx","frames":[{"hex":"8D4840D6202CC371C32CE0576098"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_per_receiver_auth_invalid_key() {
        let (state, _dir) = test_state();

        // Register a receiver so per-receiver mode has at least one key
        state
            .db
            .register_receiver("some-rx", None, None, None, None)
            .await;

        let app = crate::web::build_router(state, None);

        // Use a bogus key — should get 401
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/frames")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer bogus-key-12345")
                    .body(Body::from(
                        r#"{"receiver":"test","frames":[{"hex":"8D4840D6202CC371C32CE0576098"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
