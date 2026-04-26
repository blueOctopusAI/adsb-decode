//! AIS ingester — connects to AISStream.io's WebSocket feed and writes
//! decoded vessel positions/static data into the same TimescaleDB
//! instance the ADS-B server uses.
//!
//! Independent process; runs alongside `adsb` (the main server). The web
//! dashboard reads from the shared `vessel_positions` + `vessels` tables
//! via the existing `/api/vessels*` routes — no protocol coupling between
//! ingester and server beyond the database.
//!
//! Configuration (all env vars):
//!
//!   AISSTREAM_API_KEY   (required) — get one at https://aisstream.io/apikeys
//!   DATABASE_URL        (required UNLESS AIS_DRY_RUN=1) —
//!                                    postgres://user:pass@host/db
//!   AIS_DRY_RUN         (optional) — `1` skips the DB entirely and prints
//!                                    parsed messages to stdout instead.
//!                                    Useful for verifying the WebSocket +
//!                                    parser end-to-end without a Postgres.
//!   AIS_BOUNDING_BOX    (optional) — JSON of `[[[lat_min, lon_min],
//!                                    [lat_max, lon_max]]]`. Default:
//!                                    global `[[[-90,-180],[90,180]]]`
//!   AIS_RECONNECT_MS    (optional) — milliseconds to wait between
//!                                    reconnect attempts. Default 5000.
//!   AIS_LOG_INTERVAL_S  (optional) — print stats every N seconds.
//!                                    Default 60.
//!
//! Reconnect: on any WebSocket disconnect or send error, sleep
//! AIS_RECONNECT_MS and retry. AISStream is a beta service with no SLA;
//! disconnects are routine.
//!
//! Per AISStream docs:
//!   - WSS only (rejected over plain WS)
//!   - Subscription must be sent within 3s of connect
//!   - Max 1 subscription update per second
//!   - May disconnect if data queue grows too large

use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

// Pull in the adsb-server crate's modules. We can't `use crate::ais;` here
// because this is a separate binary; we re-include the file via the
// build-time module declared in main.rs. Simplest: declare the modules
// inline for this binary too.
//
// In Cargo's "binary inside a library-less crate" pattern, each binary
// must redeclare any modules it uses from src/. The adsb-server crate
// has both src/main.rs (the `adsb` binary) and src/bin/ais-ingester.rs;
// the binary doesn't see main.rs's `mod ais;` declaration automatically.

#[path = "../ais.rs"]
mod ais;

#[path = "../db_pg.rs"]
#[cfg(feature = "timescaledb")]
mod db_pg;

// db_pg pulls in db (for shared types). Pull it in too.
#[path = "../db.rs"]
mod db;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = env::var("AISSTREAM_API_KEY")
        .map_err(|_| "AISSTREAM_API_KEY not set; get one at https://aisstream.io/apikeys")?;
    let dry_run = matches!(env::var("AIS_DRY_RUN").as_deref(), Ok("1") | Ok("true"));
    let database_url = if dry_run {
        String::new()
    } else {
        env::var("DATABASE_URL")
            .map_err(|_| "DATABASE_URL not set (postgres://user:pass@host/db); set AIS_DRY_RUN=1 to skip the DB")?
    };
    let bbox_json = env::var("AIS_BOUNDING_BOX")
        .unwrap_or_else(|_| "[[[-90,-180],[90,180]]]".to_string());
    let reconnect_ms: u64 = env::var("AIS_RECONNECT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000);
    let log_interval_s: u64 = env::var("AIS_LOG_INTERVAL_S")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    let bounding_boxes: serde_json::Value = serde_json::from_str(&bbox_json)
        .map_err(|e| format!("AIS_BOUNDING_BOX is not valid JSON: {e}"))?;

    eprintln!("ais-ingester starting{}", if dry_run { " (dry-run, no DB)" } else { "" });
    eprintln!("  bounding box: {}", bbox_json);
    eprintln!("  reconnect: {} ms", reconnect_ms);
    eprintln!("  log interval: {} s", log_interval_s);

    let db: Option<Arc<db_pg::TimescaleDb>> = if dry_run {
        None
    } else {
        let connected = db_pg::TimescaleDb::connect(&database_url).await?;
        eprintln!("  connected to database");
        Some(Arc::new(connected))
    };

    let positions_count = Arc::new(AtomicU64::new(0));
    let static_count = Arc::new(AtomicU64::new(0));
    let dropped_count = Arc::new(AtomicU64::new(0));
    let reconnect_count = Arc::new(AtomicU64::new(0));

    spawn_stats_loop(
        log_interval_s,
        positions_count.clone(),
        static_count.clone(),
        dropped_count.clone(),
        reconnect_count.clone(),
    );

    loop {
        match run_ingester(
            &api_key,
            &bounding_boxes,
            db.clone(),
            positions_count.clone(),
            static_count.clone(),
            dropped_count.clone(),
            dry_run,
        )
        .await
        {
            Ok(_) => eprintln!("websocket loop exited cleanly; reconnecting"),
            Err(e) => eprintln!("websocket loop error: {e}; reconnecting in {reconnect_ms} ms"),
        }
        reconnect_count.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(reconnect_ms)).await;
    }
}

