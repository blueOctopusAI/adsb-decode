//! adsb-server: CLI + web server for ADS-B tracking.

use std::collections::HashMap;
use std::io::{self, BufRead};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use comfy_table::{Cell, Table};

use adsb_core::cpr;
use adsb_core::decode;
use adsb_core::frame::{self, IcaoCache};
use adsb_core::icao;
use adsb_core::tracker::Tracker;
use adsb_core::types::*;

mod db;
#[cfg(feature = "timescaledb")]
mod db_pg;
mod web;

#[derive(Parser)]
#[command(name = "adsb", version, about = "ADS-B decoder and tracker")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Decode hex frames from a file and print aircraft table
    Decode {
        /// Path to file containing hex frames (one per line)
        file: PathBuf,

        /// Show raw decoded messages instead of summary table
        #[arg(short, long)]
        raw: bool,
    },

    /// Track aircraft from a capture file or live RTL-SDR
    Track {
        /// Path to file containing hex frames (one per line)
        file: Option<PathBuf>,

        /// Live capture from RTL-SDR dongle (via rtl_adsb)
        #[arg(long)]
        live: bool,

        /// Use native IQ demodulation instead of rtl_adsb (pipe from rtl_sdr)
        #[arg(long)]
        native_demod: bool,

        /// SQLite database path
        #[arg(long, default_value = "data/adsb.db")]
        db_path: String,

        /// Minimum seconds between stored positions per aircraft
        #[arg(long, default_value = "2.0")]
        min_interval: f64,

        /// Launch web dashboard on this port
        #[arg(short, long)]
        port: Option<u16>,

        /// CORS allowed origin (e.g. "https://example.com"). Omit for same-origin only.
        #[arg(long)]
        cors_origin: Option<String>,
    },

    /// Show database statistics
    Stats {
        /// SQLite database path
        #[arg(long, default_value = "data/adsb.db")]
        db_path: String,
    },

    /// Show aircraft history from database
    History {
        /// SQLite database path
        #[arg(long, default_value = "data/adsb.db")]
        db_path: String,

        /// Show aircraft from last N hours
        #[arg(long, default_value = "24")]
        last: f64,

        /// Filter by ICAO address
        #[arg(long)]
        icao: Option<String>,
    },

    /// Export position data to CSV or JSON
    Export {
        /// SQLite database path
        #[arg(long, default_value = "data/adsb.db")]
        db_path: String,

        /// Output format (csv, json)
        #[arg(short, long, default_value = "csv")]
        format: String,

        /// Output file (stdout if not specified)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Export data from last N hours
        #[arg(long)]
        last: Option<f64>,

        /// Filter by ICAO address
        #[arg(long)]
        icao: Option<String>,

        /// Maximum rows to export
        #[arg(long, default_value = "100000")]
        limit: i64,
    },

    /// Start the web server
    Serve {
        /// SQLite database path
        #[arg(long, default_value = "data/adsb.db")]
        db_path: String,

        /// Port to listen on
        #[arg(short, long, default_value = "8080")]
        port: u16,

        /// Host to bind to
        #[arg(long, default_value = "0.0.0.0")]
        host: String,

        /// CORS allowed origin (e.g. "https://example.com"). Omit for same-origin only.
        #[arg(long)]
        cors_origin: Option<String>,
    },

    /// Interactive setup wizard — configure receiver, database, and server
    Setup,
}

/// Accumulated aircraft state from decoded messages.
struct AircraftState {
    icao: Icao,
    callsign: Option<String>,
    altitude_ft: Option<i32>,
    speed_kts: Option<f64>,
    heading_deg: Option<f64>,
    vertical_rate: Option<i32>,
    squawk: Option<String>,
    lat: Option<f64>,
    lon: Option<f64>,
    country: Option<&'static str>,
    messages: u32,
    // CPR pairing state
    even_lat: Option<u32>,
    even_lon: Option<u32>,
    even_ts: Option<f64>,
    odd_lat: Option<u32>,
    odd_lon: Option<u32>,
    odd_ts: Option<f64>,
}

