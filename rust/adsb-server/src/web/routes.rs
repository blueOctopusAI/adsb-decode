//! REST API route handlers.
//!
//! Each handler opens its own DB connection (like Flask's g.db pattern).
//! When a live tracker is attached, position/aircraft endpoints serve from
//! in-memory state for sub-second latency.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use adsb_core::types::icao_to_string;

use crate::web::AppState;

// ---------------------------------------------------------------------------
// Query param types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct AircraftParams {
    military: Option<bool>,
}

#[derive(Deserialize)]
pub struct PositionParams {
    minutes: Option<f64>,
}

#[derive(Deserialize)]
pub struct TrailParams {
    minutes: Option<f64>,
    limit: Option<i64>,
}

#[derive(Deserialize)]
pub struct EventParams {
    r#type: Option<String>,
    limit: Option<i64>,
    icao: Option<String>,
}

#[derive(Deserialize)]
pub struct QueryParams {
    min_alt: Option<i32>,
    max_alt: Option<i32>,
    icao: Option<String>,
    military: Option<bool>,
    limit: Option<i64>,
}

#[derive(Deserialize)]
pub struct HeatmapParams {
    minutes: Option<f64>,
}

#[derive(Deserialize)]
pub struct AllPositionParams {
    limit: Option<i64>,
}

