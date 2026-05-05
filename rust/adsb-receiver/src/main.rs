mod capture;
mod sender;
mod stats;

use clap::Parser;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use sender::{Sender, SenderConfig};
use stats::Stats;

#[derive(Parser, Debug)]
#[command(name = "adsb-receiver")]
#[command(
    about = "Self-contained ADS-B receiver — captures, decodes, and feeds frames to an adsb-decode server"
)]
#[command(version)]
struct Cli {
    /// Server URL (e.g., https://adsb.blueoctopustechnology.com)
    #[arg(long, env = "ADSB_SERVER")]
    server: String,

    /// Receiver name (identifies this receiver on the server)
    #[arg(long, env = "ADSB_NAME")]
    name: String,

    /// API key for authentication
    #[arg(long, env = "ADSB_API_KEY")]
    api_key: Option<String>,

    /// Receiver latitude
    #[arg(long, env = "ADSB_LAT")]
    lat: Option<f64>,

    /// Receiver longitude
    #[arg(long, env = "ADSB_LON")]
    lon: Option<f64>,

    /// Batch send interval in seconds
    #[arg(long, env = "ADSB_INTERVAL", default_value = "2.0")]
    interval: f64,

    /// RTL-SDR device index
    #[arg(long, env = "ADSB_DEVICE", default_value = "0")]
    device: u32,

    /// RTL-SDR gain (0 = auto)
    #[arg(long, env = "ADSB_GAIN")]
    gain: Option<u32>,

    /// RTL-SDR PPM correction
    #[arg(long, env = "ADSB_PPM")]
    ppm: Option<i32>,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    eprintln!("adsb-receiver v{}", env!("CARGO_PKG_VERSION"));
    eprintln!("  server: {}", cli.server);
    eprintln!("  name:   {}", cli.name);
    eprintln!(
        "  auth:   {}",
        if cli.api_key.is_some() { "yes" } else { "none" }
    );
    if let (Some(lat), Some(lon)) = (cli.lat, cli.lon) {
        eprintln!("  location: {:.4}, {:.4}", lat, lon);
    }
    eprintln!(
        "  device: {} | gain: {} | ppm: {}",
        cli.device,
        cli.gain
            .map(|g| g.to_string())
            .unwrap_or_else(|| "auto".to_string()),
        cli.ppm.unwrap_or(0)
    );
    eprintln!();

    let stats = Arc::new(Stats::new());

    // Channel from capture thread to async sender
    let (tx, rx) = mpsc::channel::<String>(2048);

    // Start capture thread
    let capture_stats = Arc::clone(&stats);
    capture::detect_capture_mode();
    let capture_handle = capture::start_rtl_adsb(cli.device, cli.gain, cli.ppm, tx, capture_stats);

    // Start sender
    let sender_config = SenderConfig {
        server_url: cli.server.trim_end_matches('/').to_string(),
        receiver_name: cli.name.clone(),
        api_key: cli.api_key.clone(),
        lat: cli.lat,
        lon: cli.lon,
        batch_interval: Duration::from_secs_f64(cli.interval),
        heartbeat_interval: Duration::from_secs(30),
    };
    let sender = Sender::new(sender_config, Arc::clone(&stats));

    // Spawn sender task
    let sender_handle = tokio::spawn(sender.run(rx));

    // Make the blocking capture-thread join awaitable so we can race it against Ctrl+C.
    let capture_join = tokio::task::spawn_blocking(move || {
        let _ = capture_handle.join();
    });

    eprintln!("[main] receiver running, press Ctrl+C to stop");

    // If rtl_adsb exits (USB transient, dongle pull, kernel re-enumerate), the parent
    // process used to keep running idle and systemd's Restart=always couldn't help.
    // Crash-only: if the capture thread ends before Ctrl+C, exit nonzero so systemd
    // respawns us and rtl_adsb starts fresh.
    let capture_died = tokio::select! {
        _ = tokio::signal::ctrl_c() => false,
        _ = capture_join => true,
    };

    if capture_died {
        eprintln!("[main] capture ended unexpectedly — exiting for systemd to restart");
    } else {
        eprintln!("\n[main] shutting down...");
    }

    // Wait for sender to flush. On capture death the channel is already closing.
    let _ = tokio::time::timeout(Duration::from_secs(5), sender_handle).await;

    eprintln!("[main] {}", stats.summary());

    if capture_died {
        std::process::exit(1);
    }
    eprintln!("[main] goodbye");
}