impl AircraftState {
    fn new(icao: Icao) -> Self {
        AircraftState {
            icao,
            callsign: None,
            altitude_ft: None,
            speed_kts: None,
            heading_deg: None,
            vertical_rate: None,
            squawk: None,
            lat: None,
            lon: None,
            country: icao::lookup_country(&icao),
            messages: 0,
            even_lat: None,
            even_lon: None,
            even_ts: None,
            odd_lat: None,
            odd_lon: None,
            odd_ts: None,
        }
    }

    fn update(&mut self, msg: &DecodedMsg) {
        self.messages += 1;
        match msg {
            DecodedMsg::Identification(m) => {
                self.callsign = Some(m.callsign.trim().to_string());
            }
            DecodedMsg::Position(m) => {
                if let Some(alt) = m.altitude_ft {
                    self.altitude_ft = Some(alt);
                }
                // Store CPR data for global decode
                if m.cpr_odd {
                    self.odd_lat = Some(m.cpr_lat);
                    self.odd_lon = Some(m.cpr_lon);
                    self.odd_ts = Some(m.timestamp);
                } else {
                    self.even_lat = Some(m.cpr_lat);
                    self.even_lon = Some(m.cpr_lon);
                    self.even_ts = Some(m.timestamp);
                }
                // Attempt global CPR decode
                if let (Some(elat), Some(elon), Some(ets), Some(olat), Some(olon), Some(ots)) = (
                    self.even_lat,
                    self.even_lon,
                    self.even_ts,
                    self.odd_lat,
                    self.odd_lon,
                    self.odd_ts,
                ) {
                    if let Some((lat, lon)) = cpr::global_decode(elat, elon, olat, olon, ets, ots) {
                        self.lat = Some(lat);
                        self.lon = Some(lon);
                    }
                }
            }
            DecodedMsg::Velocity(m) => {
                self.speed_kts = m.speed_kts;
                self.heading_deg = m.heading_deg;
                self.vertical_rate = m.vertical_rate_fpm;
            }
            DecodedMsg::Altitude(m) => {
                self.altitude_ft = m.altitude_ft;
            }
            DecodedMsg::Squawk(m) => {
                self.squawk = Some(m.squawk.clone());
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Decode { file, raw } => cmd_decode(file, raw),
        Commands::Track {
            file,
            live,
            native_demod,
            db_path,
            min_interval,
            port,
            cors_origin,
        } => {
            if !live && file.is_none() {
                eprintln!("Error: provide a FILE or use --live for RTL-SDR capture");
                std::process::exit(1);
            }
            if native_demod && !live {
                eprintln!("Error: --native-demod requires --live");
                std::process::exit(1);
            }
            if live {
                if native_demod {
                    cmd_track_live_native(&db_path, min_interval, port, cors_origin.as_deref())
                        .await;
                } else {
                    cmd_track_live(&db_path, min_interval, port, cors_origin.as_deref()).await;
                }
            } else {
                cmd_track(file.unwrap(), &db_path, min_interval);
            }
        }
        Commands::Stats { db_path } => cmd_stats(&db_path),
        Commands::History {
            db_path,
            last,
            icao,
        } => cmd_history(&db_path, last, icao.as_deref()),
        Commands::Export {
            db_path,
            format,
            output,
            last,
            icao,
            limit,
        } => cmd_export(&db_path, &format, output, last, icao.as_deref(), limit),
        Commands::Serve {
            db_path,
            port,
            host,
            cors_origin,
        } => {
            let db: std::sync::Arc<dyn db::AdsbDatabase> =
                std::sync::Arc::new(db::SqliteDb::new(db_path));
            web::serve(db, host, port, cors_origin.as_deref()).await;
        }
        Commands::Setup => cmd_setup(),
    }
}

fn cmd_decode(file: PathBuf, raw: bool) {
    let reader: Box<dyn BufRead> = if file.to_str() == Some("-") {
        Box::new(io::stdin().lock())
    } else {
        let f = std::fs::File::open(&file).unwrap_or_else(|e| {
            eprintln!("Error opening {}: {e}", file.display());
            std::process::exit(1);
        });
        Box::new(io::BufReader::new(f))
    };

    let mut icao_cache = IcaoCache::new(60.0);
    let mut aircraft: HashMap<Icao, AircraftState> = HashMap::new();
    let mut total_frames = 0u64;
    let mut decoded_frames = 0u64;
    let mut timestamp = 0.0f64;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        let hex = line.trim();
        if hex.is_empty() || hex.starts_with('#') {
            continue;
        }

        // Handle lines with format "hex;timestamp" or just "hex"
        let (hex_part, ts) = if let Some((h, t)) = hex.split_once(';') {
            (h.trim(), t.trim().parse::<f64>().unwrap_or(timestamp))
        } else {
            (hex, timestamp)
        };
        timestamp = ts + 0.001; // Auto-increment for files without timestamps

        let frame = match frame::parse_frame(hex_part, ts, None, true, &mut icao_cache) {
            Some(f) => f,
            None => continue,
        };

        total_frames += 1;

        if let Some(msg) = decode::decode(&frame) {
            decoded_frames += 1;

            if raw {
                println!("{msg:?}");
            }

            let state = aircraft
                .entry(frame.icao)
                .or_insert_with(|| AircraftState::new(frame.icao));
            state.update(&msg);
        }
    }

    if !raw {
        print_summary(&aircraft, total_frames, decoded_frames);
    }
}

fn cmd_track(file: PathBuf, db_path: &str, min_interval: f64) {
    let mut database = db::Database::open(db_path).unwrap_or_else(|e| {
        eprintln!("Error opening database {db_path}: {e}");
        std::process::exit(1);
    });

    let source = file.display().to_string();
    let capture_id = database.start_capture(&source, None);

    let mut tracker = Tracker::new(None, Some(capture_id), None, None, min_interval);
    let mut icao_cache = IcaoCache::new(60.0);

    let reader: Box<dyn BufRead> = if file.to_str() == Some("-") {
        Box::new(io::stdin().lock())
    } else {
        let f = std::fs::File::open(&file).unwrap_or_else(|e| {
            eprintln!("Error opening {}: {e}", file.display());
            std::process::exit(1);
        });
        Box::new(io::BufReader::new(f))
    };

    let mut timestamp = 0.0f64;

    // Batch mode for throughput
    database.set_autocommit(false);

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        let hex = line.trim();
        if hex.is_empty() || hex.starts_with('#') {
            continue;
        }

        let (hex_part, ts) = if let Some((h, t)) = hex.split_once(';') {
            (h.trim(), t.trim().parse::<f64>().unwrap_or(timestamp))
        } else {
            (hex, timestamp)
        };
        timestamp = ts + 0.1;

        let frame = match frame::parse_frame(hex_part, ts, None, true, &mut icao_cache) {
            Some(f) => f,
            None => match frame::parse_frame(hex_part, ts, None, false, &mut icao_cache) {
                Some(f) => f,
                None => continue,
            },
        };

        let (_msg, events) = tracker.update(&frame);
        database.apply_events(&events);
    }

