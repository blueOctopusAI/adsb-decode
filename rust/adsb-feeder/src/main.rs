//! adsb-feeder: Edge device binary for ADS-B capture and demodulation.
//!
//! Supports:
//! - Demodulating raw IQ files into hex frames
//! - Reading pre-decoded hex frame files
//!
//! Live RTL-SDR capture will be added with `rtlsdr_mt` integration.

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
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Demod {
            file,
            sample_rate,
            decode: do_decode,
        } => cmd_demod(file, sample_rate, do_decode),
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
