//! Multi-receiver ingest API — feeders POST frames here.
//!
//! Each feeder identifies by name. The server maintains a tracker per
//! feeder and merges decoded data into the shared database via the
//! AdsbDatabase trait.
//!
//! **Raw sink tee (Phase-0 A2):** When `ADSB_RAW_SINK_DIR` is set, every
//! decoded ADS-B position is also appended to a dated NDJSON file under
//! that directory (`<dir>/adsb/YYYY-MM-DD/adsb-YYYY-MM-DD.ndjson`). This is
//! the durable off-box record that survives DB compaction and server restarts.
//! The sink initialises once at startup via `OnceLock`; when the env var is
//! absent the tee is a no-op and the ingest path is unchanged.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
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

use crate::raw_sink::RawSink;
use crate::web::AppState;

// ---------------------------------------------------------------------------
// Per-feeder state (module-level, protected by RwLock)
// ---------------------------------------------------------------------------

use std::sync::LazyLock;

static FEEDER_TRACKERS: LazyLock<RwLock<HashMap<String, FeederState>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

static RECEIVER_STATUS: LazyLock<RwLock<HashMap<String, ReceiverStatus>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Raw append-only sink — initialised once from `ADSB_RAW_SINK_DIR` at startup.
/// `None` when the env var is absent (no-op path; zero overhead).
static RAW_SINK: OnceLock<Option<RawSink>> = OnceLock::new();

fn raw_sink() -> Option<&'static RawSink> {
    RAW_SINK.get_or_init(RawSink::from_env).as_ref()
}

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
    /// When the hub first saw this receiver. The frame-stall clock starts
    /// here, so a receiver that never delivers a frame reads degraded one
    /// window after it appears — not "online" forever.
    first_seen: f64,
    /// Last time the hub had evidence frames were flowing: a non-empty
    /// `/api/v1/frames` batch, or a heartbeat whose `frames_captured` moved.
    /// None until the first such evidence.
    last_frames_at: Option<f64>,
}

impl ReceiverStatus {
    fn new(name: &str, lat: Option<f64>, lon: Option<f64>, frames_captured: u64, now: f64) -> Self {
        Self {
            name: name.to_string(),
            lat,
            lon,
            last_heartbeat: now,
            frames_captured,
            frames_sent: 0,
            uptime_sec: 0.0,
            active_aircraft: 0,
            online: true,
            first_seen: now,
            last_frames_at: None,
        }
    }

    /// Reference point for the stall clock: last frame evidence, else first sight.
    fn stall_reference(&self) -> f64 {
        self.last_frames_at.unwrap_or(self.first_seen)
    }

    fn health(&self, now: f64) -> ReceiverHealth {
        classify_health(now, self.last_heartbeat, self.stall_reference())
    }
}

/// Heartbeats older than this mean the agent itself is gone.
pub const HEARTBEAT_TIMEOUT_SECS: f64 = 120.0;

/// Heartbeats arriving but no frame evidence for this long means the agent is
/// up and its dongle is not. Mirrors the receiver-side watchdog default
/// (`adsb-receiver --stall-timeout`, 600 s): a working receiver sees frames
/// every second, so ten silent minutes is a dead device, not a quiet sky.
///
/// Why this exists: from 2026-10-05 to 2026-10-29 the blue-octopus receiver
/// heartbeated for 23 days with `frames_captured: 0` and this endpoint said
/// `online: true` the whole time. See `docs/incident-2026-10-05-silent-receiver.md`.
pub const FRAME_STALL_SECS: f64 = 600.0;

/// Three-way receiver health. `Online` is the only state that counts as healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiverHealth {
    /// Heartbeats fresh and frames flowing.
    Online,
    /// Heartbeats fresh, but no frame evidence within `FRAME_STALL_SECS`.
    Degraded,
    /// No heartbeat within `HEARTBEAT_TIMEOUT_SECS`.
    Offline,
}

impl ReceiverHealth {
    pub fn as_str(self) -> &'static str {
        match self {
            ReceiverHealth::Online => "online",
            ReceiverHealth::Degraded => "degraded",
            ReceiverHealth::Offline => "offline",
        }
    }
}

/// Pure classification so the thresholds are testable without the clock.
pub fn classify_health(now: f64, last_heartbeat: f64, stall_reference: f64) -> ReceiverHealth {
    if now - last_heartbeat >= HEARTBEAT_TIMEOUT_SECS {
        ReceiverHealth::Offline
    } else if now - stall_reference >= FRAME_STALL_SECS {
        ReceiverHealth::Degraded
    } else {
        ReceiverHealth::Online
    }
}