    database.flush();
    database.end_capture(
        capture_id,
        tracker.total_frames,
        tracker.valid_frames,
        tracker.aircraft.len() as u64,
    );
    database.flush();

    // Print summary
    let stats = database.stats();
    println!();
    println!("Track complete: {}", file.display());
    println!(
        "  Frames: {} total, {} valid",
        tracker.total_frames, tracker.valid_frames
    );
    println!(
        "  Positions: {} decoded, {} stored, {} downsampled",
        tracker.position_decodes,
        tracker.position_decodes - tracker.positions_skipped,
        tracker.positions_skipped
    );
    println!("  Aircraft: {}", tracker.aircraft.len());
    println!();
    println!("Database: {db_path}");
    println!(
        "  {} aircraft, {} positions, {} events",
        stats.aircraft, stats.positions, stats.events
    );

    // Print aircraft table
    let now = timestamp;
    let active = tracker.get_active(now + 3600.0); // Show all (generous timeout)

    if !active.is_empty() {
        println!();
        let mut table = Table::new();
        table.set_header(vec![
            "ICAO", "Callsign", "Squawk", "Alt (ft)", "Speed", "Hdg", "Lat", "Lon", "Country",
            "Msgs",
        ]);

        for ac in &active {
            table.add_row(vec![
                Cell::new(icao_to_string(&ac.icao)),
                Cell::new(ac.callsign.as_deref().unwrap_or("-")),
                Cell::new(ac.squawk.as_deref().unwrap_or("-")),
                Cell::new(ac.altitude_ft.map(|a| a.to_string()).unwrap_or("-".into())),
                Cell::new(
                    ac.speed_kts
                        .map(|s| format!("{s:.0}"))
                        .unwrap_or("-".into()),
                ),
                Cell::new(
                    ac.heading_deg
                        .map(|h| format!("{h:.1}"))
                        .unwrap_or("-".into()),
                ),
                Cell::new(ac.lat.map(|l| format!("{l:.4}")).unwrap_or("-".into())),
                Cell::new(ac.lon.map(|l| format!("{l:.4}")).unwrap_or("-".into())),
                Cell::new(ac.country.unwrap_or("-")),
                Cell::new(ac.message_count),
            ]);
        }

        println!("{table}");
    }
}

