//! adsb-feeder: Edge device binary for ADS-B capture and demodulation.
//!
//! Supports:
//! - Demodulating raw IQ files into hex frames
//! - Reading pre-decoded hex frame files
//! - Live RTL-SDR capture via native USB (requires `native-sdr` feature)

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use adsb_core::decode;
use adsb_core::frame::{self, IcaoCache};

mod capture;

#[derive(Parser)]
#[command(
    name = "adsb-feeder",
    version,
    about = "ADS-B capture and demodulation"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Demodulate a raw IQ file into ADS-B frames
    Demod {
        /// Path to raw IQ binary file (.iq or .bin)
        file: PathBuf,

        /// Sample rate in Hz
        #[arg(long, default_value = "2000000")]
        sample_rate: u32,

        /// Parse and decode frames (not just print hex)
        #[arg(short, long)]
        decode: bool,
    },

    /// Live capture from RTL-SDR dongle (requires native-sdr feature)
    #[cfg(feature = "native-sdr")]
    Live {
        /// USB device index (0 for first dongle)
        #[arg(long, default_value = "0")]
        device: u32,

        /// Gain in tenths of dB (e.g. 400 = 40.0 dB). Omit for AGC.
        #[arg(long)]
        gain: Option<i32>,

        /// Frequency correction in PPM
        #[arg(long, default_value = "0")]
        ppm: i32,

        /// Parse and decode frames (not just print hex)
        #[arg(short, long)]
        decode: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Demod {
            file,
            sample_rate,
            decode: do_decode,
        } => cmd_demod(file, sample_rate, do_decode),
        #[cfg(feature = "native-sdr")]
        Commands::Live {
            device,
            gain,
            ppm,
            decode: do_decode,
        } => cmd_live(device, gain, ppm, do_decode),
    }
}

fn cmd_demod(file: PathBuf, sample_rate: u32, do_decode: bool) {
    let path_str = file.display().to_string();
    let reader = capture::IQReader::new(&path_str, sample_rate);

    let duration = reader.duration_seconds().unwrap_or(0.0);
    let n_samples = reader.n_samples().unwrap_or(0);

    eprintln!(
        "Demodulating: {} ({} samples, {:.1}s at {} Hz)",
        file.display(),
        n_samples,
        duration,
        sample_rate
    );

    let frames = match reader.demodulate() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    eprintln!("Found {} raw frames", frames.len());

    if do_decode {
        let mut icao_cache = IcaoCache::new(60.0);
        let mut decoded_count = 0u64;

        for raw in &frames {
            let parsed =
                frame::parse_frame(&raw.hex_str, raw.timestamp, None, false, &mut icao_cache);
            if let Some(f) = parsed {
                if let Some(msg) = decode::decode(&f) {
                    decoded_count += 1;
                    println!("{:.6} {}", raw.timestamp, raw.hex_str);
                    println!("  {:?}", msg);
                }
            }
        }
        eprintln!("{decoded_count} decoded messages");
    } else {
        for raw in &frames {
            println!(
                "{:.6} {} signal={:.0}",
                raw.timestamp, raw.hex_str, raw.signal_level
            );
        }
    }
}

#[cfg(feature = "native-sdr")]
fn cmd_live(device: u32, gain: Option<i32>, ppm: i32, do_decode: bool) {
    use adsb_core::demod::NoiseFloorTracker;

    eprintln!("Opening RTL-SDR device {device} (1090 MHz, 2 MHz sample rate)");
    if let Some(g) = gain {
        eprintln!("  Gain: {:.1} dB", g as f64 / 10.0);
    } else {
        eprintln!("  Gain: AGC");
    }

    let mut source = match capture::LiveCapture::open(device, gain, ppm) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: {e}");
            if e.kind() == std::io::ErrorKind::NotFound {
                eprintln!("Hint: Is an RTL-SDR dongle plugged in? Is librtlsdr installed?");
            }
            std::process::exit(1);
        }
    };

    eprintln!("Streaming... (Ctrl+C to stop)");

    let mut noise_tracker = NoiseFloorTracker::new();
    let mut icao_cache = IcaoCache::new(60.0);
    let mut frame_count = 0u64;
    let mut decoded_count = 0u64;

    let result = capture::demodulate_stream(
        &mut source,
        2_000_000,
        &mut noise_tracker,
        &mut |raw| {
            frame_count += 1;
            if do_decode {
                let parsed = frame::parse_frame(
                    &raw.hex_str,
                    raw.timestamp,
                    Some(raw.signal_level),
                    false,
                    &mut icao_cache,
                );
                if let Some(f) = parsed {
                    if let Some(msg) = decode::decode(&f) {
                        decoded_count += 1;
                        println!("{:.6} {}", raw.timestamp, raw.hex_str);
                        println!("  {:?}", msg);
                    }
                }
            } else {
                println!(
                    "{:.6} {} signal={:.0}",
                    raw.timestamp, raw.hex_str, raw.signal_level
                );
            }
        },
    );

    if let Err(e) = result {
        eprintln!("Stream error: {e}");
    }

    eprintln!("{frame_count} raw frames, {decoded_count} decoded");
}
