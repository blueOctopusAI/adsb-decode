//! Web server — axum REST API for ADS-B data.
//!
//! Shared state includes the database backend (SQLite or TimescaleDB),
//! an optional live tracker for real-time positions, and in-memory geofences.

use std::sync::{Arc, RwLock};

use axum::extract::DefaultBodyLimit;
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
    pub auth_token: Option<String>,
    pub photo_cache: std::sync::Mutex<std::collections::HashMap<String, Option<serde_json::Value>>>,
    pub airspace_cache: std::sync::Mutex<Option<(std::time::Instant, serde_json::Value)>>,
    pub ollama_url: Option<String>,
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
        .route("/register", axum::routing::get(pages::page_register))
        .route("/about", axum::routing::get(pages::page_about))
        .route(
            "/how-it-works",
            axum::routing::get(pages::page_how_it_works),
        )
        .route("/features", axum::routing::get(pages::page_features))
        .route("/setup", axum::routing::get(pages::page_setup))
        // SEO + AI routes
        .route("/robots.txt", axum::routing::get(pages::robots_txt))
        .route("/sitemap.xml", axum::routing::get(pages::sitemap_xml))
        .route("/llms.txt", axum::routing::get(pages::llms_txt))
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
        .route("/api/lookup/:icao", axum::routing::get(routes::api_lookup))
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
        .route("/api/photos/:icao", axum::routing::get(routes::api_photos))
        .route("/api/airspace", axum::routing::get(routes::api_airspace))
        .route("/api/nlp-query", axum::routing::post(routes::api_nlp_query))
        // Vessel (AIS) API
        .route("/api/vessels", axum::routing::get(routes::api_vessels))
        .route(
            "/api/vessel-positions",
            axum::routing::get(routes::api_vessel_positions),
        )
        .route(
            "/api/vessel-positions/latest",
            axum::routing::get(routes::api_vessel_positions_latest),
        )
        // Registration API
        .route("/api/register", axum::routing::post(ingest::api_register))
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
        .layer(DefaultBodyLimit::max(512 * 1024)); // 512 KB max request body

    // CORS — only add when explicitly configured
    if let Some(origin) = cors_origin {
        let cors = CorsLayer::new()
            .allow_origin(AllowOrigin::exact(
                HeaderValue::from_str(origin).expect("invalid CORS origin"),
            ))
            .allow_methods([http::Method::GET, http::Method::POST, http::Method::DELETE])
            .allow_headers([http::header::CONTENT_TYPE, http::header::AUTHORIZATION]);
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
    auth_token: Option<String>,
    ollama_url: Option<String>,
) {
    let state = Arc::new(AppState {
        db,
        tracker: None,
        geofences: RwLock::new(Vec::new()),
        geofence_next_id: RwLock::new(1),
        auth_token,
        photo_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        airspace_cache: std::sync::Mutex::new(None),
        ollama_url,
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