#[derive(Deserialize)]
pub struct GeofenceBody {
    name: String,
    lat: f64,
    lon: f64,
    radius_nm: f64,
    description: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn clamp(val: f64, min: f64, max: f64) -> f64 {
    val.max(min).min(max)
}

fn clamp_i64(val: i64, min: i64, max: i64) -> i64 {
    val.max(min).min(max)
}

// ---------------------------------------------------------------------------
// Aircraft endpoints
// ---------------------------------------------------------------------------

/// GET /api/aircraft — list all aircraft.
pub async fn api_aircraft(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AircraftParams>,
) -> impl IntoResponse {
    // Dual-path: live tracker or DB
    if let Some(tracker) = &state.tracker {
        let tracker = tracker.read().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let active = tracker.get_active(now);
        let aircraft: Vec<Value> = active
            .iter()
            .filter(|ac| {
                if params.military == Some(true) {
                    ac.is_military
                } else {
                    true
                }
            })
            .map(|ac| {
                json!({
                    "icao": icao_to_string(&ac.icao),
                    "callsign": ac.callsign,
                    "squawk": ac.squawk,
                    "lat": ac.lat,
                    "lon": ac.lon,
                    "altitude_ft": ac.altitude_ft,
                    "speed_kts": ac.speed_kts,
                    "heading_deg": ac.heading_deg,
                    "vertical_rate_fpm": ac.vertical_rate_fpm,
                    "country": ac.country,
                    "is_military": ac.is_military,
                    "messages": ac.message_count,
                    "first_seen": ac.first_seen,
                    "last_seen": ac.last_seen,
                })
            })
            .collect();
        return Json(json!(aircraft));
    }

    let mut aircraft = state.db.get_all_aircraft().await;
    if params.military == Some(true) {
        aircraft.retain(|a| a.is_military);
    }

    Json(serde_json::to_value(&aircraft).unwrap_or(json!([])))
}

/// GET /api/aircraft/:icao — single aircraft detail + positions + events.
pub async fn api_aircraft_detail(
    State(state): State<Arc<AppState>>,
    Path(icao): Path<String>,
) -> impl IntoResponse {
    let icao_upper = icao.to_ascii_uppercase();

    let aircraft = match state.db.get_aircraft(&icao_upper).await {
        Some(ac) => ac,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Aircraft not found"})),
            )
                .into_response()
        }
    };

    let positions = state.db.get_positions(&icao_upper, 100).await;
    let events = state.db.get_events(None, Some(&icao_upper), 50).await;

    Json(json!({
        "aircraft": aircraft,
        "positions": positions,
        "events": events,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// Position endpoints
// ---------------------------------------------------------------------------

/// GET /api/positions — recent positions for map polling.
pub async fn api_positions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PositionParams>,
) -> impl IntoResponse {
    let minutes = clamp(params.minutes.unwrap_or(5.0), 1.0, 525600.0);

    // Dual-path: live tracker for sub-second latency
    if let Some(tracker) = &state.tracker {
        let tracker = tracker.read().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let cutoff = now - (minutes * 60.0);
        let active = tracker.get_active(now);
        let positions: Vec<Value> = active
            .iter()
            .filter(|ac| ac.has_position() && ac.last_seen >= cutoff)
            .map(|ac| {
                json!({
                    "icao": icao_to_string(&ac.icao),
                    "lat": ac.lat,
                    "lon": ac.lon,
                    "altitude_ft": ac.altitude_ft,
                    "speed_kts": ac.speed_kts,
                    "heading_deg": ac.heading_deg,
                    "vertical_rate_fpm": ac.vertical_rate_fpm,
                    "timestamp": ac.last_seen,
                    "callsign": ac.callsign,
                })
            })
            .collect();
        return Json(json!(positions));
    }

    let positions = state.db.get_recent_positions(minutes, 50000).await;
    Json(serde_json::to_value(&positions).unwrap_or(json!([])))
}

/// GET /api/trails — position trails per aircraft.
pub async fn api_trails(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TrailParams>,
) -> impl IntoResponse {
    let minutes = clamp(params.minutes.unwrap_or(60.0), 1.0, 1440.0);
    let limit = clamp_i64(params.limit.unwrap_or(500), 1, 5000);

    let positions = state.db.get_trails(minutes, limit).await;

    // Group by ICAO
    let mut trails: std::collections::HashMap<String, Vec<Value>> =
        std::collections::HashMap::new();
    for pos in &positions {
        trails
            .entry(pos.icao.clone())
            .or_default()
            .push(json!({
                "lat": pos.lat,
                "lon": pos.lon,
                "altitude_ft": pos.altitude_ft,
                "timestamp": pos.timestamp,
            }));
    }

    Json(json!(trails))
}

/// GET /api/positions/all — all positions for replay.
pub async fn api_positions_all(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AllPositionParams>,
) -> impl IntoResponse {
    let limit = clamp_i64(params.limit.unwrap_or(50000), 1, 100000);

    let positions = state.db.get_all_positions_ordered(limit).await;
    Json(serde_json::to_value(&positions).unwrap_or(json!([])))
}

// ---------------------------------------------------------------------------
// Events + Stats
// ---------------------------------------------------------------------------

/// GET /api/events — recent events with optional filters.
pub async fn api_events(
    State(state): State<Arc<AppState>>,
    Query(params): Query<EventParams>,
) -> impl IntoResponse {
    let limit = clamp_i64(params.limit.unwrap_or(100), 1, 10000);

    let events = state
        .db
        .get_events(params.r#type.as_deref(), params.icao.as_deref(), limit)
        .await;
    Json(serde_json::to_value(&events).unwrap_or(json!([])))
}

/// GET /api/stats — database statistics.
pub async fn api_stats(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let stats = state.db.stats().await;
    Json(serde_json::to_value(&stats).unwrap_or(json!({})))
}

// ---------------------------------------------------------------------------
// Query + Heatmap
// ---------------------------------------------------------------------------

/// GET /api/query — filtered position query.
pub async fn api_query(
    State(state): State<Arc<AppState>>,
    Query(params): Query<QueryParams>,
) -> impl IntoResponse {
    let limit = clamp_i64(params.limit.unwrap_or(1000), 1, 50000);

    let positions = state
        .db
        .query_positions(
            params.min_alt,
            params.max_alt,
            params.icao.as_deref(),
            params.military.unwrap_or(false),
            limit,
        )
        .await;
    Json(serde_json::to_value(&positions).unwrap_or(json!([])))
}

/// GET /api/heatmap — position density data.
pub async fn api_heatmap(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HeatmapParams>,
) -> impl IntoResponse {
    let minutes = clamp(params.minutes.unwrap_or(1440.0), 1.0, 10080.0);

    let points = state.db.get_heatmap_positions(minutes, 50000).await;
    let data: Vec<Value> = points
        .iter()
        .map(|(lat, lon, alt)| json!({"lat": lat, "lon": lon, "altitude_ft": alt}))
        .collect();
    Json(json!(data))
}

// ---------------------------------------------------------------------------
// Airports
// ---------------------------------------------------------------------------

/// GET /api/airports — built-in airport list.
pub async fn api_airports() -> impl IntoResponse {
    // Return the 4 built-in airports from the enrich module
    let airports = vec![
        json!({"icao": "KATL", "name": "Atlanta Hartsfield-Jackson", "lat": 33.6367, "lon": -84.4281, "elevation_ft": 1026, "type": "major"}),
        json!({"icao": "KCLT", "name": "Charlotte Douglas", "lat": 35.2140, "lon": -80.9431, "elevation_ft": 748, "type": "major"}),
        json!({"icao": "KAVL", "name": "Asheville Regional", "lat": 35.4362, "lon": -82.5418, "elevation_ft": 2165, "type": "medium"}),
        json!({"icao": "KTYS", "name": "Knoxville McGhee Tyson", "lat": 35.8110, "lon": -83.9940, "elevation_ft": 981, "type": "medium"}),
    ];
    Json(json!(airports))
}

// ---------------------------------------------------------------------------
// Geofences
// ---------------------------------------------------------------------------

/// GET /api/geofences — list all geofences.
pub async fn api_geofences_list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let fences = state.geofences.read().unwrap();
    Json(serde_json::to_value(&*fences).unwrap_or(json!([])))
}

/// POST /api/geofences — add a geofence.
pub async fn api_geofences_add(
    State(state): State<Arc<AppState>>,
    Json(body): Json<GeofenceBody>,
) -> impl IntoResponse {
    if body.name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name is required"})),
        );
    }
    if body.radius_nm <= 0.0 || body.radius_nm > 500.0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "radius_nm must be between 0 and 500"})),
        );
    }

    let mut next_id = state.geofence_next_id.write().unwrap();
    let id = *next_id;
    *next_id += 1;

    let entry = super::GeofenceEntry {
        id,
        name: body.name,
        lat: body.lat,
        lon: body.lon,
        radius_nm: body.radius_nm,
        description: body.description,
    };

    let response = serde_json::to_value(&entry).unwrap();
    state.geofences.write().unwrap().push(entry);

    (StatusCode::CREATED, Json(response))
}

