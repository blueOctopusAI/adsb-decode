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

use crate::db::{AircraftRow, HistoryRow};
use crate::web::AppState;

// ---------------------------------------------------------------------------
// Query param types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct AircraftParams {
    military: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
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
    /// Grid cell size in degrees (default 0.01 ≈ 1km). Smaller = finer detail.
    resolution: Option<f64>,
}

#[derive(Deserialize)]
pub struct AllPositionParams {
    limit: Option<i64>,
    start: Option<f64>,
    end: Option<f64>,
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

    let total = aircraft.len();
    let offset = params.offset.unwrap_or(0).max(0) as usize;
    let limit = params.limit.unwrap_or(500).clamp(1, 5000) as usize;
    let page: Vec<_> = aircraft.into_iter().skip(offset).take(limit).collect();

    Json(json!({
        "aircraft": page,
        "total": total,
        "offset": offset,
        "limit": limit,
    }))
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
                    "is_military": ac.is_military,
                    "country": ac.country,
                    "registration": ac.registration,
                })
            })
            .collect();
        return Json(json!(positions));
    }

    let positions = state.db.get_recent_positions(minutes, 50000).await;

    // Enrich positions with aircraft + sighting data (callsign, registration, etc.)
    // so the map has the same fields as the live tracker path.
    let aircraft = state.db.get_all_aircraft().await;
    let history = state.db.get_aircraft_history(minutes / 60.0 + 1.0).await;

    let ac_map: std::collections::HashMap<&str, &AircraftRow> =
        aircraft.iter().map(|a| (a.icao.as_str(), a)).collect();
    let hist_map: std::collections::HashMap<&str, &HistoryRow> =
        history.iter().map(|h| (h.icao.as_str(), h)).collect();

    let enriched: Vec<Value> = positions
        .iter()
        .map(|p| {
            let ac = ac_map.get(p.icao.as_str());
            let hi = hist_map.get(p.icao.as_str());
            json!({
                "icao": p.icao,
                "lat": p.lat,
                "lon": p.lon,
                "altitude_ft": p.altitude_ft,
                "speed_kts": p.speed_kts,
                "heading_deg": p.heading_deg,
                "vertical_rate_fpm": p.vertical_rate_fpm,
                "timestamp": p.timestamp,
                "callsign": hi.and_then(|h| h.callsign.as_deref()),
                "registration": ac.and_then(|a| a.registration.as_deref()),
                "country": ac.and_then(|a| a.country.as_deref()),
                "is_military": ac.map(|a| a.is_military).unwrap_or(false),
            })
        })
        .collect();

    Json(json!(enriched))
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
        trails.entry(pos.icao.clone()).or_default().push(json!({
            "lat": pos.lat,
            "lon": pos.lon,
            "altitude_ft": pos.altitude_ft,
            "timestamp": pos.timestamp,
        }));
    }

    Json(json!({ "trails": trails, "count": trails.len() }))
}

/// GET /api/positions/all — all positions for replay.
///
/// Optional `start` and `end` query params (epoch seconds) filter the time range.
pub async fn api_positions_all(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AllPositionParams>,
) -> impl IntoResponse {
    let limit = clamp_i64(params.limit.unwrap_or(50000), 1, 100000);

    let positions = state
        .db
        .get_all_positions_ordered(limit, params.start, params.end)
        .await;
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

/// GET /api/heatmap — grid-aggregated position density data.
///
/// Returns `{lat, lon, count, avg_alt}` cells. Grid resolution defaults to
/// 0.01° (~1 km) and can be adjusted via `?resolution=0.005`.
pub async fn api_heatmap(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HeatmapParams>,
) -> impl IntoResponse {
    let minutes = clamp(params.minutes.unwrap_or(1440.0), 1.0, 10080.0);
    let resolution = clamp(params.resolution.unwrap_or(0.01), 0.001, 1.0);

    let cells = state.db.get_heatmap_density(minutes, resolution).await;
    Json(json!(cells))
}

// ---------------------------------------------------------------------------
// Airports
// ---------------------------------------------------------------------------

/// GET /api/airports — built-in airport list.
pub async fn api_airports() -> impl IntoResponse {
    let airports = adsb_core::enrich::all_airports();
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
    if body.name.is_empty() || body.name.len() > 64 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name must be 1-64 characters"})),
        );
    }
    if body.name.contains('<') || body.name.contains('>') {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name contains invalid characters"})),
        );
    }
    if body.radius_nm <= 0.0 || body.radius_nm > 500.0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "radius_nm must be between 0 and 500"})),
        );
    }
    if !(-90.0..=90.0).contains(&body.lat) || !(-180.0..=180.0).contains(&body.lon) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid lat/lon coordinates"})),
        );
    }
    // Cap total geofences to prevent memory exhaustion
    {
        let fences = state.geofences.read().unwrap();
        if fences.len() >= 100 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "maximum 100 geofences reached"})),
            );
        }
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
// HexDB lookup
// ---------------------------------------------------------------------------

