//! Web server — axum REST API for ADS-B data.
//!
//! Shared state includes the database backend (SQLite or TimescaleDB),
//! an optional live tracker for real-time positions, and in-memory geofences.

use std::sync::{Arc, RwLock};

use axum::Router;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;

use adsb_core::tracker::Tracker;

use crate::db::AdsbDatabase;

pub mod ingest;
pub mod pages;
pub mod routes;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

pub struct AppState {
    pub db: Arc<dyn AdsbDatabase>,
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

pub fn build_router(state: Arc<AppState>, cors_origin: Option<&str>) -> Router {
    use http::HeaderValue;

    let mut app = Router::new()
        // Page routes
        .route("/", axum::routing::get(pages::page_map))
        .route("/table", axum::routing::get(pages::page_table))
        .route("/stats", axum::routing::get(pages::page_stats))
        .route("/events", axum::routing::get(pages::page_events))
        .route("/aircraft/:icao", axum::routing::get(pages::page_detail))
        .route("/query", axum::routing::get(pages::page_query))
        .route("/replay", axum::routing::get(pages::page_replay))
        .route("/receivers", axum::routing::get(pages::page_receivers))
        // API routes
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
            axum::routing::get(routes::api_geofences_list).post(routes::api_geofences_add),
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
        .with_state(state);

    // CORS — only add when explicitly configured
    if let Some(origin) = cors_origin {
        let cors = CorsLayer::new()
            .allow_origin(AllowOrigin::exact(
                HeaderValue::from_str(origin).expect("invalid CORS origin"),
            ))
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any);
        app = app.layer(cors);
    }

    // Security headers (match Caddyfile, safe even without reverse proxy)
    app = app
        .layer(SetResponseHeaderLayer::overriding(
            http::header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            http::header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            http::header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ));

    app
}

/// Start the web server.
pub async fn serve(
    db: Arc<dyn AdsbDatabase>,
    host: String,
    port: u16,
    cors_origin: Option<&str>,
) {
    let state = Arc::new(AppState {
        db,
        tracker: None,
        geofences: RwLock::new(Vec::new()),
        geofence_next_id: RwLock::new(1),
    });

    let app = build_router(state, cors_origin);
    let addr = format!("{host}:{port}");

    eprintln!("ADS-B server listening on http://{addr}");

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Error: cannot bind to {addr}: {e}");
            if e.kind() == std::io::ErrorKind::AddrInUse {
                eprintln!("Hint: port {port} is already in use. Try a different --port.");
            }
            std::process::exit(1);
        }
    };
    axum::serve(listener, app).await.unwrap();
}
