//! Capture and file I/O for ADS-B data.
//!
//! Input modes:
//! - `FrameReader`:  Pre-demodulated hex frame strings (one per line)
//! - `IQReader`:     Raw IQ samples from RTL-SDR (.iq files, interleaved uint8)
//! - `demodulate_stream`: Streaming IQ demod from any `Read` source (file, pipe, stdin)

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
// Streaming IQ Demodulator
// ---------------------------------------------------------------------------

/// Read as many bytes as possible from source, returning count read.
/// Unlike `read_exact`, returns partial reads on EOF without error.
fn read_fill<R: Read>(source: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match source.read(&mut buf[total..])? {
            0 => break,
            n => total += n,
        }
    }
    Ok(total)
}

/// Demodulate a stream of raw IQ samples into ADS-B frames.
///
/// Reads interleaved uint8 IQ pairs from any `Read` source (file, pipe,
/// stdin) in 1-second chunks, overlapping by `WINDOW_SIZE` samples to
/// avoid missing frames at chunk boundaries. Each chunk goes through
/// `iq_to_magnitude()` → `demodulate_buffer()`, and discovered frames
/// are passed to the callback.
///
/// Works for both file-based (IQReader) and live streaming (rtl_sdr pipe).
pub fn demodulate_stream<R: Read>(
    source: &mut R,
    sample_rate: u32,
    noise_tracker: &mut NoiseFloorTracker,
    callback: &mut dyn FnMut(RawFrame),
) -> io::Result<()> {
    let chunk_bytes = sample_rate as usize * 2; // 1 second of IQ data
    let overlap_bytes = WINDOW_SIZE * 2;

    let mut carry: Vec<u8> = Vec::new();
    let mut sample_offset: u64 = 0;

    loop {
        // Build chunk: carry (overlap from previous) + fresh data
        let fresh_needed = chunk_bytes - carry.len();
        let mut fresh = vec![0u8; fresh_needed];
        let bytes_read = read_fill(source, &mut fresh)?;
        fresh.truncate(bytes_read);

        let mut chunk = Vec::with_capacity(carry.len() + fresh.len());
        chunk.extend_from_slice(&carry);
        chunk.extend_from_slice(&fresh);

        if chunk.len() < WINDOW_SIZE * 2 {
            break;
        }

        let mag = demod::iq_to_magnitude(&chunk);
        let chunk_time = sample_offset as f64 / sample_rate as f64;
        let frames = demod::demodulate_buffer(&mag, chunk_time, noise_tracker);
        for frame in frames {
            callback(frame);
        }

        // Save last WINDOW_SIZE samples as overlap for next chunk
        let chunk_samples = chunk.len() / 2;
        if chunk.len() >= overlap_bytes {
            carry = chunk[chunk.len() - overlap_bytes..].to_vec();
        } else {
            carry.clear();
        }

        // Advance sample offset by non-overlapping portion
        sample_offset += (chunk_samples - WINDOW_SIZE) as u64;

        if bytes_read == 0 || bytes_read < fresh_needed {
            break; // EOF
        }
    }

    Ok(())
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
        let mut file = fs::File::open(&self.path)?;
        let mut noise_tracker = NoiseFloorTracker::new();
        let mut all_frames = Vec::new();

        demodulate_stream(
            &mut file,
            self.sample_rate,
            &mut noise_tracker,
            &mut |frame| all_frames.push(frame),
        )?;

        Ok(all_frames)
    }
}

// ---------------------------------------------------------------------------
// Live RTL-SDR Capture (feature-gated)
// ---------------------------------------------------------------------------

/// Live capture from an RTL-SDR dongle via `rtlsdr_mt`.
///
/// Opens the device at `device_index`, tunes to 1090 MHz at 2 MHz sample rate,
/// and bridges the async callback to a synchronous `Read` implementation via a
/// bounded channel. Feed the resulting struct directly into `demodulate_stream`.
///
/// Requires `librtlsdr` installed on the system.
#[cfg(feature = "native-sdr")]
pub struct LiveCapture {
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
    buf_pos: usize,
    _thread: std::thread::JoinHandle<()>,
}