/// Validate that an ICAO string is 1-6 hex characters.
fn is_valid_icao(s: &str) -> bool {
    !s.is_empty() && s.len() <= 6 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// GET /api/lookup/:icao — proxy lookup to hexdb.io for aircraft metadata.
pub async fn api_lookup(Path(icao): Path<String>) -> impl IntoResponse {
    let icao_upper = icao.to_ascii_uppercase();
    if !is_valid_icao(&icao_upper) {
        return Json(json!({"error": "invalid ICAO hex"})).into_response();
    }
    let url = format!("https://hexdb.io/hex-{icao_upper}-v2.json");

    let client = reqwest::Client::new();
    let result = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await;

    match result {
        Ok(resp) => match resp.json::<Value>().await {
            Ok(data) => Json(data).into_response(),
            Err(_) => Json(json!({"error": "lookup failed"})).into_response(),
        },
        Err(_) => Json(json!({"error": "lookup failed"})).into_response(),
    }
}

// ---------------------------------------------------------------------------
// Aircraft photo proxy — planespotters.net
// ---------------------------------------------------------------------------

pub async fn api_photos(
    State(state): State<Arc<AppState>>,
    Path(icao): Path<String>,
) -> impl IntoResponse {
    let icao_upper = icao.to_ascii_uppercase();
    if !is_valid_icao(&icao_upper) {
        return Json(json!({"error": "invalid ICAO hex"})).into_response();
    }

    // Check cache first (cap at 5000 entries to prevent memory exhaustion)
    {
        let cache = state.photo_cache.lock().unwrap();
        if let Some(cached) = cache.get(&icao_upper) {
            return match cached {
                Some(data) => Json(data.clone()).into_response(),
                None => Json(json!({"photos": []})).into_response(),
            };
        }
    }

    // Fetch from planespotters.net
    let url = format!(
        "https://api.planespotters.net/pub/photos/hex/{}",
        icao_upper
    );
    let client = reqwest::Client::new();
    let result = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await;

    let response_data = match result {
        Ok(resp) => (resp.json::<Value>().await).ok(),
        Err(_) => None,
    };

    // Cache the result (even failures, to avoid repeated lookups).
    // Cap at 5000 entries to prevent unbounded memory growth.
    {
        let mut cache = state.photo_cache.lock().unwrap();
        if cache.len() >= 5000 {
            // Evict a random entry (HashMap iter order is arbitrary)
            if let Some(old_key) = cache.keys().next().cloned() {
                cache.remove(&old_key);
            }
        }
        cache.insert(icao_upper, response_data.clone());
    }

    match response_data {
        Some(data) => Json(data).into_response(),
        None => Json(json!({"photos": []})).into_response(),
    }
}

// ---------------------------------------------------------------------------
// Airspace (FAA data)
// ---------------------------------------------------------------------------

const AIRSPACE_CACHE_TTL_SECS: u64 = 900; // 15 minutes

const CLASS_AIRSPACE_URL: &str = "https://services6.arcgis.com/ssFJjBXIUyZDrSYZ/ArcGIS/rest/services/Class_Airspace/FeatureServer/0/query?where=CLASS+IN+(%27B%27,%27C%27,%27D%27)&outFields=IDENT,NAME,CLASS,TYPE_CODE,LOWER_VAL,UPPER_VAL,CITY,STATE&f=geojson&resultRecordCount=5000";

const SPECIAL_USE_AIRSPACE_URL: &str = "https://services6.arcgis.com/ssFJjBXIUyZDrSYZ/ArcGIS/rest/services/Special_Use_Airspace/FeatureServer/0/query?where=TYPE_CODE+IN+(%27R%27,%27P%27,%27A%27,%27W%27)&outFields=NAME,TYPE_CODE,LOWER_VAL,UPPER_VAL,CITY,STATE&f=geojson&resultRecordCount=3000";

/// GET /api/airspace — US airspace boundaries from FAA ArcGIS.
///
/// Returns a merged GeoJSON FeatureCollection of Class B/C/D and
/// special-use airspace (restricted, prohibited, alert, warning).
/// Results are cached for 15 minutes.
pub async fn api_airspace(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Check cache
    {
        let cache = state.airspace_cache.lock().unwrap();
        if let Some((instant, ref data)) = *cache {
            if instant.elapsed().as_secs() < AIRSPACE_CACHE_TTL_SECS {
                return Json(data.clone()).into_response();
            }
        }
    }

    // Fetch both datasets in parallel
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    let (class_result, sua_result) = tokio::join!(
        client.get(CLASS_AIRSPACE_URL).send(),
        client.get(SPECIAL_USE_AIRSPACE_URL).send(),
    );

    let mut all_features: Vec<Value> = Vec::new();

    // Parse Class Airspace
    if let Ok(resp) = class_result {
        if let Ok(geojson) = resp.json::<Value>().await {
            if let Some(features) = geojson.get("features").and_then(|f| f.as_array()) {
                for f in features {
                    let mut feature = f.clone();
                    // Normalize: add "source" property
                    if let Some(props) = feature.get_mut("properties") {
                        if let Some(obj) = props.as_object_mut() {
                            obj.insert("source".to_string(), json!("class"));
                        }
                    }
                    all_features.push(feature);
                }
            }
        }
    }

    // Parse Special Use Airspace
    if let Ok(resp) = sua_result {
        if let Ok(geojson) = resp.json::<Value>().await {
            if let Some(features) = geojson.get("features").and_then(|f| f.as_array()) {
                for f in features {
                    let mut feature = f.clone();
                    if let Some(props) = feature.get_mut("properties") {
                        if let Some(obj) = props.as_object_mut() {
                            obj.insert("source".to_string(), json!("special_use"));
                        }
                    }
                    all_features.push(feature);
                }
            }
        }
    }

    let result = json!({
        "type": "FeatureCollection",
        "features": all_features,
    });

    // Update cache
    {
        let mut cache = state.airspace_cache.lock().unwrap();
        *cache = Some((std::time::Instant::now(), result.clone()));
    }

    Json(result).into_response()
}

// ---------------------------------------------------------------------------
// NLP Query (Ollama)
// ---------------------------------------------------------------------------

const NLP_SYSTEM_PROMPT: &str = r#"You are a filter translator for an aircraft tracking system.
Convert the user's natural language query into a JSON object with these optional fields:
- "min_alt": integer (minimum altitude in feet)
- "max_alt": integer (maximum altitude in feet)
- "icao": string (6-char hex ICAO address, e.g. "A00001")
- "military": boolean (true = military only, false = civilian only)
- "limit": integer (max results, default 5000)

Only include fields that the query mentions. Examples:
- "military jets above flight level 300" → {"military":true,"min_alt":30000}
- "everything below 5000 feet" → {"max_alt":5000}
- "show aircraft A1B2C3" → {"icao":"A1B2C3"}
- "low flying civilian aircraft" → {"max_alt":5000,"military":false}

Respond with ONLY the JSON object, no explanation."#;

#[derive(Deserialize)]
pub struct NlpQueryBody {
    query: String,
}

/// POST /api/nlp-query — natural language aircraft query via Ollama.
pub async fn api_nlp_query(
    State(state): State<Arc<AppState>>,
    Json(body): Json<NlpQueryBody>,
) -> impl IntoResponse {
    // Reject overly long queries to prevent abuse
    if body.query.len() > 500 {
        return Json(json!({"error": "query too long (max 500 characters)"})).into_response();
    }

    let ollama_url = match &state.ollama_url {
        Some(url) => url.clone(),
        None => {
            return Json(json!({
                "error": "Ollama not configured. Start the server with --ollama-url.",
            }))
            .into_response();
        }
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    // Call Ollama chat API
    let chat_url = format!("{}/api/chat", ollama_url.trim_end_matches('/'));
    let ollama_resp = client
        .post(&chat_url)
        .json(&json!({
            "model": "qwen2.5:7b",
            "messages": [
                {"role": "system", "content": NLP_SYSTEM_PROMPT},
                {"role": "user", "content": body.query},
            ],
            "stream": false,
            "format": "json",
        }))
        .send()
        .await;

    let parsed_filters = match ollama_resp {
        Ok(resp) => match resp.json::<Value>().await {
            Ok(data) => {
                let content = data["message"]["content"].as_str().unwrap_or("{}");
                serde_json::from_str::<Value>(content).unwrap_or(json!({}))
            }
            Err(_) => {
                return Json(json!({"error": "Failed to parse Ollama response"})).into_response();
            }
        },
        Err(e) => {
            return Json(json!({
                "error": format!("Ollama request failed: {e}"),
            }))
            .into_response();
        }
    };

    // Extract filter params from LLM output
    let min_alt = parsed_filters["min_alt"].as_i64().map(|v| v as i32);
    let max_alt = parsed_filters["max_alt"].as_i64().map(|v| v as i32);
    let icao = parsed_filters["icao"].as_str().map(|s| s.to_string());
    let military = parsed_filters["military"].as_bool().unwrap_or(false);
    let limit = parsed_filters["limit"]
        .as_i64()
        .map(|v| clamp_i64(v, 1, 50000))
        .unwrap_or(5000);

    // Run the query using existing DB method
    let positions = state
        .db
        .query_positions(min_alt, max_alt, icao.as_deref(), military, limit)
        .await;

    Json(json!({
        "interpretation": parsed_filters,
        "count": positions.len(),
        "positions": positions,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// Vessel (AIS) endpoints
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct VesselParams {
    limit: Option<i64>,
}

#[derive(Deserialize)]
pub struct VesselPositionParams {
    minutes: Option<f64>,
    limit: Option<i64>,
}

pub async fn api_vessels(
    State(state): State<Arc<AppState>>,
    Query(params): Query<VesselParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(100).min(1000);
    let vessels = state.db.get_vessels(limit).await;
    Json(json!(vessels))
}

pub async fn api_vessel_positions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<VesselPositionParams>,
) -> impl IntoResponse {
    let minutes = params.minutes.unwrap_or(60.0).clamp(1.0, 525600.0);
    let limit = params.limit.unwrap_or(10000).min(50000);
    let positions = state.db.get_vessel_positions(minutes, limit).await;
    Json(json!(positions))
}

pub async fn api_vessel_positions_latest(
    State(state): State<Arc<AppState>>,
    Query(params): Query<VesselParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(200).min(1000);
    let positions = state.db.get_recent_vessel_positions(limit).await;
    Json(json!(positions))
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
        db.add_position(
            &icao,
            52.25,
            3.92,
            Some(38000),
            Some(450.0),
            Some(90.0),
            None,
            None,
            1.0,
        );
        db.add_event(
            &icao,
            "military",
            "Test event",
            Some(52.25),
            Some(3.92),
            Some(38000),
            1.0,
        );
        drop(db);

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

    #[tokio::test]
    async fn test_api_stats() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/stats")
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
        assert_eq!(json["aircraft"], 1);
        assert_eq!(json["positions"], 1);
        // feed_age_seconds is the freshness signal that lets external monitors
        // distinguish "API up but feeder dead" from "everything green."
        assert!(
            json.get("feed_age_seconds").is_some(),
            "/api/stats must expose feed_age_seconds"
        );
        assert!(
            json["feed_age_seconds"].as_f64().is_some(),
            "feed_age_seconds should be a number when positions exist, got {}",
            json["feed_age_seconds"]
        );
    }

    #[tokio::test]
    async fn test_api_aircraft() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

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
        assert!(!json["aircraft"].as_array().unwrap().is_empty());
        assert!(json["total"].as_u64().unwrap() >= 1);
    }

    #[tokio::test]
    async fn test_api_aircraft_detail() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

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
        let app = crate::web::build_router(state, None);

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
        let app = crate::web::build_router(state, None);

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
        assert!(!json.as_array().unwrap().is_empty());
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
            auth_token: None,

            photo_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
            airspace_cache: std::sync::Mutex::new(None),
            ollama_url: None,
        });

        // Create geofence
        let app = crate::web::build_router(state.clone(), None);
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
        let app = crate::web::build_router(state.clone(), None);
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
        let app = crate::web::build_router(state.clone(), None);
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
        let app = crate::web::build_router(
            Arc::new(AppState {
                db: Arc::new(SqliteDb::new(db_path)),
                tracker: None,
                geofences: RwLock::new(Vec::new()),
                geofence_next_id: RwLock::new(1),
                auth_token: None,

                photo_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
                airspace_cache: std::sync::Mutex::new(None),
                ollama_url: None,
            }),
            None,
        );

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
        let airports = json.as_array().unwrap();
        assert!(
            airports.len() > 3600,
            "Expected 3600+ airports, got {}",
            airports.len()
        );
        // Verify types are normalized
        let types: Vec<&str> = airports.iter().filter_map(|a| a["type"].as_str()).collect();
        assert!(types.contains(&"major"));
        assert!(types.contains(&"medium"));
        assert!(types.contains(&"small"));
    }

    #[tokio::test]
    async fn test_api_positions_clamped() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

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
        let app = crate::web::build_router(state, None);

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
        let app = crate::web::build_router(state, None);

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

    #[tokio::test]
    async fn test_page_routes() {
        let (state, _dir) = test_state();

        let pages = [
            "/",
            "/table",
            "/stats",
            "/events",
            "/query",
            "/replay",
            "/receivers",
        ];
        for page in pages {
            let app = crate::web::build_router(state.clone(), None);
            let response = app
                .oneshot(Request::builder().uri(page).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "Page {page} failed");

            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            let html = String::from_utf8_lossy(&body);
            assert!(html.contains("adsb-decode"), "Page {page} missing brand");
            assert!(html.contains("<nav>"), "Page {page} missing nav");
        }
    }

    #[tokio::test]
    async fn test_page_detail() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/aircraft/4840D6")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("detail-split"));
    }

    #[tokio::test]
    async fn test_api_photos_returns_json() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        // Requesting photos for a non-existent ICAO should return valid JSON
        // (either from the external API or a fallback empty response)
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/photos/000000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        // Should be valid JSON regardless of external API response
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json.is_object());
    }

    #[tokio::test]
    async fn test_api_photos_cache() {
        let (state, _dir) = test_state();

        // Pre-populate the cache
        {
            let mut cache = state.photo_cache.lock().unwrap();
            cache.insert(
                "AAAAAA".to_string(),
                Some(serde_json::json!({
                    "photos": [{
                        "id": "12345",
                        "thumbnail": {"src": "https://example.com/photo.jpg"},
                        "photographer": "Test Photographer",
                        "link": "https://example.com/photo"
                    }]
                })),
            );
        }

        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/photos/AAAAAA")
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
        let photos = json["photos"].as_array().unwrap();
        assert_eq!(photos.len(), 1);
        assert_eq!(
            photos[0]["photographer"].as_str().unwrap(),
            "Test Photographer"
        );
    }

    #[tokio::test]
    async fn test_api_photos_cache_miss_stored() {
        let (state, _dir) = test_state();

        // Pre-populate cache with None (failed lookup)
        {
            let mut cache = state.photo_cache.lock().unwrap();
            cache.insert("BBBBBB".to_string(), None);
        }

        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/photos/BBBBBB")
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
        let photos = json["photos"].as_array().unwrap();
        assert!(photos.is_empty());
    }

    #[tokio::test]
    async fn test_api_positions_all_no_range() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/positions/all")
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
        assert!(json.is_array());
    }

    #[tokio::test]
    async fn test_api_positions_all_with_time_range() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/positions/all?start=1000&end=2000")
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
        assert!(json.is_array());
    }

    #[tokio::test]
    async fn test_api_positions_all_start_only() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/positions/all?start=1000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_positions_all_end_only() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/positions/all?end=2000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_photos_icao_case_normalized() {
        let (state, _dir) = test_state();

        // Cache with uppercase key
        {
            let mut cache = state.photo_cache.lock().unwrap();
            cache.insert(
                "CCCCCC".to_string(),
                Some(serde_json::json!({"photos": [{"id": "test"}]})),
            );
        }

        let app = crate::web::build_router(state, None);

        // Request with lowercase — should match the cached uppercase entry
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/photos/cccccc")
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
        let photos = json["photos"].as_array().unwrap();
        assert_eq!(photos.len(), 1);
    }

    #[tokio::test]
    async fn test_api_airspace_returns_geojson() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/airspace")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["type"].as_str().unwrap(), "FeatureCollection");
        assert!(json["features"].is_array());
    }

    #[tokio::test]
    async fn test_api_airspace_caches_result() {
        let (state, _dir) = test_state();

        // Pre-populate the cache
        {
            let mut cache = state.airspace_cache.lock().unwrap();
            *cache = Some((
                std::time::Instant::now(),
                json!({
                    "type": "FeatureCollection",
                    "features": [{"type": "Feature", "properties": {"CLASS": "B"}, "geometry": null}],
                }),
            ));
        }

        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/airspace")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        // Should return cached data with 1 feature
        assert_eq!(json["features"].as_array().unwrap().len(), 1);
        assert_eq!(
            json["features"][0]["properties"]["CLASS"].as_str().unwrap(),
            "B"
        );
    }

    #[tokio::test]
    async fn test_api_nlp_query_no_ollama() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/nlp-query")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"query":"military aircraft"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        // Should return error since no Ollama is configured
        assert!(json["error"].as_str().unwrap().contains("not configured"));
    }

    #[tokio::test]
    async fn test_api_nlp_query_invalid_body() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/nlp-query")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"bad":"field"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Should fail deserialization (422 from axum)
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    fn test_state_with_vessels() -> (Arc<AppState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_str().unwrap().to_string();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let mut db = Database::open(&db_path).unwrap();
        db.upsert_vessel(
            "367000001",
            Some("TEST SHIP"),
            Some("Cargo"),
            Some("US"),
            now,
        );
        db.add_vessel_position("367000001", 32.5, -79.8, Some(12.0), Some(180.0), None, now);
        db.upsert_vessel(
            "367000002",
            Some("SECOND SHIP"),
            Some("Tanker"),
            Some("MH"),
            now + 1.0,
        );
        db.add_vessel_position(
            "367000002",
            33.0,
            -78.5,
            Some(8.0),
            Some(90.0),
            None,
            now + 1.0,
        );
        drop(db);

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

    #[tokio::test]
    async fn test_api_vessels() {
        let (state, _dir) = test_state_with_vessels();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/vessels")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 2);
        // Should contain both vessels
        let mmsis: Vec<&str> = json.iter().map(|v| v["mmsi"].as_str().unwrap()).collect();
        assert!(mmsis.contains(&"367000001"));
        assert!(mmsis.contains(&"367000002"));
    }

    #[tokio::test]
    async fn test_api_vessel_positions_latest() {
        let (state, _dir) = test_state_with_vessels();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/vessel-positions/latest")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 2);
        // Check position data
        let first = &json[0];
        assert!(first["lat"].as_f64().is_some());
        assert!(first["lon"].as_f64().is_some());
    }

    #[tokio::test]
    async fn test_api_vessels_empty() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/vessels")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 0);
    }

    #[tokio::test]
    async fn test_api_trails() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/trails?minutes=1440")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        // Response wraps trails in {"trails": {...}, "count": N}
        assert!(json["trails"].is_object(), "trails key should be an object");
        assert!(json["count"].is_u64(), "count key should be a number");
    }

    #[tokio::test]
    async fn test_api_trails_contains_position_data() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/trails?minutes=1440")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let trails = &json["trails"];
        // Test data has one aircraft (4840D6) with one position
        if let Some(points) = trails["4840D6"].as_array() {
            assert!(!points.is_empty());
            let point = &points[0];
            assert!(point["lat"].as_f64().is_some());
            assert!(point["lon"].as_f64().is_some());
            assert!(point["altitude_ft"].is_number());
            assert!(point["timestamp"].is_number());
        }
    }

    #[tokio::test]
    async fn test_api_trails_minutes_clamped() {
        let (state, _dir) = test_state();
        // Request with absurdly high minutes — should be clamped, not error
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/trails?minutes=999999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_vessel_positions() {
        let (state, _dir) = test_state_with_vessels();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/vessel-positions?minutes=1440")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 2);
        // Verify position fields
        let pos = &json[0];
        assert!(pos["mmsi"].as_str().is_some());
        assert!(pos["lat"].as_f64().is_some());
        assert!(pos["lon"].as_f64().is_some());
        assert!(pos["speed_kts"].as_f64().is_some());
    }

    #[tokio::test]
    async fn test_api_vessel_positions_empty() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/vessel-positions?minutes=60")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 0);
    }

    #[tokio::test]
    async fn test_api_vessels_with_limit() {
        let (state, _dir) = test_state_with_vessels();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/vessels?limit=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 1);
    }

    #[tokio::test]
    async fn test_api_vessel_data_fields() {
        let (state, _dir) = test_state_with_vessels();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/vessels")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<Value> = serde_json::from_slice(&body).unwrap();
        let vessel = &json[0];
        // Verify all expected fields
        assert!(vessel["mmsi"].as_str().is_some());
        assert!(vessel["name"].as_str().is_some());
        assert!(vessel["vessel_type"].as_str().is_some());
        assert!(vessel["flag"].as_str().is_some());
    }

    // -----------------------------------------------------------------------
    // Aircraft pagination tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_api_aircraft_pagination_response_format() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/aircraft")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        // Paginated response must have aircraft array, total, offset, limit
        assert!(json["aircraft"].is_array());
        assert!(json["total"].is_u64());
        assert!(json["offset"].is_u64());
        assert!(json["limit"].is_u64());
    }

    #[tokio::test]
    async fn test_api_aircraft_pagination_with_limit() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/aircraft?limit=1&offset=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["aircraft"].as_array().unwrap().len(), 1);
        assert_eq!(json["total"].as_u64().unwrap(), 1);
        assert_eq!(json["offset"].as_u64().unwrap(), 0);
        assert_eq!(json["limit"].as_u64().unwrap(), 1);
    }

    #[tokio::test]
    async fn test_api_aircraft_pagination_offset_beyond_total() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        // Offset past all records — should return empty page but still report total
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/aircraft?offset=999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["aircraft"].as_array().unwrap().len(), 0);
        assert_eq!(json["total"].as_u64().unwrap(), 1); // Still 1 in DB
    }

    #[tokio::test]
    async fn test_api_aircraft_military_filter_with_pagination() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        // Test data has no military aircraft
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/aircraft?military=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["aircraft"].as_array().unwrap().len(), 0);
        assert_eq!(json["total"].as_u64().unwrap(), 0);
    }

    // -----------------------------------------------------------------------
    // Heatmap tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_api_heatmap_returns_array() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/heatmap?minutes=60")
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
        assert!(json.is_array(), "Heatmap should return an array of cells");
    }

    #[tokio::test]
    async fn test_api_heatmap_minutes_clamped() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        // Absurdly high minutes — should be clamped, not error
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/heatmap?minutes=999999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}