/// DELETE /api/geofences/:id — remove a geofence.
pub async fn api_geofences_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    let mut fences = state.geofences.write().unwrap();
    let len_before = fences.len();
    fences.retain(|f| f.id != id);

    if fences.len() < len_before {
        (StatusCode::OK, Json(json!({"deleted": id})))
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Geofence not found"})),
        )
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

    use crate::db::{Database, SqliteDb};

    fn test_state() -> (Arc<AppState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_str().unwrap().to_string();

        // Create DB with test data
        let mut db = Database::open(&db_path).unwrap();
        let icao = adsb_core::types::icao_from_hex("4840D6").unwrap();
        db.upsert_aircraft(&icao, Some("Netherlands"), None, false, 1.0);
        db.add_position(&icao, 52.25, 3.92, Some(38000), Some(450.0), Some(90.0), None, None, 1.0);
        db.add_event(&icao, "military", "Test event", Some(52.25), Some(3.92), Some(38000), 1.0);
        drop(db);

        let state = Arc::new(AppState {
            db: Arc::new(SqliteDb::new(db_path)),
            tracker: None,
            geofences: RwLock::new(Vec::new()),
            geofence_next_id: RwLock::new(1),
        });
        (state, dir)
    }

    #[tokio::test]
    async fn test_api_stats() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/stats").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["aircraft"], 1);
        assert_eq!(json["positions"], 1);
    }

    #[tokio::test]
    async fn test_api_aircraft() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/aircraft")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().len() >= 1);
    }

    #[tokio::test]
    async fn test_api_aircraft_detail() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/aircraft/4840D6")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["aircraft"]["icao"], "4840D6");
    }

    #[tokio::test]
    async fn test_api_aircraft_not_found() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/aircraft/FFFFFF")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_api_events() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().len() >= 1);
    }

    #[tokio::test]
    async fn test_api_geofences_crud() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_str().unwrap().to_string();
        let state = Arc::new(AppState {
            db: Arc::new(SqliteDb::new(db_path)),
            tracker: None,
            geofences: RwLock::new(Vec::new()),
            geofence_next_id: RwLock::new(1),
        });

        // Create geofence
        let app = crate::web::build_router(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/geofences")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"test","lat":35.0,"lon":-82.0,"radius_nm":10.0}"#,
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
        assert_eq!(json["id"], 1);

        // List geofences
        let app = crate::web::build_router(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/geofences")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1);

        // Delete geofence
        let app = crate::web::build_router(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/geofences/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(state.geofences.read().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_api_airports() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_str().unwrap().to_string();
        let _keep = &dir;
        let app = crate::web::build_router(Arc::new(AppState {
            db: Arc::new(SqliteDb::new(db_path)),
            tracker: None,
            geofences: RwLock::new(Vec::new()),
            geofence_next_id: RwLock::new(1),
        }));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/airports")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn test_api_positions_clamped() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state);

        // Minutes clamped to valid range
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/positions?minutes=999999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_heatmap() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/heatmap?minutes=1440")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_query() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/query?min_alt=30000&limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