#[cfg(feature = "native-sdr")]
impl LiveCapture {
    /// Open RTL-SDR device and start streaming IQ samples.
    ///
    /// - `device_index`: USB device index (0 for first dongle)
    /// - `gain`: `None` for AGC, `Some(gain_tenths)` for manual (e.g., `Some(400)` = 40.0 dB)
    /// - `ppm`: Frequency correction in parts per million
    pub fn open(device_index: u32, gain: Option<i32>, ppm: i32) -> io::Result<Self> {
        let (ctl, mut reader) = rtlsdr_mt::open(device_index).map_err(|e| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("failed to open RTL-SDR device {device_index}: {e}"),
            )
        })?;

        // Configure for ADS-B: 1090 MHz, 2 MHz sample rate
        ctl.set_center_freq(1_090_000_000).map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("set_center_freq: {e}"))
        })?;
        ctl.set_sample_rate(2_000_000).map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("set_sample_rate: {e}"))
        })?;

        if ppm != 0 {
            ctl.set_ppm(ppm).map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("set_ppm: {e}"))
            })?;
        }

        if let Some(g) = gain {
            ctl.set_tuner_gain(g).map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("set_tuner_gain: {e}"))
            })?;
        } else {
            ctl.enable_agc().map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("enable_agc: {e}"))
            })?;
        }

        // Channel bridges async callback → sync Read
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);

        let thread = std::thread::spawn(move || {
            // 32 buffers of 256K each — standard RTL-SDR streaming params
            let _ = reader.read_async(32, 262_144, |bytes| {
                if tx.send(bytes.to_vec()).is_err() {
                    // Receiver dropped — stop reading
                    return;
                }
            });
        });

        Ok(LiveCapture {
            rx,
            buf: Vec::new(),
            buf_pos: 0,
            _thread: thread,
        })
    }
}

#[cfg(feature = "native-sdr")]
impl Read for LiveCapture {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        // Drain internal buffer first
        if self.buf_pos < self.buf.len() {
            let avail = self.buf.len() - self.buf_pos;
            let n = avail.min(out.len());
            out[..n].copy_from_slice(&self.buf[self.buf_pos..self.buf_pos + n]);
            self.buf_pos += n;
            return Ok(n);
        }

        // Get next chunk from device thread
        match self.rx.recv() {
            Ok(chunk) => {
                let n = chunk.len().min(out.len());
                out[..n].copy_from_slice(&chunk[..n]);
                if n < chunk.len() {
                    self.buf = chunk;
                    self.buf_pos = n;
                } else {
                    self.buf.clear();
                    self.buf_pos = 0;
                }
                Ok(n)
            }
            Err(_) => Ok(0), // Channel closed — EOF
        }
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

    #[test]
    fn test_demodulate_stream_empty() {
        let mut source = io::Cursor::new(Vec::<u8>::new());
        let mut noise_tracker = NoiseFloorTracker::new();
        let mut frames = Vec::new();

        demodulate_stream(&mut source, 2_000_000, &mut noise_tracker, &mut |f| {
            frames.push(f);
        })
        .unwrap();

        assert!(frames.is_empty());
    }

    #[test]
    fn test_demodulate_stream_noise_only() {
        // Random noise — no valid preambles, should produce no frames
        let data: Vec<u8> = (0..4_000_000u32).map(|i| (i % 256) as u8).collect();
        let mut source = io::Cursor::new(data);
        let mut noise_tracker = NoiseFloorTracker::new();
        let mut frames = Vec::new();

        demodulate_stream(&mut source, 2_000_000, &mut noise_tracker, &mut |f| {
            frames.push(f);
        })
        .unwrap();

        // Patterned data shouldn't produce valid ADS-B frames
        // (any "frames" found would fail CRC downstream)
        // Just verify it doesn't panic or infinite-loop
        assert!(frames.len() < 100); // sanity bound
    }

    #[test]
    fn test_demodulate_stream_too_small() {
        // Less than WINDOW_SIZE * 2 bytes — should gracefully return nothing
        let data = vec![128u8; WINDOW_SIZE * 2 - 2];
        let mut source = io::Cursor::new(data);
        let mut noise_tracker = NoiseFloorTracker::new();
        let mut frames = Vec::new();

        demodulate_stream(&mut source, 2_000_000, &mut noise_tracker, &mut |f| {
            frames.push(f);
        })
        .unwrap();

        assert!(frames.is_empty());
    }

    #[test]
    fn test_read_fill_partial() {
        let data = vec![1u8, 2, 3, 4, 5];
        let mut source = io::Cursor::new(data);
        let mut buf = vec![0u8; 10]; // request more than available
        let n = read_fill(&mut source, &mut buf).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf[..5], &[1, 2, 3, 4, 5]);
    }
}