async fn cmd_track_live(
    db_path: &str,
    min_interval: f64,
    port: Option<u16>,
    cors_origin: Option<&str>,
) {
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex, RwLock};

    let mut database = db::Database::open(db_path).unwrap_or_else(|e| {
        eprintln!("Error opening database {db_path}: {e}");
        std::process::exit(1);
    });

    let source = "rtl_adsb:live";
    let capture_id = database.start_capture(source, None);
    database.set_autocommit(false);

    let database = Arc::new(Mutex::new(database));

    let tracker = Arc::new(RwLock::new(Tracker::new(
        None,
        Some(capture_id),
        None,
        None,
        min_interval,
    )));
    let mut icao_cache = IcaoCache::new(60.0);

    // Start web server if --port given
    // SqliteDb opens fresh connections per request, so it sees writes from our Database
    if let Some(p) = port {
        let web_db: Arc<dyn db::AdsbDatabase> = Arc::new(db::SqliteDb::new(db_path.to_string()));
        let state = Arc::new(web::AppState {
            db: web_db,
            tracker: Some(tracker.clone()),
            geofences: RwLock::new(Vec::new()),
            geofence_next_id: RwLock::new(1),
        });
        let app = web::build_router(state, cors_origin);
        let addr = format!("0.0.0.0:{p}");
        eprintln!("Dashboard → http://127.0.0.1:{p}");
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Error: cannot bind to {addr}: {e}");
                if e.kind() == std::io::ErrorKind::AddrInUse {
                    eprintln!("Hint: port {p} is already in use. Try a different --port.");
                }
                std::process::exit(1);
            }
        };
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
    }

    // Background data retention task (every 60 minutes)
    let retention_db = database.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        interval.tick().await; // skip immediate first tick
        loop {
            interval.tick().await;
            let mut db = retention_db.lock().unwrap();
            let pruned_pos = db.prune_positions(72);
            let downsampled = db.downsample_positions(24, 30);
            let phantoms = db.prune_phantom_aircraft(24.0);
            let pruned_evt = db.prune_events(168);
            db.flush();
            eprintln!(
                "  [retention] pruned {pruned_pos} positions, downsampled {downsampled}, \
                 removed {phantoms} phantom aircraft, pruned {pruned_evt} events"
            );
        }
    });

    // Spawn rtl_adsb subprocess
    eprintln!("Starting rtl_adsb...");
    let mut child = Command::new("rtl_adsb")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| {
            eprintln!("Failed to start rtl_adsb: {e}");
            eprintln!("Make sure rtl-sdr tools are installed (brew install librtlsdr)");
            std::process::exit(1);
        });

    let stdout = child.stdout.take().unwrap();
    let reader = io::BufReader::new(stdout);

    eprintln!("Live tracking started — Ctrl+C to stop\n");

    let mut last_print = std::time::Instant::now();
    let mut last_flush = std::time::Instant::now();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let hex = line.trim();
        if hex.is_empty() || hex.starts_with('#') {
            continue;
        }

        // rtl_adsb outputs lines like "*8D4840D6202CC371C32CE0576098;"
        let hex_clean = if hex.starts_with('*') && hex.ends_with(';') {
            &hex[1..hex.len() - 1]
        } else {
            hex
        };

        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();

        let frame = match frame::parse_frame(hex_clean, now_ts, None, true, &mut icao_cache) {
            Some(f) => f,
            None => continue,
        };

        let events = {
            let mut t = tracker.write().unwrap();
            let (_msg, events) = t.update(&frame);
            events
        };

        database.lock().unwrap().apply_events(&events);

        // Periodic status + flush
        if last_print.elapsed().as_secs_f64() > 10.0 {
            let mut t = tracker.write().unwrap();
            let active = t.get_active(now_ts);
            eprintln!(
                "  {} frames, {} valid, {} active aircraft, {} positions",
                t.total_frames, t.valid_frames, active.len(), t.position_decodes
            );
            t.prune_stale(now_ts);
            last_print = std::time::Instant::now();
        }

        if last_flush.elapsed().as_secs_f64() > 5.0 {
            database.lock().unwrap().flush();
            last_flush = std::time::Instant::now();
        }
    }

    // Cleanup — runs on Ctrl+C (child gets SIGINT, stdout closes, loop exits)
    let _ = child.kill();
    let _ = child.wait();
    let mut db = database.lock().unwrap();
    db.flush();
    let t = tracker.read().unwrap();
    db.end_capture(
        capture_id,
        t.total_frames,
        t.valid_frames,
        t.aircraft.len() as u64,
    );
    db.flush();
    eprintln!(
        "\nStopped. {} frames, {} valid, {} aircraft",
        t.total_frames, t.valid_frames, t.aircraft.len()
    );
}

