mod capture;
mod sender;
mod stats;
mod watchdog;

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

    /// Frame-stall watchdog: exit nonzero if no frame has been captured for
    /// this many seconds, so systemd restarts the agent and rtl_adsb reopens
    /// the dongle. 0 disables. Default 600 (10 min) — a working receiver sees
    /// frames every second even at night; see watchdog.rs for the reasoning.
    #[arg(long, env = "ADSB_STALL_TIMEOUT", default_value_t = watchdog::DEFAULT_STALL_TIMEOUT_SECS)]
    stall_timeout: f64,
}

/// Exit status when the frame-stall watchdog trips. Distinct from the
/// capture-died exit (1) so `systemctl status` / journal show which path fired.
const EXIT_STALLED: i32 = 2;

/// Why the main loop stopped waiting.
#[derive(Debug, PartialEq, Eq)]
enum Stop {
    /// Ctrl+C / SIGINT — orderly shutdown, exit 0.
    Signal,
    /// rtl_adsb exited (USB transient, dongle pull, kernel re-enumerate).
    CaptureDied,
    /// rtl_adsb alive but silent for the stall window.
    Stalled,
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
    eprintln!(
        "  stall timeout: {}",
        if cli.stall_timeout > 0.0 {
            format!("{:.0}s", cli.stall_timeout)
        } else {
            "disabled".to_string()
        }
    );
    eprintln!();

    let stats = Arc::new(Stats::new());

    // Channel from capture thread to async sender
    let (tx, rx) = mpsc::channel::<String>(2048);

    // Start capture thread
    let capture_stats = Arc::clone(&stats);
    capture::detect_capture_mode();
    let child_slot = capture::new_child_slot();
    let capture_handle = capture::start_rtl_adsb(
        cli.device,
        cli.gain,
        cli.ppm,
        tx,
        capture_stats,
        Arc::clone(&child_slot),
    );

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

    // Frame-stall watchdog. Pending forever when disabled, so the select! below
    // needs no special case.
    let stall_window = Duration::from_secs_f64(cli.stall_timeout.max(0.0));
    let watchdog_run = watchdog::run(Arc::clone(&stats), stall_window);

    eprintln!("[main] receiver running, press Ctrl+C to stop");

    // Crash-only, two triggers:
    //  1. rtl_adsb exits (USB transient, dongle pull, kernel re-enumerate) —
    //     the capture thread ends; exit nonzero so systemd respawns us.
    //  2. rtl_adsb is alive but has produced no frame for the stall window —
    //     the 2026-10-05 failure, where a dongle re-enumerated at full speed
    //     and rtl_adsb ran silently for 23 days. Kill it and exit nonzero.
    // Either way systemd's Restart=always starts a fresh rtl_adsb against the device.
    let stop = tokio::select! {
        _ = tokio::signal::ctrl_c() => Stop::Signal,
        _ = capture_join => Stop::CaptureDied,
        verdict = watchdog_run => {
            eprintln!("[watchdog] {}", watchdog::stall_message(&verdict));
            Stop::Stalled
        }
    };

    match stop {
        Stop::Signal => eprintln!("\n[main] shutting down..."),
        Stop::CaptureDied => {
            eprintln!("[main] capture ended unexpectedly — exiting for systemd to restart")
        }
        Stop::Stalled => {
            // Closing rtl_adsb closes the channel, which lets the sender flush.
            capture::kill_child(&child_slot);
            eprintln!("[main] frame stall — exiting for systemd to restart");
        }
    }

    // Wait for sender to flush. On capture death the channel is already closing.
    let _ = tokio::time::timeout(Duration::from_secs(5), sender_handle).await;

    eprintln!("[main] {}", stats.summary());

    match stop {
        Stop::CaptureDied => std::process::exit(1),
        Stop::Stalled => std::process::exit(EXIT_STALLED),
        Stop::Signal => eprintln!("[main] goodbye"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_stall_timeout_defaults_to_ten_minutes() {
        let cli =
            Cli::try_parse_from(["adsb-receiver", "--server", "http://h", "--name", "rx"]).unwrap();
        assert_eq!(cli.stall_timeout, watchdog::DEFAULT_STALL_TIMEOUT_SECS);
        assert_eq!(cli.stall_timeout, 600.0);
    }

    #[test]
    fn cli_stall_timeout_flag_overrides_default() {
        let cli = Cli::try_parse_from([
            "adsb-receiver",
            "--server",
            "http://h",
            "--name",
            "rx",
            "--stall-timeout",
            "120",
        ])
        .unwrap();
        assert_eq!(cli.stall_timeout, 120.0);
    }

    #[test]
    fn cli_stall_timeout_zero_disables() {
        let cli = Cli::try_parse_from([
            "adsb-receiver",
            "--server",
            "http://h",
            "--name",
            "rx",
            "--stall-timeout",
            "0",
        ])
        .unwrap();
        assert_eq!(cli.stall_timeout, 0.0);
    }
}
