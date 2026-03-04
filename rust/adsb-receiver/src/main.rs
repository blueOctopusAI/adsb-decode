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

    // Wait for Ctrl+C
    eprintln!("[main] receiver running, press Ctrl+C to stop");
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for Ctrl+C");
    eprintln!("\n[main] shutting down...");

    // Drop happens when capture thread's tx is dropped, which triggers sender cleanup
    // The capture thread will stop when rtl_adsb is killed or stdin closes
    // Detach capture thread — rtl_adsb child process gets killed when thread drops
    drop(capture_handle);

    // Wait for sender to flush
    let _ = tokio::time::timeout(Duration::from_secs(5), sender_handle).await;

    eprintln!("[main] {}", stats.summary());
    eprintln!("[main] goodbye");
}