async fn cmd_track_live_native(
    db_path: &str,
    min_interval: f64,
    port: Option<u16>,
    cors_origin: Option<&str>,
) {
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex, RwLock};

    let mut database = db::Database::open(db_path).unwrap_or_else(|e| {
        eprintln!("Error opening database {db_path}: {e}");
        std::process::exit(1);
    });

    let source = "rtl_sdr:native_demod";
    let capture_id = database.start_capture(source, None);
    database.set_autocommit(false);

    let database = Arc::new(Mutex::new(database));

    let tracker = Arc::new(RwLock::new(Tracker::new(
        None,
        Some(capture_id),
        None,
        None,
        min_interval,
    )));
    let mut icao_cache = IcaoCache::new(60.0);

    // Start web server if --port given
    if let Some(p) = port {
        let web_db: Arc<dyn db::AdsbDatabase> = Arc::new(db::SqliteDb::new(db_path.to_string()));
        let state = Arc::new(web::AppState {
            db: web_db,
            tracker: Some(tracker.clone()),
            geofences: RwLock::new(Vec::new()),
            geofence_next_id: RwLock::new(1),
        });
        let app = web::build_router(state, cors_origin);
        let addr = format!("0.0.0.0:{p}");
        eprintln!("Dashboard → http://127.0.0.1:{p}");
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Error: cannot bind to {addr}: {e}");
                if e.kind() == std::io::ErrorKind::AddrInUse {
                    eprintln!("Hint: port {p} is already in use. Try a different --port.");
                }
                std::process::exit(1);
            }
        };
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
    }

    // Background data retention task (every 60 minutes)
    let retention_db = database.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        interval.tick().await; // skip immediate first tick
        loop {
            interval.tick().await;
            let mut db = retention_db.lock().unwrap();
            let pruned_pos = db.prune_positions(72);
            let downsampled = db.downsample_positions(24, 30);
            let phantoms = db.prune_phantom_aircraft(24.0);
            let pruned_evt = db.prune_events(168);
            db.flush();
            eprintln!(
                "  [retention] pruned {pruned_pos} positions, downsampled {downsampled}, \
                 removed {phantoms} phantom aircraft, pruned {pruned_evt} events"
            );
        }
    });

    // Spawn rtl_sdr — outputs raw interleaved uint8 IQ pairs to stdout
    eprintln!("Starting rtl_sdr (native demod)...");
    let mut child = Command::new("rtl_sdr")
        .args(["-f", "1090000000", "-s", "2000000", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| {
            eprintln!("Failed to start rtl_sdr: {e}");
            eprintln!("Make sure rtl-sdr tools are installed (brew install librtlsdr)");
            std::process::exit(1);
        });

    let mut stdout = child.stdout.take().unwrap();
    let mut noise_tracker = adsb_core::demod::NoiseFloorTracker::new();
    let sample_rate = 2_000_000u32;

    eprintln!("Live tracking started (native demod) — Ctrl+C to stop\n");

    let mut last_print = std::time::Instant::now();
    let mut last_flush = std::time::Instant::now();
    let tracker_ref = tracker.clone();
    let loop_db = database.clone();

    let result = adsb_feeder::capture::demodulate_stream(
        &mut stdout,
        sample_rate,
        &mut noise_tracker,
        &mut |raw_frame| {
            let now_ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64();

            let frame = match frame::parse_frame(
                &raw_frame.hex_str,
                now_ts,
                Some(raw_frame.signal_level as f64),
                true,
                &mut icao_cache,
            ) {
                Some(f) => f,
                None => return,
            };

            let events = {
                let mut t = tracker_ref.write().unwrap();
                let (_msg, events) = t.update(&frame);
                events
            };

            loop_db.lock().unwrap().apply_events(&events);

            if last_print.elapsed().as_secs_f64() > 10.0 {
                let mut t = tracker_ref.write().unwrap();
                let active = t.get_active(now_ts);
                eprintln!(
                    "  {} frames, {} valid, {} active aircraft, {} positions [native]",
                    t.total_frames, t.valid_frames, active.len(), t.position_decodes
                );
                t.prune_stale(now_ts);
                last_print = std::time::Instant::now();
            }

            if last_flush.elapsed().as_secs_f64() > 5.0 {
                loop_db.lock().unwrap().flush();
                last_flush = std::time::Instant::now();
            }
        },
    );

    if let Err(e) = result {
        eprintln!("Stream error: {e}");
    }

    // Cleanup — runs on Ctrl+C (child gets SIGINT, stdout closes, stream exits)
    let _ = child.kill();
    let _ = child.wait();
    let mut db = database.lock().unwrap();
    db.flush();
    let t = tracker.read().unwrap();
    db.end_capture(
        capture_id,
        t.total_frames,
        t.valid_frames,
        t.aircraft.len() as u64,
    );
    db.flush();
    eprintln!(
        "\nStopped. {} frames, {} valid, {} aircraft [native demod]",
        t.total_frames, t.valid_frames, t.aircraft.len()
    );
}

