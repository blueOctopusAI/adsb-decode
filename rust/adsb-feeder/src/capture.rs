//! Capture and file I/O for ADS-B data.
//!
//! Input modes:
//! - `FrameReader`:  Pre-demodulated hex frame strings (one per line)
//! - `IQReader`:     Raw IQ samples from RTL-SDR (.iq files, interleaved uint8)
//!
//! Live RTL-SDR capture will be added when `rtlsdr_mt` integration is done.

#![allow(dead_code)]

use std::fs;
use std::io::{self, Read};

use adsb_core::demod::{self, NoiseFloorTracker, RawFrame, WINDOW_SIZE};

// ---------------------------------------------------------------------------
// Hex Frame Reader
// ---------------------------------------------------------------------------

/// Read pre-demodulated hex frames from a file.
///
/// Accepts hex strings from tools like rtl_adsb, dump1090 --raw, or
/// any source that produces one hex frame per line.
pub struct FrameReader {
    path: String,
}

impl FrameReader {
    pub fn new(path: &str) -> Self {
        FrameReader {
            path: path.to_string(),
        }
    }

    /// Read all frames from the file.
    pub fn read_all(&self) -> io::Result<Vec<RawFrame>> {
        let content = fs::read_to_string(&self.path)?;
        let mut frames = Vec::new();
        let t0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();

        for (i, line) in content.lines().enumerate() {
            if let Some(hex) = clean_hex_line(line) {
                frames.push(RawFrame {
                    hex_str: hex,
                    timestamp: t0 + i as f64 * 0.001,
                    signal_level: 0.0,
                });
            }
        }

        Ok(frames)
    }
}

/// Extract a valid Mode S hex string from a line.
///
/// Handles plain hex, dump1090 format (`*hex;`), and whitespace.
pub fn clean_hex_line(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    // Try dump1090 format: *<hex>;
    if line.starts_with('*') && line.ends_with(';') {
        let inner = &line[1..line.len() - 1];
        if is_valid_hex(inner) {
            return Some(inner.to_ascii_uppercase());
        }
    }

    // Try plain hex
    if is_valid_hex(line) {
        return Some(line.to_ascii_uppercase());
    }

    None
}

fn is_valid_hex(s: &str) -> bool {
    (s.len() == 14 || s.len() == 28) && s.chars().all(|c| c.is_ascii_hexdigit())
}

// ---------------------------------------------------------------------------
// IQ File Reader
// ---------------------------------------------------------------------------

/// Read raw IQ samples from a binary file and demodulate.
///
/// RTL-SDR produces interleaved unsigned 8-bit IQ pairs:
/// `[I0, Q0, I1, Q1, I2, Q2, ...]`
pub struct IQReader {
    path: String,
    sample_rate: u32,
}

impl IQReader {
    pub fn new(path: &str, sample_rate: u32) -> Self {
        IQReader {
            path: path.to_string(),
            sample_rate,
        }
    }

    /// File size in bytes.
    pub fn file_size(&self) -> io::Result<u64> {
        Ok(fs::metadata(&self.path)?.len())
    }

    /// Number of IQ sample pairs.
    pub fn n_samples(&self) -> io::Result<u64> {
        Ok(self.file_size()? / 2)
    }

    /// Duration of the recording in seconds.
    pub fn duration_seconds(&self) -> io::Result<f64> {
        Ok(self.n_samples()? as f64 / self.sample_rate as f64)
    }

    /// Demodulate the entire IQ file into ADS-B frames.
    ///
    /// Reads in chunks to manage memory. Each chunk overlaps the
    /// previous by WINDOW_SIZE samples to avoid missing frames.
    pub fn demodulate(&self) -> io::Result<Vec<RawFrame>> {
        let file_size = self.file_size()? as usize;
        let total_samples = file_size / 2;
        let chunk_samples = self.sample_rate as usize; // 1 second per chunk

        let mut all_frames = Vec::new();
        let mut noise_tracker = NoiseFloorTracker::new();
        let overlap = WINDOW_SIZE;
        let mut offset = 0usize;

        let mut file = fs::File::open(&self.path)?;

        while offset < total_samples {
            let byte_offset = offset * 2;
            let byte_count = (chunk_samples * 2).min(file_size - byte_offset);

            if byte_count < WINDOW_SIZE * 2 {
                break;
            }

            let mut raw = vec![0u8; byte_count];
            // Seek to position and read
            use std::io::Seek;
            file.seek(io::SeekFrom::Start(byte_offset as u64))?;
            file.read_exact(&mut raw)?;

            let mag = demod::iq_to_magnitude(&raw);
            let chunk_time = offset as f64 / self.sample_rate as f64;
            let frames = demod::demodulate_buffer(&mag, chunk_time, &mut noise_tracker);
            all_frames.extend(frames);

            offset += chunk_samples - overlap;
        }

        Ok(all_frames)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_hex_line_plain() {
        let result = clean_hex_line("8D4840D6202CC371C32CE0576098");
        assert_eq!(result.as_deref(), Some("8D4840D6202CC371C32CE0576098"));
    }

    #[test]
    fn test_clean_hex_line_dump1090() {
        let result = clean_hex_line("*8D4840D6202CC371C32CE0576098;");
        assert_eq!(result.as_deref(), Some("8D4840D6202CC371C32CE0576098"));
    }

    #[test]
    fn test_clean_hex_line_lowercase() {
        let result = clean_hex_line("8d4840d6202cc371c32ce0576098");
        assert_eq!(result.as_deref(), Some("8D4840D6202CC371C32CE0576098"));
    }

    #[test]
    fn test_clean_hex_line_whitespace() {
        let result = clean_hex_line("  8D4840D6202CC371C32CE0576098  ");
        assert_eq!(result.as_deref(), Some("8D4840D6202CC371C32CE0576098"));
    }

    #[test]
    fn test_clean_hex_line_comment() {
        assert!(clean_hex_line("# comment").is_none());
    }

    #[test]
    fn test_clean_hex_line_empty() {
        assert!(clean_hex_line("").is_none());
        assert!(clean_hex_line("  ").is_none());
    }

    #[test]
    fn test_clean_hex_line_invalid() {
        assert!(clean_hex_line("not hex at all").is_none());
        assert!(clean_hex_line("8D4840").is_none()); // too short
    }

    #[test]
    fn test_clean_hex_line_short_frame() {
        // 14 chars = 56-bit short frame
        let result = clean_hex_line("02E197C845AC82");
        assert_eq!(result.as_deref(), Some("02E197C845AC82"));
    }

    #[test]
    fn test_is_valid_hex() {
        assert!(is_valid_hex("8D4840D6202CC371C32CE0576098")); // 28 chars
        assert!(is_valid_hex("02E197C845AC82")); // 14 chars
        assert!(!is_valid_hex("8D4840")); // wrong length
        assert!(!is_valid_hex("ZZZZZZZZZZZZZZ")); // invalid chars
    }
}