// ---------------------------------------------------------------------------
// Consumer contract regression tests
//
// These tests pin the JSON shape of every endpoint that downstream Blue Octopus
// projects consume. Changing a shape here without coordinating with the listed
// consumer is what shipped the 2026-04-28 envelope-vs-bare-array bug.
//
// Downstream consumers (UtilitarianTechnology repo):
//   - orin/scripts/adsb_correlator.py        — ADSBClient: positions, query, aircraft, vessels
//   - orin/scripts/ais_correlator.py         — same client, vessel methods only
//   - ardupilot/adsb_bridge.py               — /api/positions for SITL injection
//   - ardupilot/telemetry_logger.py          — /api/positions, /api/vessel-positions/latest, /api/vessels, /api/events
//   - tools/demo_console.py                  — same as telemetry_logger
//
// Operations:
//   - deploy/adsb-decode-healthcheck.sh      — /api/stats (feed_age_seconds)
//
// Touching a shape pinned below = breaks one of those consumers. Coordinate the
// rollout (server change + consumer update + run consumers' tests) before merge.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod consumer_contract_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::RwLock;
    use tower::ServiceExt;

    use crate::db::{Database, SqliteDb};

    /// Seed enough data that every consumer-facing field has a non-trivial value.
    /// Distinct from the basic `test_state()` above: that one leaves military/registration
    /// at defaults, which would let a serializer regression where those fields drop
    /// silently slip past null-tolerant assertions.
    fn contract_state() -> (Arc<AppState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_str().unwrap().to_string();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();

        let mut db = Database::open(&db_path).unwrap();

        // Aircraft + position with EVERY field populated so we can verify each
        // serialized key reaches the wire with a real (non-null) value.
        let icao = adsb_core::types::icao_from_hex("ADF123").unwrap();
        db.upsert_aircraft(&icao, Some("United States"), Some("N12345"), true, now);
        db.add_position(
            &icao,
            35.18,
            -83.33,
            Some(35000),
            Some(450.0),
            Some(270.0),
            Some(-1500),
            None,
            now,
        );
        // Sighting so HistoryRow joins yield a callsign for the /api/positions DB-fallback path.
        db.upsert_sighting(&icao, None, Some("AAL2127"), None, Some(35000), now);

        // Vessel + position with full metadata for /api/vessels + /api/vessel-positions*.
        db.upsert_vessel(
            "367000099",
            Some("CONTRACT TEST"),
            Some("Cargo"),
            Some("US"),
            now,
        );
        db.add_vessel_position(
            "367000099",
            32.5,
            -79.8,
            Some(12.5),
            Some(180.0),
            Some(175.0),
            now,
        );
        drop(db);

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

    async fn get_json(state: Arc<AppState>, uri: &str) -> Value {
        let app = crate::web::build_router(state, None);
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri} returned non-200");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap_or_else(|e| panic!("{uri}: bad JSON: {e}"))
    }

    /// Assert every named key is present (may be null — that's a valid value).
    /// "Present" beats "non-null" as a contract because Option::None still keeps
    /// the key in the JSON object via serde, and the consumer reads with .get().
    fn assert_keys_present(item: &Value, keys: &[&str], context: &str) {
        let obj = item
            .as_object()
            .unwrap_or_else(|| panic!("{context}: item is not an object: {item}"));
        for key in keys {
            assert!(
                obj.contains_key(*key),
                "{context}: missing key {key:?}. Got keys: {:?}",
                obj.keys().collect::<Vec<_>>()
            );
        }
    }

    // -----------------------------------------------------------------------
    // /api/positions  (consumer: adsb_correlator.ADSBClient.positions_within_minutes,
    //                            adsb_bridge.py, telemetry_logger.py, demo_console.py)
    //
    // Contract: bare top-level JSON array. Each item carries the live-tracker
    // enrichment fields (callsign/registration/country/is_military) — these come
    // from the live-tracker path lines 213-226 and the DB-fallback path lines 249-262.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn contract_api_positions_is_bare_array() {
        let (state, _dir) = contract_state();
        let json = get_json(state, "/api/positions?minutes=1440").await;
        assert!(
            json.is_array(),
            "/api/positions must return a bare JSON array, not an envelope. \
             The 2026-04-28 correlator bug was caused by a consumer mocking \
             {{positions: [...]}} when this endpoint actually returns [...]. \
             Got: {json}"
        );
        let arr = json.as_array().unwrap();
        assert!(
            !arr.is_empty(),
            "/api/positions: contract_state seeded a position"
        );
        assert_keys_present(
            &arr[0],
            &[
                "icao",
                "lat",
                "lon",
                "altitude_ft",
                "speed_kts",
                "heading_deg",
                "vertical_rate_fpm",
                "timestamp",
                "callsign",
                "registration",
                "country",
                "is_military",
            ],
            "/api/positions item",
        );
    }

    // -----------------------------------------------------------------------
    // /api/positions/all  (consumer: adsb_correlator.ADSBClient.positions_in_window)
    //
    // Contract: bare array. Used for post-flight replay correlation against
    // TimescaleDB. PositionRow now includes aircraft enrichment fields
    // (callsign/registration/country/is_military) populated via JOIN — without
    // them the correlator's military discrimination silently defaulted to false
    // on historical replay queries. See db.rs:836 + db.rs:get_all_positions_ordered.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn contract_api_positions_all_is_bare_array() {
        let (state, _dir) = contract_state();
        let json = get_json(state, "/api/positions/all").await;
        assert!(
            json.is_array(),
            "/api/positions/all must return a bare JSON array. Got: {json}"
        );
        let arr = json.as_array().unwrap();
        assert!(
            !arr.is_empty(),
            "/api/positions/all: contract_state seeded a position"
        );
        assert_keys_present(
            &arr[0],
            &[
                "icao",
                "lat",
                "lon",
                "altitude_ft",
                "speed_kts",
                "heading_deg",
                "vertical_rate_fpm",
                "timestamp",
                // Enrichment — these were added 2026-05-05 to fix the historical-replay
                // military-discrimination silent-default bug. If a future schema change
                // drops them, the correlator goes back to thinking nothing is military.
                "callsign",
                "registration",
                "country",
                "is_military",
            ],
            "/api/positions/all item",
        );
    }

    /// The 2026-05-05 enrichment is load-bearing — assert the seeded military aircraft
    /// actually surfaces as `is_military: true` and the registration/callsign reach the
    /// wire. A regression to bare positions (dropping the JOIN) would set is_military:
    /// null on this seeded military row, which is the correlator-breaking condition.
    #[tokio::test]
    async fn contract_api_positions_all_enrichment_is_populated() {
        let (state, _dir) = contract_state();
        let json = get_json(state, "/api/positions/all").await;
        let arr = json.as_array().unwrap();
        let row = arr.iter().find(|r| r["icao"] == "ADF123").expect(
            "seeded military aircraft ADF123 missing from /api/positions/all — JOIN broken?",
        );
        assert_eq!(
            row["is_military"], true,
            "seeded ADF123 is_military=true but endpoint returned {}. \
             Check the LEFT JOIN aircraft a ON p.icao = a.icao path.",
            row["is_military"]
        );
        assert_eq!(
            row["registration"], "N12345",
            "registration enrichment dropped: {}",
            row["registration"]
        );
        assert_eq!(
            row["callsign"], "AAL2127",
            "callsign enrichment dropped (latest sighting JOIN broken?): {}",
            row["callsign"]
        );
    }

    // -----------------------------------------------------------------------
    // /api/query  (consumer: adsb_correlator.ADSBClient.query)
    //
    // Contract: bare array of PositionRow. Same field shape as /api/positions/all.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn contract_api_query_is_bare_array() {
        let (state, _dir) = contract_state();
        let json = get_json(state, "/api/query?limit=10").await;
        assert!(
            json.is_array(),
            "/api/query must return a bare JSON array, not an envelope. Got: {json}"
        );
        let arr = json.as_array().unwrap();
        assert!(
            !arr.is_empty(),
            "/api/query: contract_state seeded a position"
        );
        assert_keys_present(
            &arr[0],
            &[
                "icao",
                "lat",
                "lon",
                "altitude_ft",
                "speed_kts",
                "heading_deg",
                "timestamp",
                "callsign",
                "registration",
                "country",
                "is_military",
            ],
            "/api/query item",
        );
    }

    /// Mirror of `contract_api_positions_all_enrichment_is_populated` for /api/query.
    /// Same JOIN code path, separate query function — needs its own assertion.
    #[tokio::test]
    async fn contract_api_query_enrichment_is_populated() {
        let (state, _dir) = contract_state();
        let json = get_json(state, "/api/query?limit=10").await;
        let row = json
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["icao"] == "ADF123")
            .expect("seeded military aircraft missing from /api/query — JOIN broken?");
        assert_eq!(row["is_military"], true);
        assert_eq!(row["registration"], "N12345");
        assert_eq!(row["callsign"], "AAL2127");
    }

    // -----------------------------------------------------------------------
    // /api/aircraft/<icao>  (consumer: adsb_correlator.ADSBClient.aircraft)
    //
    // Contract: object with `aircraft` envelope. Note this is one of the few
    // endpoints that DOES use an envelope — different from the bare-array
    // family above. Don't normalize without coordinating with the consumer.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn contract_api_aircraft_detail_uses_envelope() {
        let (state, _dir) = contract_state();
        let json = get_json(state, "/api/aircraft/ADF123").await;
        assert!(
            json.is_object(),
            "/api/aircraft/<icao> uses an envelope object (not bare). Got: {json}"
        );
        assert!(
            json.get("aircraft").is_some(),
            "/api/aircraft/<icao> must have an `aircraft` key"
        );
        let ac = &json["aircraft"];
        assert_keys_present(
            ac,
            &["icao", "registration", "country", "is_military"],
            "/api/aircraft/<icao> aircraft object",
        );
    }

    // -----------------------------------------------------------------------
    // /api/vessels  (consumer: ais_correlator.ADSBClient.vessels,
    //                          telemetry_logger.py, demo_console.py)
    //
    // Contract: bare array of vessel metadata.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn contract_api_vessels_is_bare_array() {
        let (state, _dir) = contract_state();
        let json = get_json(state, "/api/vessels").await;
        assert!(
            json.is_array(),
            "/api/vessels must return a bare array. Got: {json}"
        );
        let arr = json.as_array().unwrap();
        assert!(
            !arr.is_empty(),
            "/api/vessels: contract_state seeded a vessel"
        );
        assert_keys_present(
            &arr[0],
            &[
                "mmsi",
                "name",
                "vessel_type",
                "flag",
                "first_seen",
                "last_seen",
            ],
            "/api/vessels item",
        );
    }

    // -----------------------------------------------------------------------
    // /api/vessel-positions  (consumer: ais_correlator.ADSBClient.vessel_positions_within_minutes,
    //                                   used by vessel_positions_in_window for replay correlation)
    //
    // Contract: bare array of vessel position rows.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn contract_api_vessel_positions_is_bare_array() {
        let (state, _dir) = contract_state();
        let json = get_json(state, "/api/vessel-positions?minutes=1440").await;
        assert!(
            json.is_array(),
            "/api/vessel-positions must return a bare array. Got: {json}"
        );
        let arr = json.as_array().unwrap();
        assert!(
            !arr.is_empty(),
            "/api/vessel-positions: contract_state seeded a position"
        );
        assert_keys_present(
            &arr[0],
            &[
                "mmsi",
                "lat",
                "lon",
                "speed_kts",
                "course_deg",
                "heading_deg",
                "timestamp",
            ],
            "/api/vessel-positions item",
        );
    }

    // -----------------------------------------------------------------------
    // /api/vessel-positions/latest  (consumer: ais_correlator.ADSBClient.vessel_positions_latest,
    //                                          telemetry_logger.py, demo_console.py)
    //
    // Contract: bare array. Note the URL: hyphen, not underscore. The
    // 2026-04-28 doc-vs-code mismatch had docs referencing
    // /api/vessel_positions_latest (underscore), which doesn't exist.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn contract_api_vessel_positions_latest_is_bare_array() {
        let (state, _dir) = contract_state();
        let json = get_json(state, "/api/vessel-positions/latest").await;
        assert!(
            json.is_array(),
            "/api/vessel-positions/latest must return a bare array. Got: {json}"
        );
        let arr = json.as_array().unwrap();
        assert!(
            !arr.is_empty(),
            "/api/vessel-positions/latest: contract_state seeded a position"
        );
        assert_keys_present(
            &arr[0],
            &[
                "mmsi",
                "lat",
                "lon",
                "speed_kts",
                "course_deg",
                "heading_deg",
                "timestamp",
            ],
            "/api/vessel-positions/latest item",
        );
    }

    // -----------------------------------------------------------------------
    // /api/stats  (consumer: deploy/adsb-decode-healthcheck.sh — external timer
    //              that restarts the service if /api/stats stops returning 200,
    //              and an external monitor that reads feed_age_seconds to detect
    //              "API up but feeder dead" — the failure that took 27h to surface
    //              on May 4 2026)
    //
    // Contract: object (not array). Must expose feed_age_seconds.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn contract_api_stats_exposes_feed_age_seconds() {
        let (state, _dir) = contract_state();
        let json = get_json(state, "/api/stats").await;
        assert!(
            json.is_object(),
            "/api/stats must be an object. Got: {json}"
        );
        assert_keys_present(
            &json,
            &[
                "aircraft",
                "positions",
                "events",
                "receivers",
                "captures",
                "feed_age_seconds",
            ],
            "/api/stats",
        );
    }
}