async fn run_ingester(
    api_key: &str,
    bounding_boxes: &serde_json::Value,
    db: Option<Arc<db_pg::TimescaleDb>>,
    positions_count: Arc<AtomicU64>,
    static_count: Arc<AtomicU64>,
    dropped_count: Arc<AtomicU64>,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = "wss://stream.aisstream.io/v0/stream";
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await?;

    let subscription = json!({
        "APIKey": api_key,
        "BoundingBoxes": bounding_boxes,
        "FilterMessageTypes": [
            "PositionReport",
            "StandardClassBPositionReport",
            "ExtendedClassBPositionReport",
            "ShipStaticData"
        ]
    });
    socket.send(Message::Text(subscription.to_string())).await?;
    eprintln!("subscribed to AISStream");

    while let Some(msg) = socket.next().await {
        let msg = msg?;
        let text = match msg {
            Message::Text(t) => t,
            Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
            Message::Close(_) => {
                eprintln!("server closed connection");
                return Ok(());
            }
            Message::Ping(p) => {
                socket.send(Message::Pong(p)).await?;
                continue;
            }
            Message::Pong(_) | Message::Frame(_) => continue,
        };

        if dry_run {
            // In dry-run, print every raw frame so we can see what AISStream
            // is actually sending (or not). Truncate long frames for readability.
            let preview: String = text.chars().take(700).collect();
            eprintln!("RAW {}", preview);
        }

        let parsed = ais::parse_message(&text);
        if dry_run && parsed.is_none() {
            // Diagnose why we couldn't parse — peek at the top-level keys.
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                let keys: Vec<&str> = v.as_object()
                    .map(|m| m.keys().map(|s| s.as_str()).collect())
                    .unwrap_or_default();
                let mt = v.get("MessageType").and_then(|x| x.as_str()).unwrap_or("<missing>");
                eprintln!("PARSE_FAIL keys={:?} MessageType={}", keys, mt);
            } else {
                eprintln!("PARSE_FAIL: not valid JSON");
            }
        }
        match parsed {
            Some(ais::AisParsed::Position(p)) => {
                if dry_run {
                    println!(
                        "POS  mmsi={} lat={:.5} lon={:.5} sog={:?} cog={:?} hdg={:?} name={:?}",
                        p.mmsi, p.lat, p.lon, p.speed_kts, p.course_deg, p.heading_deg, p.ship_name
                    );
                    positions_count.fetch_add(1, Ordering::Relaxed);
                } else if let Some(db) = &db {
                    let now = epoch_now();
                    let res = db
                        .add_vessel_position(
                            &p.mmsi,
                            p.lat,
                            p.lon,
                            p.speed_kts,
                            p.course_deg,
                            p.heading_deg,
                            now,
                        )
                        .await;
                    if res.is_ok() {
                        positions_count.fetch_add(1, Ordering::Relaxed);
                        // If a ship name came through with the position, opportunistically
                        // upsert it so the vessels table fills in even before we see a
                        // ShipStaticData broadcast (which is rarer).
                        if let Some(name) = p.ship_name.as_deref() {
                            let _ = db.upsert_vessel(&p.mmsi, Some(name), None, None, now).await;
                        }
                    } else {
                        dropped_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Some(ais::AisParsed::Static(s)) => {
                if dry_run {
                    println!(
                        "STAT mmsi={} name={:?} type={:?}",
                        s.mmsi, s.name, s.vessel_type
                    );
                    static_count.fetch_add(1, Ordering::Relaxed);
                } else if let Some(db) = &db {
                    let now = epoch_now();
                    let res = db
                        .upsert_vessel(
                            &s.mmsi,
                            s.name.as_deref(),
                            s.vessel_type.as_deref(),
                            s.flag.as_deref(),
                            now,
                        )
                        .await;
                    if res.is_ok() {
                        static_count.fetch_add(1, Ordering::Relaxed);
                    } else {
                        dropped_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            None => {
                // unhandled message type or parse failure — silently skip
            }
        }
    }
    Ok(())
}

fn epoch_now() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn spawn_stats_loop(
    interval_s: u64,
    positions: Arc<AtomicU64>,
    statics: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    reconnects: Arc<AtomicU64>,
) {
    tokio::spawn(async move {
        let start = Instant::now();
        let mut last_pos = 0u64;
        let mut last_stat = 0u64;
        loop {
            tokio::time::sleep(Duration::from_secs(interval_s)).await;
            let p = positions.load(Ordering::Relaxed);
            let s = statics.load(Ordering::Relaxed);
            let d = dropped.load(Ordering::Relaxed);
            let r = reconnects.load(Ordering::Relaxed);
            let dp = p.saturating_sub(last_pos);
            let ds = s.saturating_sub(last_stat);
            let elapsed = start.elapsed().as_secs();
            eprintln!(
                "[{}s] pos+{dp} stat+{ds} | total pos={p} stat={s} dropped={d} reconnects={r}",
                elapsed
            );
            last_pos = p;
            last_stat = s;
        }
    });
}