/// Hub-wide feed summary for `/api/stats`: the newest frame evidence across
/// all receivers plus how many are degraded. In-memory only — after a hub
/// restart `newest_frame_at` is None until the first batch lands.
pub struct FeedSummary {
    pub newest_frame_at: Option<f64>,
    pub receivers_degraded: usize,
}

pub fn feed_summary(now: f64) -> FeedSummary {
    let status = RECEIVER_STATUS.read().unwrap();
    let newest_frame_at = status
        .values()
        .filter_map(|s| s.last_frames_at)
        .fold(None, |acc: Option<f64>, t| {
            Some(acc.map_or(t, |a| a.max(t)))
        });
    let receivers_degraded = status
        .values()
        .filter(|s| s.health(now) == ReceiverHealth::Degraded)
        .count();
    FeedSummary {
        newest_frame_at,
        receivers_degraded,
    }
}

/// Collect only **non-stale** aircraft from all feeder trackers.
///
/// Without this filter, aircraft that haven't transmitted in >60 s linger
/// in the feeder trackers and get re-included in proximity-pair checks every
/// 10-second filter tick. Combined with the dedup-clear-on-stale logic, that
/// caused 0.8 events/sec/aircraft on prod (A35E49 = 2898 events/hour with the
/// same 5 nearby aircraft, 2026-05-05 incident) — the same pair re-fires on
/// every tick because dedup gets cleared but the stale aircraft is still
/// "near" the live one in collect_all_aircraft.
pub fn collect_active_aircraft(now_ts: f64) -> Vec<adsb_core::tracker::AircraftState> {
    let feeders = FEEDER_TRACKERS.read().unwrap();
    let mut all = Vec::new();
    for feeder in feeders.values() {
        for ac in feeder.tracker.aircraft.values() {
            if !ac.is_stale(now_ts) {
                all.push(ac.clone());
            }
        }
    }
    all
}

/// Prune stale aircraft from every feeder tracker. Returns the pruned ICAOs.
///
/// Without this, stale aircraft (>60 s no transmission) accumulate forever
/// in the feeder trackers — both a memory leak and the source of the
/// phantom-pair-refiring bug. Caller is expected to clear FilterEngine dedup
/// state for each returned ICAO so that if the aircraft reappears later, its
/// pair events can re-fire (e.g. a long signal dropout followed by re-entry).
pub fn prune_stale_from_feeders(now_ts: f64) -> Vec<adsb_core::types::Icao> {
    let mut feeders = FEEDER_TRACKERS.write().unwrap();
    let mut pruned = Vec::new();
    for feeder in feeders.values_mut() {
        // Identify stale ICAOs first, then prune. Tracker.prune_stale returns
        // a count, not the keys, so we collect before calling it.
        let stale: Vec<_> = feeder
            .tracker
            .aircraft
            .iter()
            .filter(|(_, ac)| ac.is_stale(now_ts))
            .map(|(k, _)| *k)
            .collect();
        for icao in &stale {
            feeder.tracker.aircraft.remove(icao);
        }
        pruned.extend(stale);
    }
    pruned
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
                anomaly_score,
                receiver_id,
                timestamp,
            } => {
                let icao_str = icao_to_string(icao);
                // Prefer auth-resolved receiver_id over tracker-assigned one
                let rid = auth_receiver_id.or(*receiver_id);
                // Combine the rules-based score (computed in the tracker
                // from physical-plausibility checks) with the statistical
                // score from the spatial-density baseline. Additive: each
                // rule contributes 0.5–2.0, baseline contributes 0–5.
                let stat_score = state
                    .baseline
                    .read()
                    .ok()
                    .map(|b| b.score(*lat, *lon))
                    .unwrap_or(0.0);
                let combined = Some(anomaly_score.unwrap_or(0.0) + stat_score);
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
                        combined,
                        rid,
                        *timestamp,
                    )
                    .await;

                // Phase-0 A2: tee to the raw append-only sink when configured.
                // Fire-and-forget; errors are logged to stderr inside RawSink::append
                // so a NAS hiccup cannot affect the DB write path above.
                if let Some(sink) = raw_sink() {
                    sink.write_adsb(
                        &icao_str,
                        *timestamp,
                        Some(*lat),
                        Some(*lon),
                        *altitude_ft,
                        &body.receiver,
                    );
                }
            }
        }
    }

    // Update receiver status
    {
        let current = now();
        let mut status = RECEIVER_STATUS.write().unwrap();
        let entry = status
            .entry(body.receiver.clone())
            .or_insert_with(|| ReceiverStatus::new(&body.receiver, body.lat, body.lon, 0, current));
        entry.last_heartbeat = current;
        entry.lat = body.lat.or(entry.lat);
        entry.lon = body.lon.or(entry.lon);
        entry.active_aircraft = active_count;
        // The agent only POSTs when its buffer is non-empty, so a batch is
        // direct evidence the dongle is producing frames.
        if !body.frames.is_empty() {
            entry.last_frames_at = Some(current);
        }
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

    let current = now();
    let mut status = RECEIVER_STATUS.write().unwrap();
    // Seed a brand-new entry with the reported count so the first heartbeat
    // after a hub restart isn't mistaken for the counter advancing.
    let entry = status.entry(body.receiver.clone()).or_insert_with(|| {
        ReceiverStatus::new(
            &body.receiver,
            body.lat,
            body.lon,
            body.frames_captured.unwrap_or(0),
            current,
        )
    });

    entry.last_heartbeat = current;
    entry.lat = body.lat.or(entry.lat);
    entry.lon = body.lon.or(entry.lon);
    if let Some(frames_captured) = body.frames_captured {
        // Only a *rising* counter is evidence of frames. A drop means the
        // agent restarted (counter reset to 0) — not a frame.
        if frames_captured > entry.frames_captured {
            entry.last_frames_at = Some(current);
        }
        entry.frames_captured = frames_captured;
    }
    entry.frames_sent = body.frames_sent.unwrap_or(entry.frames_sent);
    entry.uptime_sec = body.uptime_sec.unwrap_or(entry.uptime_sec);
    entry.online = true;

    (StatusCode::OK, Json(json!({"ok": true})))
}