// ---------------------------------------------------------------------------
// Per-page deep tests
//
// `test_page_routes` (in the main tests module) is a 7-page smoke test that
// only asserts status==OK + brand string + nav. The tests below validate the
// content-types, mime headers, and key DOM/markup features that downstream
// consumers (browsers, search engines, AI scrapers) rely on. Each test names
// the specific contract it's pinning so a future drop catches the right thing.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod pages_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::RwLock;
    use tower::ServiceExt;

    use crate::db::SqliteDb;

    fn empty_state() -> (Arc<AppState>, tempfile::TempDir) {
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

    /// Helper: GET a URL and return (status, content-type, body-as-string).
    async fn fetch(state: Arc<AppState>, uri: &str) -> (StatusCode, String, String) {
        let app = crate::web::build_router(state, None);
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8_lossy(&body).into_owned();
        (status, ct, body_str)
    }

    /// /robots.txt — content-type text/plain, references the sitemap.
    /// Crawlers won't find /sitemap.xml without this. Pinned because content-type
    /// drift (text/html) makes Google ignore the file silently.
    #[tokio::test]
    async fn page_robots_txt_is_plaintext_with_sitemap_ref() {
        let (state, _dir) = empty_state();
        let (status, ct, body) = fetch(state, "/robots.txt").await;
        assert_eq!(status, StatusCode::OK);
        assert!(ct.contains("text/plain"), "robots.txt content-type {ct:?}");
        assert!(
            body.contains("Sitemap:"),
            "robots.txt missing Sitemap directive"
        );
        assert!(
            body.contains("/sitemap.xml"),
            "robots.txt sitemap path missing"
        );
    }

    /// /sitemap.xml — XML mime, contains every page handler we have.
    /// A new page added without updating sitemap_xml() is invisible to crawlers.
    /// This test enumerates the known pages so adding a new one fires the test.
    #[tokio::test]
    async fn page_sitemap_xml_lists_every_indexable_page() {
        let (state, _dir) = empty_state();
        let (status, ct, body) = fetch(state, "/sitemap.xml").await;
        assert_eq!(status, StatusCode::OK);
        assert!(ct.contains("xml"), "sitemap.xml content-type {ct:?}");

        // Every public page handler should be in the sitemap. Update both this
        // list and pages::sitemap_xml() when adding a new top-level page.
        let expected_paths = [
            "/",
            "/about",
            "/how-it-works",
            "/features",
            "/setup",
            "/table",
            "/events",
            "/query",
            "/replay",
            "/receivers",
            "/stats",
            "/register",
        ];
        for path in expected_paths {
            let needle = if path == "/" {
                "<loc>https://adsb.blueoctopustechnology.com/</loc>".to_string()
            } else {
                format!("<loc>https://adsb.blueoctopustechnology.com{path}</loc>")
            };
            assert!(
                body.contains(&needle),
                "sitemap.xml missing entry for {path}. Body: {body}"
            );
        }
    }

    /// /llms.txt — AI-discovery convention. Pinned because it's the doc that
    /// LLMs scrape to learn about the project, and we list real API endpoint
    /// paths in there. If those drift, AI agents call dead endpoints.
    #[tokio::test]
    async fn page_llms_txt_lists_real_api_endpoints() {
        let (state, _dir) = empty_state();
        let (status, ct, body) = fetch(state, "/llms.txt").await;
        assert_eq!(status, StatusCode::OK);
        assert!(ct.contains("text/plain"), "llms.txt content-type {ct:?}");
        // Project framing
        assert!(body.contains("ADS-B"), "llms.txt should mention ADS-B");
        assert!(body.contains("AIS"), "llms.txt should mention AIS");
        // Real API endpoints — keep this list in sync with the routing in mod.rs.
        // If an endpoint is removed and llms.txt isn't updated, agents get 404s.
        for endpoint in [
            "/api/positions",
            "/api/aircraft",
            "/api/stats",
            "/api/vessels",
        ] {
            assert!(
                body.contains(endpoint),
                "llms.txt missing real endpoint {endpoint}. AI agents will call dead URLs."
            );
        }
    }

    /// /register — Pi receiver registration form.
    /// Recent (2026-05-05) — server-side coord validation was added in commit bfad1ac.
    /// The form needs lat/lon inputs for the validation path to be reachable.
    #[tokio::test]
    async fn page_register_has_required_form_fields() {
        let (state, _dir) = empty_state();
        let (status, _ct, body) = fetch(state, "/register").await;
        assert_eq!(status, StatusCode::OK);
        // The form needs name + lat + lon to feed POST /api/register.
        // Markup-pattern checks (not exact string) — tolerates HTML reformatting.
        let lower = body.to_lowercase();
        assert!(
            lower.contains("name=\"name\"") || lower.contains("id=\"name\""),
            "/register form missing name input"
        );
        assert!(
            lower.contains("name=\"lat\"") || lower.contains("id=\"lat\""),
            "/register form missing lat input"
        );
        assert!(
            lower.contains("name=\"lon\"") || lower.contains("id=\"lon\""),
            "/register form missing lon input"
        );
    }

    /// /stats — system stats page. Should pull from /api/stats. Pinned because
    /// `feed_age_seconds` is the field external monitors read; if the page ever
    /// embeds a JSON shape that doesn't match, the dashboard goes silent.
    #[tokio::test]
    async fn page_stats_references_stats_api() {
        let (state, _dir) = empty_state();
        let (status, _ct, body) = fetch(state, "/stats").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.contains("/api/stats"),
            "/stats page must fetch /api/stats — JS embedding broken?"
        );
    }

    /// /aircraft/<icao> — detail page. Has split-screen layout with the trail
    /// map on the left and external intel cards (hexdb.io, Planespotters, etc.)
    /// on the right. Pinned so a layout refactor doesn't silently drop the
    /// external links the value-add of this page depends on.
    #[tokio::test]
    async fn page_aircraft_detail_has_external_intel_links() {
        let (state, _dir) = empty_state();
        let (status, _ct, body) = fetch(state, "/aircraft/A4CA5F").await;
        assert_eq!(status, StatusCode::OK);
        // Split-screen marker — exists today as detail-split class.
        assert!(
            body.contains("detail-split"),
            "/aircraft/<icao>: detail-split layout marker dropped"
        );
        // The page ships at least one external aviation database link as the
        // RE-driven value-add. Pin presence of any one well-known service.
        let has_external = [
            "adsbexchange",
            "planespotters",
            "flightaware",
            "flightradar",
        ]
        .iter()
        .any(|svc| body.to_lowercase().contains(svc));
        assert!(
            has_external,
            "/aircraft/<icao>: lost all external aviation-database links"
        );
    }

    /// /og-image.png — Open Graph share image. Served as SVG with image/svg+xml
    /// content-type. Pinned because Twitter/Facebook crawlers ignore wrong mime.
    #[tokio::test]
    async fn page_og_image_has_image_content_type() {
        let (state, _dir) = empty_state();
        let (status, ct, _body) = fetch(state, "/og-image.png").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            ct.starts_with("image/"),
            "/og-image.png content-type must start with image/. Got {ct:?}"
        );
    }

    /// Home page (`/`) — the map. Pinned because the embedded JS that drives
    /// every overlay (heatmap, vessels, weather, etc.) lives in this template.
    /// A `<script>` tag mismatch in the template silently breaks the whole map;
    /// substring checks in isolation aren't enough to catch that.
    #[tokio::test]
    async fn page_map_renders_with_balanced_script_tags() {
        let (state, _dir) = empty_state();
        let (status, _ct, body) = fetch(state, "/").await;
        assert_eq!(status, StatusCode::OK);
        let opens = body.matches("<script").count();
        let closes = body.matches("</script>").count();
        assert_eq!(
            opens, closes,
            "map.html script tags unbalanced: {opens} opens vs {closes} closes"
        );
    }

    /// Weather radar toggle (B235). Pinned because RainViewer is the only
    /// integration point with a non-Anthropic external service the dashboard
    /// has, and the JS hook names are the contract between the toggle and
    /// the implementation.
    #[tokio::test]
    async fn page_map_has_weather_toggle_and_handlers() {
        let (state, _dir) = empty_state();
        let (status, _ct, body) = fetch(state, "/").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.contains("id=\"weather-toggle\""),
            "weather toggle checkbox missing"
        );
        for needle in ["enableWeather", "disableWeather", "buildWeatherTileUrl"] {
            assert!(
                body.contains(needle),
                "weather JS function {needle} missing from map.html"
            );
        }
        assert!(
            body.contains("api.rainviewer.com"),
            "weather toggle present but RainViewer manifest URL missing"
        );
    }

    /// /replay — historical playback. Pinned because the page renders inline JS
    /// for both Leaflet (2D) and CesiumJS (3D, lazy-loaded). A `<script>` tag
    /// mismatch breaks both renderers; the 3D toggle name + lazy-loader are the
    /// contract between user UI and the entity update path.
    #[tokio::test]
    async fn page_replay_renders_with_balanced_script_tags() {
        let (state, _dir) = empty_state();
        let (status, _ct, body) = fetch(state, "/replay").await;
        assert_eq!(status, StatusCode::OK);
        let opens = body.matches("<script").count();
        let closes = body.matches("</script>").count();
        assert_eq!(
            opens, closes,
            "replay.html script tags unbalanced: {opens} opens vs {closes} closes"
        );
    }

    /// 4D replay (B209). Lazy-loads CesiumJS only when the 3D button is clicked
    /// — the default 2D path must NOT pull the ~3MB Cesium bundle.
    #[tokio::test]
    async fn page_replay_has_3d_toggle_with_lazy_cesium_load() {
        let (state, _dir) = empty_state();
        let (status, _ct, body) = fetch(state, "/replay").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.contains("id=\"mode-3d-btn\""),
            "replay 3D toggle button missing"
        );
        for needle in [
            "loadCesiumJS",
            "initReplayCesium",
            "updateDisplay3D",
            "clearCesium3DEntities",
        ] {
            assert!(
                body.contains(needle),
                "replay 3D function {needle} missing — 4D replay broken"
            );
        }
        // Cesium URL should appear inside the lazy-loader, not as a top-level
        // <script src=> tag — that's what makes the load lazy. Pin the lazy
        // shape by asserting NO <script src=...cesium...> element.
        let cesium_top_level_script = body.contains("<script src=\"https://cdn.jsdelivr.net/npm/cesium")
            || body.contains("<script src=\"https://unpkg.com/cesium");
        assert!(
            !cesium_top_level_script,
            "Cesium loaded as top-level <script src=> — should be lazy via loadCesiumJS()"
        );
    }

    /// `/api/airports` is consumed by the map page JS to draw airport overlays.
    /// Bare-array contract (not envelope) — pinned to prevent the same shape
    /// regression that bit /api/positions on 2026-04-28.
    #[tokio::test]
    async fn page_airports_endpoint_returns_bare_array() {
        let (state, _dir) = empty_state();
        let app = crate::web::build_router(state, None);
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
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json.is_array(),
            "/api/airports must be a bare array (map JS expects this). Got: {json}"
        );
    }
}
