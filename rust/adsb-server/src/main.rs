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

    /// Track aircraft from a capture file with database persistence
    Track {
        /// Path to file containing hex frames (one per line)
        file: PathBuf,

        /// SQLite database path
        #[arg(long, default_value = "data/adsb.db")]
        db_path: String,

        /// Minimum seconds between stored positions per aircraft
        #[arg(long, default_value = "2.0")]
        min_interval: f64,
    },

    /// Show database statistics
    Stats {
        /// SQLite database path
        #[arg(long, default_value = "data/adsb.db")]
        db_path: String,
    },
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
                if let (
                    Some(elat),
                    Some(elon),
                    Some(ets),
                    Some(olat),
                    Some(olon),
                    Some(ots),
                ) = (
                    self.even_lat,
                    self.even_lon,
                    self.even_ts,
                    self.odd_lat,
                    self.odd_lon,
                    self.odd_ts,
                ) {
                    if let Some((lat, lon)) =
                        cpr::global_decode(elat, elon, olat, olon, ets, ots)
                    {
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

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Decode { file, raw } => cmd_decode(file, raw),
        Commands::Track {
            file,
            db_path,
            min_interval,
        } => cmd_track(file, &db_path, min_interval),
        Commands::Stats { db_path } => cmd_stats(&db_path),
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
        timestamp = ts + 0.1; // Auto-increment for files without timestamps

        let frame = match frame::parse_frame(hex_part, ts, None, true, &mut icao_cache) {
            Some(f) => f,
            None => {
                // Try without ICAO validation for standalone files
                match frame::parse_frame(hex_part, ts, None, false, &mut icao_cache) {
                    Some(f) => f,
                    None => continue,
                }
            }
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
                Cell::new(
                    ac.altitude_ft
                        .map(|a| a.to_string())
                        .unwrap_or("-".into()),
                ),
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
                Cell::new(
                    ac.lat
                        .map(|l| format!("{l:.4}"))
                        .unwrap_or("-".into()),
                ),
                Cell::new(
                    ac.lon
                        .map(|l| format!("{l:.4}"))
                        .unwrap_or("-".into()),
                ),
                Cell::new(ac.country.unwrap_or("-")),
                Cell::new(ac.message_count),
            ]);
        }

        println!("{table}");
    }
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

fn print_summary(
    aircraft: &HashMap<Icao, AircraftState>,
    total_frames: u64,
    decoded_frames: u64,
) {
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
        "ICAO", "Callsign", "Squawk", "Alt (ft)", "Speed (kts)", "Hdg", "VRate",
        "Lat", "Lon", "Country", "Msgs",
    ]);

    let mut sorted: Vec<_> = aircraft.values().collect();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.messages));

    for ac in sorted {
        table.add_row(vec![
            Cell::new(icao_to_string(&ac.icao)),
            Cell::new(ac.callsign.as_deref().unwrap_or("-")),
            Cell::new(ac.squawk.as_deref().unwrap_or("-")),
            Cell::new(
                ac.altitude_ft
                    .map(|a| a.to_string())
                    .unwrap_or("-".into()),
            ),
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
            Cell::new(
                ac.vertical_rate
                    .map(|v| format!("{v:+}"))
                    .unwrap_or("-".into()),
            ),
            Cell::new(
                ac.lat
                    .map(|l| format!("{l:.4}"))
                    .unwrap_or("-".into()),
            ),
            Cell::new(
                ac.lon
                    .map(|l| format!("{l:.4}"))
                    .unwrap_or("-".into()),
            ),
            Cell::new(ac.country.unwrap_or("-")),
            Cell::new(ac.messages),
        ]);
    }

    println!("{table}");
}