/// GET /api/v1/receivers — list all receivers with status.
///
/// `online` is true only when heartbeats are fresh **and** frames are
/// flowing. A receiver whose agent heartbeats but whose dongle has gone
/// silent reads `online: false, degraded: true, status: "degraded"` so no
/// consumer — the dashboard, Splatlas, a monitor reading only `online` —
/// can mistake it for healthy. `heartbeat_ok` keeps the old liveness signal
/// for consumers that want the distinction.
pub async fn api_receivers(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let status = RECEIVER_STATUS.read().unwrap();
    let current = now();

    let receivers: Vec<Value> = status
        .values()
        .map(|s| {
            let health = s.health(current);
            json!({
                "name": s.name,
                "lat": s.lat,
                "lon": s.lon,
                "last_heartbeat": s.last_heartbeat,
                "frames_captured": s.frames_captured,
                "frames_sent": s.frames_sent,
                "uptime_sec": s.uptime_sec,
                "active_aircraft": s.active_aircraft,
                "online": health == ReceiverHealth::Online,
                "heartbeat_ok": health != ReceiverHealth::Offline,
                "degraded": health == ReceiverHealth::Degraded,
                "status": health.as_str(),
                "last_frames_at": s.last_frames_at,
                "frames_age_seconds": s.last_frames_at.map(|t| (current - t).max(0.0)),
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

    // Validate lat/lon ranges. Bots and broken clients have submitted garbage
    // (prod has rows with lat=2761703565); accept only finite values inside the
    // real coordinate envelope.
    if let Some(lat) = body.lat {
        if !lat.is_finite() || !(-90.0..=90.0).contains(&lat) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "lat must be between -90 and 90"})),
            );
        }
    }
    if let Some(lon) = body.lon {
        if !lon.is_finite() || !(-180.0..=180.0).contains(&lon) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "lon must be between -180 and 180"})),
            );
        }
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
            baseline: Arc::new(RwLock::new(crate::baseline::BaselineCache::new())),
            positions_broadcast: tokio::sync::broadcast::channel(8).0,
            tle_cache: crate::tle_cache::TleCache::new(),
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
            baseline: Arc::new(RwLock::new(crate::baseline::BaselineCache::new())),
            positions_broadcast: tokio::sync::broadcast::channel(8).0,
            tle_cache: crate::tle_cache::TleCache::new(),
        });
        (state, dir)
    }

    // -----------------------------------------------------------------------
    // Receiver health: heartbeats alone must not read as healthy
    // (2026-10-05 silent-receiver incident).
    // -----------------------------------------------------------------------

    #[test]
    fn classify_online_when_heartbeat_and_frames_fresh() {
        let now = 1_000_000.0;
        assert_eq!(
            classify_health(now, now - 30.0, now - 2.0),
            ReceiverHealth::Online
        );
    }

    #[test]
    fn classify_degraded_when_heartbeats_but_frames_stalled() {
        let now = 1_000_000.0;
        // Heartbeat 30 s ago, last frame evidence 10 min ago: the incident shape.
        assert_eq!(
            classify_health(now, now - 30.0, now - FRAME_STALL_SECS),
            ReceiverHealth::Degraded
        );
        // Just inside the window is still online — a quiet 9 minutes is not a fault.
        assert_eq!(
            classify_health(now, now - 30.0, now - FRAME_STALL_SECS + 1.0),
            ReceiverHealth::Online
        );
    }

    #[test]
    fn classify_offline_when_heartbeat_stale_regardless_of_frames() {
        let now = 1_000_000.0;
        assert_eq!(
            classify_health(now, now - HEARTBEAT_TIMEOUT_SECS, now - 1.0),
            ReceiverHealth::Offline
        );
        assert_eq!(
            classify_health(now, now - 86_400.0, now - 86_400.0),
            ReceiverHealth::Offline
        );
    }

    #[test]
    fn health_strings_match_dashboard_contract() {
        assert_eq!(ReceiverHealth::Online.as_str(), "online");
        assert_eq!(ReceiverHealth::Degraded.as_str(), "degraded");
        assert_eq!(ReceiverHealth::Offline.as_str(), "offline");
    }

    /// Backdate a receiver's frame-evidence clock so a test can cross the
    /// stall window without sleeping ten minutes.
    fn backdate_frames(name: &str, seconds: f64) {
        let mut status = RECEIVER_STATUS.write().unwrap();
        let entry = status.get_mut(name).expect("receiver registered");
        entry.first_seen -= seconds;
        if let Some(t) = entry.last_frames_at.as_mut() {
            *t -= seconds;
        }
    }

    async fn post_heartbeat(state: Arc<AppState>, body: &str) {
        let app = crate::web::build_router(state, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/heartbeat")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    async fn post_frames(state: Arc<AppState>, body: &str) {
        let app = crate::web::build_router(state, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/frames")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    async fn get_receiver(state: Arc<AppState>, name: &str) -> Value {
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
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        json.as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("receiver {name} missing from /api/v1/receivers"))
    }

    #[tokio::test]
    async fn receiver_heartbeating_with_stuck_frames_reads_degraded_not_online() {
        // The incident: agent up, heartbeats every 30 s, frames_captured stuck at 0.
        let (state, _dir) = test_state();
        let name = "health-stuck-rx";
        post_heartbeat(
            state.clone(),
            &format!(r#"{{"receiver":"{name}","frames_captured":0,"uptime_sec":30}}"#),
        )
        .await;

        // Within the window: online (benefit of the doubt while the clock runs).
        let rx = get_receiver(state.clone(), name).await;
        assert_eq!(rx["online"], true);
        assert_eq!(rx["degraded"], false);
        assert_eq!(rx["status"], "online");
        assert!(rx["last_frames_at"].is_null(), "no frame evidence yet");

        // Ten minutes later, still heartbeating, counter still 0.
        backdate_frames(name, FRAME_STALL_SECS);
        post_heartbeat(
            state.clone(),
            &format!(r#"{{"receiver":"{name}","frames_captured":0,"uptime_sec":630}}"#),
        )
        .await;

        let rx = get_receiver(state, name).await;
        assert_eq!(rx["online"], false, "a silent dongle must not read online");
        assert_eq!(rx["degraded"], true);
        assert_eq!(rx["heartbeat_ok"], true, "the agent itself is alive");
        assert_eq!(rx["status"], "degraded");
    }

    #[tokio::test]
    async fn rising_frames_captured_counts_as_frame_evidence() {
        let (state, _dir) = test_state();
        let name = "health-rising-rx";
        post_heartbeat(
            state.clone(),
            &format!(r#"{{"receiver":"{name}","frames_captured":1000}}"#),
        )
        .await;
        backdate_frames(name, FRAME_STALL_SECS);
        let rx = get_receiver(state.clone(), name).await;
        assert_eq!(rx["status"], "degraded");

        // Counter moves: back to healthy.
        post_heartbeat(
            state.clone(),
            &format!(r#"{{"receiver":"{name}","frames_captured":1500}}"#),
        )
        .await;
        let rx = get_receiver(state, name).await;
        assert_eq!(rx["online"], true);
        assert_eq!(rx["degraded"], false);
        assert_eq!(rx["status"], "online");
        let age = rx["frames_age_seconds"].as_f64().expect("age populated");
        assert!(age < 60.0, "fresh evidence, got age {age}");
    }

    #[tokio::test]
    async fn counter_reset_after_agent_restart_is_not_frame_evidence() {
        let (state, _dir) = test_state();
        let name = "health-reset-rx";
        post_heartbeat(
            state.clone(),
            &format!(r#"{{"receiver":"{name}","frames_captured":5000}}"#),
        )
        .await;
        backdate_frames(name, FRAME_STALL_SECS);
        // Agent restarted against a dead dongle: counter back to 0, no frames.
        post_heartbeat(
            state.clone(),
            &format!(r#"{{"receiver":"{name}","frames_captured":0,"uptime_sec":30}}"#),
        )
        .await;
        let rx = get_receiver(state.clone(), name).await;
        assert_eq!(
            rx["status"], "degraded",
            "a drop to 0 is a restart, not a frame"
        );
        assert_eq!(rx["frames_captured"], 0);

        // First frames after the restart: counter rises from 0, healthy again.
        post_heartbeat(
            state.clone(),
            &format!(r#"{{"receiver":"{name}","frames_captured":40,"uptime_sec":60}}"#),
        )
        .await;
        let rx = get_receiver(state, name).await;
        assert_eq!(rx["status"], "online");
    }

    #[tokio::test]
    async fn ingest_batch_counts_as_frame_evidence() {
        let (state, _dir) = test_state();
        let name = "health-batch-rx";
        post_heartbeat(
            state.clone(),
            &format!(r#"{{"receiver":"{name}","frames_captured":0}}"#),
        )
        .await;
        backdate_frames(name, FRAME_STALL_SECS);
        assert_eq!(
            get_receiver(state.clone(), name).await["status"],
            "degraded"
        );

        post_frames(
            state.clone(),
            &format!(
                r#"{{"receiver":"{name}","frames":[{{"hex":"8D4840D6202CC371C32CE0576098"}}]}}"#
            ),
        )
        .await;
        let rx = get_receiver(state, name).await;
        assert_eq!(rx["status"], "online");
        assert!(rx["last_frames_at"].as_f64().is_some());
    }

    #[tokio::test]
    async fn stale_heartbeat_reads_offline_not_degraded() {
        let (state, _dir) = test_state();
        let name = "health-offline-rx";
        post_heartbeat(
            state.clone(),
            &format!(r#"{{"receiver":"{name}","frames_captured":10}}"#),
        )
        .await;
        {
            let mut status = RECEIVER_STATUS.write().unwrap();
            status.get_mut(name).unwrap().last_heartbeat -= HEARTBEAT_TIMEOUT_SECS;
        }
        let rx = get_receiver(state, name).await;
        assert_eq!(rx["online"], false);
        assert_eq!(rx["heartbeat_ok"], false);
        assert_eq!(rx["degraded"], false);
        assert_eq!(rx["status"], "offline");
    }

    #[tokio::test]
    async fn feed_summary_counts_degraded_and_newest_frame() {
        let (state, _dir) = test_state();
        let name = "health-summary-rx";
        post_heartbeat(
            state.clone(),
            &format!(r#"{{"receiver":"{name}","frames_captured":0}}"#),
        )
        .await;
        backdate_frames(name, FRAME_STALL_SECS);
        let summary = feed_summary(now());
        assert!(
            summary.receivers_degraded >= 1,
            "the stuck receiver should count"
        );

        post_frames(
            state.clone(),
            &format!(
                r#"{{"receiver":"{name}","frames":[{{"hex":"8D4840D6202CC371C32CE0576098"}}]}}"#
            ),
        )
        .await;
        let current = now();
        let summary = feed_summary(current);
        let newest = summary.newest_frame_at.expect("a batch landed");
        assert!(current - newest < 60.0);
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

    #[tokio::test]
    async fn test_register_rejects_huge_garbage_coords() {
        // Mimics the actual junk that landed in prod: lat=2761703565, lon=9884768428.
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/register")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"junk-rx","lat":2761703565.0,"lon":9884768428.0}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_register_rejects_lat_just_over() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/register")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"oob","lat":90.0001,"lon":0.0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_register_rejects_lon_just_under() {
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/register")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"oob2","lat":0.0,"lon":-180.5}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_register_accepts_boundary_coords() {
        // 90/-180 are the inclusive boundary — north pole, antimeridian.
        let (state, _dir) = test_state();
        let app = crate::web::build_router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/register")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"boundary-rx","lat":90.0,"lon":-180.0}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
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