fn cmd_stats(db_path: &str) {
    let database = db::Database::open(db_path).unwrap_or_else(|e| {
        eprintln!("Error opening database {db_path}: {e}");
        std::process::exit(1);
    });

    let stats = database.stats();

    println!();
    println!("Database: {db_path}");
    println!();
    println!("  Aircraft:   {}", stats.aircraft);
    println!("  Positions:  {}", stats.positions);
    println!("  Events:     {}", stats.events);
    println!("  Receivers:  {}", stats.receivers);
    println!("  Captures:   {}", stats.captures);
    println!();
}

fn cmd_history(db_path: &str, hours: f64, icao_filter: Option<&str>) {
    let database = db::Database::open(db_path).unwrap_or_else(|e| {
        eprintln!("Error opening database {db_path}: {e}");
        std::process::exit(1);
    });

    if let Some(icao_hex) = icao_filter {
        let icao_upper = icao_hex.to_ascii_uppercase();
        let aircraft = database.get_aircraft(&icao_upper);
        let positions = database.get_positions(&icao_upper, 50);
        let events = database.get_events(None, Some(&icao_upper), 20);

        match aircraft {
            Some(ac) => {
                println!();
                println!("Aircraft: {}", ac.icao);
                if let Some(c) = &ac.country {
                    println!("  Country: {c}");
                }
                if ac.is_military {
                    println!("  Military: yes");
                }
                println!("  First seen: {:.0}", ac.first_seen);
                println!("  Last seen: {:.0}", ac.last_seen);

                if !positions.is_empty() {
                    println!();
                    println!("  Recent positions ({}):", positions.len());
                    let mut tbl = Table::new();
                    tbl.set_header(vec!["Time", "Lat", "Lon", "Alt", "Speed", "Hdg"]);
                    for p in positions.iter().take(20) {
                        tbl.add_row(vec![
                            Cell::new(format!("{:.0}", p.timestamp)),
                            Cell::new(format!("{:.4}", p.lat)),
                            Cell::new(format!("{:.4}", p.lon)),
                            Cell::new(p.altitude_ft.map(|a| a.to_string()).unwrap_or("-".into())),
                            Cell::new(p.speed_kts.map(|s| format!("{s:.0}")).unwrap_or("-".into())),
                            Cell::new(
                                p.heading_deg
                                    .map(|h| format!("{h:.1}"))
                                    .unwrap_or("-".into()),
                            ),
                        ]);
                    }
                    println!("{tbl}");
                }

                if !events.is_empty() {
                    println!();
                    println!("  Events ({}):", events.len());
                    for e in &events {
                        println!(
                            "    [{:.0}] {}: {}",
                            e.timestamp, e.event_type, e.description
                        );
                    }
                }
            }
            None => {
                eprintln!("Aircraft {icao_upper} not found in database");
                std::process::exit(1);
            }
        }
    } else {
        let history = database.get_aircraft_history(hours);

        println!();
        println!(
            "Aircraft seen in last {hours:.0} hours: {} (database: {db_path})",
            history.len()
        );

        if history.is_empty() {
            return;
        }

        println!();
        let mut table = Table::new();
        table.set_header(vec![
            "ICAO", "Callsign", "Country", "Mil", "Min Alt", "Max Alt", "Msgs",
        ]);

        for h in &history {
            table.add_row(vec![
                Cell::new(&h.icao),
                Cell::new(h.callsign.as_deref().unwrap_or("-")),
                Cell::new(h.country.as_deref().unwrap_or("-")),
                Cell::new(if h.is_military { "Y" } else { "" }),
                Cell::new(
                    h.min_altitude_ft
                        .map(|a| a.to_string())
                        .unwrap_or("-".into()),
                ),
                Cell::new(
                    h.max_altitude_ft
                        .map(|a| a.to_string())
                        .unwrap_or("-".into()),
                ),
                Cell::new(h.message_count),
            ]);
        }

        println!("{table}");
    }
}

