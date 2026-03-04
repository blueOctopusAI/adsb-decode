use std::io::BufRead;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::stats::Stats;

/// Parse a line from rtl_adsb output.
/// rtl_adsb outputs lines like: *8D4840D6202CC371C32CE0576098;
/// Returns the hex string without the * prefix and ; suffix.
pub fn parse_rtl_adsb_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with('*') && trimmed.ends_with(';') {
        let hex = &trimmed[1..trimmed.len() - 1];
        if !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(hex.to_uppercase());
        }
    }
    None
}

/// Capture mode selection
pub enum CaptureMode {
    /// Use rtl_adsb subprocess (always available)
    RtlAdsb,
    /// Native SDR via adsb-feeder's LiveCapture (requires native-sdr feature)
    #[cfg(feature = "native-sdr")]
    NativeSdr,
}

/// Start capture using rtl_adsb subprocess.
/// Spawns the process and sends decoded hex frames over the channel.
pub fn start_rtl_adsb(
    device_index: u32,
    gain: Option<u32>,
    ppm: Option<i32>,
    tx: mpsc::Sender<String>,
    stats: Arc<Stats>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut cmd = Command::new("rtl_adsb");
        cmd.arg("-d").arg(device_index.to_string());
        if let Some(g) = gain {
            cmd.arg("-g").arg(g.to_string());
        }
        if let Some(p) = ppm {
            cmd.arg("-p").arg(p.to_string());
        }
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        eprintln!("[capture] starting rtl_adsb (device {})", device_index);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[capture] failed to start rtl_adsb: {}", e);
                eprintln!("[capture] make sure rtl-sdr is installed: sudo apt install rtl-sdr");
                return;
            }
        };

        let stdout = child.stdout.take().expect("failed to capture stdout");
        let reader = std::io::BufReader::new(stdout);

        for line in reader.lines() {
            match line {
                Ok(l) => {
                    if let Some(hex) = parse_rtl_adsb_line(&l) {
                        stats.frames_captured.fetch_add(1, Ordering::Relaxed);
                        if tx.blocking_send(hex).is_err() {
                            eprintln!("[capture] channel closed, stopping capture");
                            break;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[capture] read error: {}", e);
                    break;
                }
            }
        }

        let _ = child.kill();
        let _ = child.wait();
        eprintln!("[capture] rtl_adsb process ended");
    })
}

/// Detect which capture mode is available.
/// Try native SDR first (if compiled with feature), fall back to rtl_adsb.
pub fn detect_capture_mode() -> CaptureMode {
    #[cfg(feature = "native-sdr")]
    {
        // Try to detect RTL-SDR device directly
        // For now, always prefer rtl_adsb for stability
        // Native SDR support will be added once the capture bridge is proven
        eprintln!("[capture] native-sdr feature enabled but using rtl_adsb for stability");
    }

    // Check if rtl_adsb is available
    match Command::new("rtl_adsb")
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(_) => {
            eprintln!("[capture] using rtl_adsb capture mode");
            CaptureMode::RtlAdsb
        }
        Err(_) => {
            eprintln!("[capture] WARNING: rtl_adsb not found in PATH");
            eprintln!("[capture] install with: sudo apt install rtl-sdr");
            // Still return RtlAdsb — will fail with clear error on start
            CaptureMode::RtlAdsb
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_frame() {
        assert_eq!(
            parse_rtl_adsb_line("*8D4840D6202CC371C32CE0576098;"),
            Some("8D4840D6202CC371C32CE0576098".to_string())
        );
    }

    #[test]
    fn test_parse_lowercase() {
        assert_eq!(
            parse_rtl_adsb_line("*8d4840d6202cc371c32ce0576098;"),
            Some("8D4840D6202CC371C32CE0576098".to_string())
        );
    }

    #[test]
    fn test_parse_with_whitespace() {
        assert_eq!(
            parse_rtl_adsb_line("  *AABBCCDD11223344AABBCCDD1122;  \n"),
            Some("AABBCCDD11223344AABBCCDD1122".to_string())
        );
    }

    #[test]
    fn test_parse_empty() {
        assert_eq!(parse_rtl_adsb_line(""), None);
    }

    #[test]
    fn test_parse_no_star() {
        assert_eq!(parse_rtl_adsb_line("8D4840D6202CC371C32CE0576098;"), None);
    }

    #[test]
    fn test_parse_no_semicolon() {
        assert_eq!(parse_rtl_adsb_line("*8D4840D6202CC371C32CE0576098"), None);
    }

    #[test]
    fn test_parse_invalid_hex() {
        assert_eq!(parse_rtl_adsb_line("*GHIJKL;"), None);
    }

    #[test]
    fn test_parse_empty_between_markers() {
        assert_eq!(parse_rtl_adsb_line("*;"), None);
    }

    #[test]
    fn test_parse_short_frame() {
        assert_eq!(
            parse_rtl_adsb_line("*5DA96C5C5F7F;"),
            Some("5DA96C5C5F7F".to_string())
        );
    }

    #[test]
    fn test_parse_long_frame() {
        let long = format!("*{};", "A".repeat(28));
        assert_eq!(parse_rtl_adsb_line(&long), Some("A".repeat(28)));
    }
}
