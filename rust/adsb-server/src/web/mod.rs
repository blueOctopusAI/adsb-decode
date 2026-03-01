//! Web server — axum REST API for ADS-B data.
//!
//! Shared state includes the DB path (each handler opens its own connection),
//! an optional live tracker for real-time positions, and in-memory geofences.

use std::sync::{Arc, RwLock};

use axum::Router;
use tower_http::cors::{Any, CorsLayer};

use adsb_core::tracker::Tracker;

pub mod ingest;
pub mod routes;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

pub struct AppState {
    pub db_path: String,
    pub tracker: Option<Arc<RwLock<Tracker>>>,
    pub geofences: RwLock<Vec<GeofenceEntry>>,
    pub geofence_next_id: RwLock<u64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GeofenceEntry {
    pub id: u64,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub radius_nm: f64,
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn build_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Page routes (placeholder — returns JSON until templates are ported)
        .route("/api/aircraft", axum::routing::get(routes::api_aircraft))
        .route(
            "/api/aircraft/:icao",
            axum::routing::get(routes::api_aircraft_detail),
        )
        .route("/api/positions", axum::routing::get(routes::api_positions))
        .route("/api/trails", axum::routing::get(routes::api_trails))
        .route("/api/events", axum::routing::get(routes::api_events))
        .route("/api/stats", axum::routing::get(routes::api_stats))
        .route("/api/query", axum::routing::get(routes::api_query))
        .route("/api/heatmap", axum::routing::get(routes::api_heatmap))
        .route("/api/airports", axum::routing::get(routes::api_airports))
        .route(
            "/api/positions/all",
            axum::routing::get(routes::api_positions_all),
        )
        .route(
            "/api/geofences",
            axum::routing::get(routes::api_geofences_list)
                .post(routes::api_geofences_add),
        )
        .route(
            "/api/geofences/:id",
            axum::routing::delete(routes::api_geofences_delete),
        )
        // Ingest API (multi-receiver)
        .route(
            "/api/v1/frames",
            axum::routing::post(ingest::api_ingest_frames),
        )
        .route(
            "/api/v1/heartbeat",
            axum::routing::post(ingest::api_heartbeat),
        )
        .route(
            "/api/v1/receivers",
            axum::routing::get(ingest::api_receivers),
        )
        .with_state(state)
        .layer(cors)
}

/// Start the web server.
pub async fn serve(db_path: String, host: String, port: u16) {
    let state = Arc::new(AppState {
        db_path,
        tracker: None,
        geofences: RwLock::new(Vec::new()),
        geofence_next_id: RwLock::new(1),
    });

    let app = build_router(state);
    let addr = format!("{host}:{port}");

    eprintln!("ADS-B server listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