fn cmd_export(
    db_path: &str,
    format: &str,
    output: Option<PathBuf>,
    last_hours: Option<f64>,
    icao_filter: Option<&str>,
    limit: i64,
) {
    let database = db::Database::open(db_path).unwrap_or_else(|e| {
        eprintln!("Error opening database {db_path}: {e}");
        std::process::exit(1);
    });

    let positions = database.export_positions(last_hours, icao_filter, limit);

    let content = match format {
        "csv" => {
            let mut lines = vec![
                "icao,lat,lon,altitude_ft,speed_kts,heading_deg,vertical_rate_fpm,timestamp"
                    .to_string(),
            ];
            for p in &positions {
                lines.push(format!(
                    "{},{},{},{},{},{},{},{}",
                    p.icao,
                    p.lat,
                    p.lon,
                    p.altitude_ft.map(|a| a.to_string()).unwrap_or_default(),
                    p.speed_kts.map(|s| format!("{s:.1}")).unwrap_or_default(),
                    p.heading_deg.map(|h| format!("{h:.1}")).unwrap_or_default(),
                    p.vertical_rate_fpm
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    p.timestamp,
                ));
            }
            lines.join("\n") + "\n"
        }
        "json" => serde_json::to_string_pretty(&positions).unwrap_or("[]".into()),
        _ => {
            eprintln!("Unknown format: {format}. Use 'csv' or 'json'.");
            std::process::exit(1);
        }
    };

    match output {
        Some(path) => {
            std::fs::write(&path, &content).unwrap_or_else(|e| {
                eprintln!("Error writing {}: {e}", path.display());
                std::process::exit(1);
            });
            eprintln!(
                "Exported {} positions to {} ({})",
                positions.len(),
                path.display(),
                format
            );
        }
        None => print!("{content}"),
    }
}

fn cmd_setup() {
    use adsb_core::config;

    println!();
    println!("adsb-decode setup wizard");
    println!("========================");
    println!();

    let existing = config::load_config();
    let mut config = existing.clone();

    // Receiver name
    println!("Receiver name [{}]: ", config.receiver.name);
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let input = input.trim();
    if !input.is_empty() {
        config.receiver.name = input.to_string();
    }

    // Receiver location
    println!(
        "Receiver latitude [{}]: ",
        config
            .receiver
            .lat
            .map(|l| l.to_string())
            .unwrap_or("not set".into())
    );
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let input = input.trim();
    if !input.is_empty() {
        config.receiver.lat = input.parse().ok();
    }

    println!(
        "Receiver longitude [{}]: ",
        config
            .receiver
            .lon
            .map(|l| l.to_string())
            .unwrap_or("not set".into())
    );
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let input = input.trim();
    if !input.is_empty() {
        config.receiver.lon = input.parse().ok();
    }

    // Database path
    println!("Database path [{}]: ", config.database.path);
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let input = input.trim();
    if !input.is_empty() {
        config.database.path = input.to_string();
    }

    // Dashboard
    println!("Dashboard port [{}]: ", config.dashboard.port);
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let input = input.trim();
    if !input.is_empty() {
        if let Ok(port) = input.parse::<u16>() {
            config.dashboard.port = port;
        }
    }

    // Save
    match config::save_config(&config) {
        Ok(path) => {
            println!();
            println!("Configuration saved to {}", path.display());
            println!();
            println!(
                "  Receiver: {} ({}, {})",
                config.receiver.name,
                config
                    .receiver
                    .lat
                    .map(|l| format!("{l}"))
                    .unwrap_or("?".into()),
                config
                    .receiver
                    .lon
                    .map(|l| format!("{l}"))
                    .unwrap_or("?".into()),
            );
            println!("  Database: {}", config.database.path);
            println!(
                "  Dashboard: {}:{}",
                config.dashboard.host, config.dashboard.port
            );
        }
        Err(e) => {
            eprintln!("Error saving config: {e}");
            std::process::exit(1);
        }
    }
}

fn print_summary(aircraft: &HashMap<Icao, AircraftState>, total_frames: u64, decoded_frames: u64) {
    println!();
    println!(
        "Frames: {total_frames} parsed, {decoded_frames} decoded, {} aircraft",
        aircraft.len()
    );
    println!();

    if aircraft.is_empty() {
        return;
    }

    let mut table = Table::new();
    table.set_header(vec![
        "ICAO",
        "Callsign",
        "Squawk",
        "Alt (ft)",
        "Speed (kts)",
        "Hdg",
        "VRate",
        "Lat",
        "Lon",
        "Country",
        "Msgs",
    ]);

    let mut sorted: Vec<_> = aircraft.values().collect();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.messages));

    for ac in sorted {
        table.add_row(vec![
            Cell::new(icao_to_string(&ac.icao)),
            Cell::new(ac.callsign.as_deref().unwrap_or("-")),
            Cell::new(ac.squawk.as_deref().unwrap_or("-")),
            Cell::new(ac.altitude_ft.map(|a| a.to_string()).unwrap_or("-".into())),
            Cell::new(
                ac.speed_kts
                    .map(|s| format!("{s:.1}"))
                    .unwrap_or("-".into()),
            ),
            Cell::new(
                ac.heading_deg
                    .map(|h| format!("{h:.1}"))
                    .unwrap_or("-".into()),
            ),
            Cell::new(
                ac.vertical_rate
                    .map(|v| format!("{v:+}"))
                    .unwrap_or("-".into()),
            ),
            Cell::new(ac.lat.map(|l| format!("{l:.4}")).unwrap_or("-".into())),
            Cell::new(ac.lon.map(|l| format!("{l:.4}")).unwrap_or("-".into())),
            Cell::new(ac.country.unwrap_or("-")),
            Cell::new(ac.messages),
        ]);
    }

    println!("{table}");
}
