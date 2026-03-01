//! IQ sample demodulation — convert raw radio samples to ADS-B bitstreams.
//!
//! Pipeline:
//! 1. IQ to magnitude: lookup table for (I-127.5)² + (Q-127.5)² per sample pair
//! 2. Preamble detection: slide window with strict ordering, quiet zone, SNR check
//! 3. Bit recovery: PPM with continuity check for weak transitions
//! 4. Adaptive signal threshold: noise floor tracking via EMA
//!
//! At 2 MHz sample rate:
//! - 1 bit = 2 samples (1 µs per bit)
//! - Preamble = 16 samples (8 µs)
//! - Short message (56 bits) = 112 samples after preamble
//! - Long message (112 bits) = 224 samples after preamble
//! - Total window for long message = 16 + 224 = 240 samples

use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SAMPLES_PER_BIT: usize = 2;
const PREAMBLE_SAMPLES: usize = 16;
pub const SHORT_MSG_BITS: usize = 56;
pub const LONG_MSG_BITS: usize = 112;
const SHORT_MSG_SAMPLES: usize = SHORT_MSG_BITS * SAMPLES_PER_BIT; // 112
const LONG_MSG_SAMPLES: usize = LONG_MSG_BITS * SAMPLES_PER_BIT; // 224

/// Total window needed: preamble + longest message.
pub const WINDOW_SIZE: usize = PREAMBLE_SAMPLES + LONG_MSG_SAMPLES; // 240

/// Preamble pulse positions in samples (at 2 MHz):
/// Pulses at 0, 1, 3.5, 4.5 µs → samples 0, 2, 7, 9.
const PULSE_POSITIONS: [usize; 4] = [0, 2, 7, 9];
/// Gap positions (should be low energy).
const GAP_POSITIONS: [usize; 6] = [1, 3, 4, 5, 6, 8];
/// Quiet zone: samples 10-15 (post-preamble, pre-data) should be low.
const QUIET_ZONE_POSITIONS: [usize; 6] = [10, 11, 12, 13, 14, 15];

/// Minimum ratio of pulse energy to gap energy for valid preamble.
const MIN_PREAMBLE_RATIO: f32 = 2.0;

/// Minimum signal level (squared magnitude) to consider a preamble.
const MIN_SIGNAL_LEVEL: f32 = 100.0;

/// SNR threshold: signal * 2 >= 3 * noise (3.5 dB minimum).
const SNR_SIGNAL_FACTOR: f32 = 2.0;
const SNR_NOISE_FACTOR: f32 = 3.0;

/// Bit recovery: minimum magnitude delta to make a confident bit decision.
const BIT_DELTA_THRESHOLD: f32 = 0.15;

/// Maximum fraction of uncertain bits before rejecting a frame.
const MAX_UNCERTAIN_RATIO: f32 = 0.20;

/// Adaptive threshold: noise floor EMA decay factor.
const NOISE_FLOOR_ALPHA: f32 = 0.05;
/// Multiplier applied to noise floor to get adaptive threshold.
const SNR_ADAPTIVE_FACTOR: f32 = 3.0;
/// Absolute floor — adaptive threshold can never go below this.
const MIN_ADAPTIVE_LEVEL: f32 = 50.0;

/// Valid downlink formats for long messages (112 bits).
const LONG_DFS: [u8; 6] = [16, 17, 18, 19, 20, 21];
/// Valid downlink formats for short messages (56 bits).
const SHORT_DFS: [u8; 4] = [0, 4, 5, 11];

// ---------------------------------------------------------------------------
// Magnitude Lookup Table
// ---------------------------------------------------------------------------

/// Pre-computed squared magnitude for all 256×256 IQ combinations.
/// `MAG_LUT[i * 256 + q] = (i - 127.5)² + (q - 127.5)²`
static MAG_LUT: LazyLock<Vec<f32>> = LazyLock::new(|| {
    let mut lut = vec![0.0f32; 256 * 256];
    for i in 0..256u32 {
        let iv = i as f32 - 127.5;
        let i_sq = iv * iv;
        for q in 0..256u32 {
            let qv = q as f32 - 127.5;
            lut[(i * 256 + q) as usize] = i_sq + qv * qv;
        }
    }
    lut
});

/// Convert interleaved uint8 IQ pairs to squared magnitude.
///
/// Input: flat slice `[I0, Q0, I1, Q1, ...]` from RTL-SDR.
/// Output: one f32 per sample pair.
pub fn iq_to_magnitude(raw: &[u8]) -> Vec<f32> {
    let n = raw.len() / 2;
    let lut = &*MAG_LUT;
    let mut mag = Vec::with_capacity(n);
    for i in 0..n {
        let idx = raw[i * 2] as usize * 256 + raw[i * 2 + 1] as usize;
        mag.push(lut[idx]);
    }
    mag
}

// ---------------------------------------------------------------------------
// Adaptive Noise Floor Tracker
// ---------------------------------------------------------------------------

/// Tracks noise floor via exponential moving average of local medians.
pub struct NoiseFloorTracker {
    noise_floor: f32,
}

impl NoiseFloorTracker {
    pub fn new() -> Self {
        NoiseFloorTracker {
            noise_floor: MIN_SIGNAL_LEVEL,
        }
    }

    /// Current adaptive threshold: max(noise_floor * factor, absolute minimum).
    pub fn threshold(&self) -> f32 {
        (self.noise_floor * SNR_ADAPTIVE_FACTOR).max(MIN_ADAPTIVE_LEVEL)
    }

    /// Update noise floor estimate from a magnitude buffer.
    pub fn update(&mut self, mag: &[f32]) {
        if mag.len() < 100 {
            return;
        }
        // Sample 64 evenly spaced windows of 16 samples, take median of each
        let step = (mag.len() / 64).max(1);
        let mut medians = Vec::with_capacity(64);
        let mut i = 0;
        while i + 16 <= mag.len() {
            let mut window: Vec<f32> = mag[i..i + 16].to_vec();
            window.sort_by(|a, b| a.partial_cmp(b).unwrap());
            medians.push(window[8]); // median of 16 = element at index 8
            i += step;
        }
        if medians.is_empty() {
            return;
        }
        // Use the 25th percentile of medians as noise estimate
        medians.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let idx = medians.len() / 4;
        let local_noise = medians[idx];
        self.noise_floor =
            (1.0 - NOISE_FLOOR_ALPHA) * self.noise_floor + NOISE_FLOOR_ALPHA * local_noise;
    }

    pub fn reset(&mut self) {
        self.noise_floor = MIN_SIGNAL_LEVEL;
    }
}

impl Default for NoiseFloorTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Preamble Detection
// ---------------------------------------------------------------------------

/// Check if a valid ADS-B preamble starts at position `pos`.
///
/// Returns signal level (average pulse magnitude) if valid, None otherwise.
///
/// Checks:
/// 1. Minimum signal level (adaptive or provided)
/// 2. Pulse-to-gap ratio ≥ 2.0
/// 3. Pulse amplitude consistency (max ≤ 6× min)
/// 4. Strict ordering: each pulse exceeds adjacent gaps
/// 5. Quiet zone: samples 10-15 below 2/3 of pulse average
/// 6. SNR: signal × 2 ≥ 3 × noise (3.5 dB minimum)
pub fn check_preamble(mag: &[f32], pos: usize, min_level: Option<f32>) -> Option<f32> {
    if pos + WINDOW_SIZE > mag.len() {
        return None;
    }

    let effective_min = min_level.unwrap_or(MIN_ADAPTIVE_LEVEL);

    let pulse_values: [f32; 4] = [
        mag[pos + PULSE_POSITIONS[0]],
        mag[pos + PULSE_POSITIONS[1]],
        mag[pos + PULSE_POSITIONS[2]],
        mag[pos + PULSE_POSITIONS[3]],
    ];
    let gap_values: [f32; 6] = [
        mag[pos + GAP_POSITIONS[0]],
        mag[pos + GAP_POSITIONS[1]],
        mag[pos + GAP_POSITIONS[2]],
        mag[pos + GAP_POSITIONS[3]],
        mag[pos + GAP_POSITIONS[4]],
        mag[pos + GAP_POSITIONS[5]],
    ];

    let pulse_avg = pulse_values.iter().sum::<f32>() / 4.0;
    let gap_sum: f32 = gap_values.iter().sum();
    let gap_avg = if gap_sum > 0.0 { gap_sum / 6.0 } else { 0.001 };

    if pulse_avg < effective_min {
        return None;
    }

    if pulse_avg / gap_avg < MIN_PREAMBLE_RATIO {
        return None;
    }

    // All pulses should be roughly similar amplitude
    let pulse_min = pulse_values.iter().cloned().fold(f32::INFINITY, f32::min);
    let pulse_max = pulse_values
        .iter()
        .cloned()
        .fold(f32::NEG_INFINITY, f32::max);
    if pulse_max > 6.0 * pulse_min {
        return None;
    }

    // Strict ordering — each pulse must exceed its adjacent gaps
    if pulse_values[0] <= gap_values[0] {
        return None;
    }
    if pulse_values[1] <= gap_values[0] || pulse_values[1] <= gap_values[2] {
        return None;
    }
    if pulse_values[2] <= gap_values[4] {
        return None;
    }
    if pulse_values[3] <= gap_values[5] {
        return None;
    }

    // Quiet zone — samples 10-15 should be low (< 2/3 pulse average)
    let quiet_limit = pulse_avg * (2.0 / 3.0);
    for &qp in &QUIET_ZONE_POSITIONS {
        if mag[pos + qp] > quiet_limit {
            return None;
        }
    }

    // SNR check — 3.5 dB minimum
    if pulse_avg * SNR_SIGNAL_FACTOR < SNR_NOISE_FACTOR * gap_avg {
        return None;
    }

    Some(pulse_avg)
}

// ---------------------------------------------------------------------------
// Bit Recovery
// ---------------------------------------------------------------------------

/// Recover bits from magnitude signal using PPM decoding with continuity check.
///
/// Each bit occupies 2 samples (1 µs at 2 MHz). Pulse Position Modulation:
/// - Bit '1': energy in first sample > energy in second sample
/// - Bit '0': energy in second sample >= energy in first sample
///
/// Returns (bits, uncertain_count).
pub fn recover_bits(mag: &[f32], pos: usize, n_bits: usize) -> (Vec<u8>, usize) {
    let mut bits = Vec::with_capacity(n_bits);
    let mut uncertain_count = 0usize;
    let mut prev_bit = 0u8;

    for i in 0..n_bits {
        let sample_pos = pos + i * SAMPLES_PER_BIT;
        if sample_pos + 1 >= mag.len() {
            break;
        }

        let high = mag[sample_pos];
        let low = mag[sample_pos + 1];
        let signal = high.max(low);

        let bit = if signal > 0.0 && (high - low).abs() / signal < BIT_DELTA_THRESHOLD {
            // Weak transition — use previous bit value (continuity)
            uncertain_count += 1;
            prev_bit
        } else if high > low {
            1
        } else {
            0
        };

        bits.push(bit);
        prev_bit = bit;
    }

    (bits, uncertain_count)
}

/// Convert bit slice to uppercase hex string.
pub fn bits_to_hex(bits: &[u8]) -> String {
    let mut hex = String::with_capacity(bits.len() / 4);
    for chunk in bits.chunks(4) {
        if chunk.len() < 4 {
            break;
        }
        let value = (chunk[0] << 3) | (chunk[1] << 2) | (chunk[2] << 1) | chunk[3];
        hex.push(
            char::from_digit(value as u32, 16)
                .unwrap()
                .to_ascii_uppercase(),
        );
    }
    hex
}

// ---------------------------------------------------------------------------
// Demodulate Buffer
// ---------------------------------------------------------------------------

/// A raw demodulated frame before CRC/parse validation.
#[derive(Debug, Clone)]
pub struct RawFrame {
    pub hex_str: String,
    pub timestamp: f64,
    pub signal_level: f32,
}

/// Scan a magnitude buffer for ADS-B messages.
///
/// Slides through the buffer looking for valid preambles, then recovers
/// bits with confidence tracking and produces hex frame strings.
pub fn demodulate_buffer(
    mag: &[f32],
    timestamp: f64,
    noise_tracker: &mut NoiseFloorTracker,
) -> Vec<RawFrame> {
    noise_tracker.update(mag);
    let threshold = noise_tracker.threshold();

    let mut frames = Vec::new();
    let sample_rate = 2_000_000.0f64;
    let mut i = 0;

    while i + WINDOW_SIZE <= mag.len() {
        let signal_level = match check_preamble(mag, i, Some(threshold)) {
            Some(s) => s,
            None => {
                i += 1;
                continue;
            }
        };

        let msg_start = i + PREAMBLE_SAMPLES;

        // Try long message first (112 bits)
        if msg_start + LONG_MSG_SAMPLES <= mag.len() {
            let (bits, uncertain) = recover_bits(mag, msg_start, LONG_MSG_BITS);
            if bits.len() == LONG_MSG_BITS
                && (uncertain as f32) / (LONG_MSG_BITS as f32) <= MAX_UNCERTAIN_RATIO
            {
                let hex_str = bits_to_hex(&bits);
                if hex_str.len() == 28 {
                    let df =
                        (bits[0] << 4) | (bits[1] << 3) | (bits[2] << 2) | (bits[3] << 1) | bits[4];
                    if LONG_DFS.contains(&df) {
                        let frame_time = timestamp + i as f64 / sample_rate;
                        frames.push(RawFrame {
                            hex_str,
                            timestamp: frame_time,
                            signal_level,
                        });
                        i = msg_start + LONG_MSG_SAMPLES;
                        continue;
                    }
                }
            }
        }

        // Try short message (56 bits)
        if msg_start + SHORT_MSG_SAMPLES <= mag.len() {
            let (bits, uncertain) = recover_bits(mag, msg_start, SHORT_MSG_BITS);
            if bits.len() == SHORT_MSG_BITS
                && (uncertain as f32) / (SHORT_MSG_BITS as f32) <= MAX_UNCERTAIN_RATIO
            {
                let hex_str = bits_to_hex(&bits);
                if hex_str.len() == 14 {
                    let df =
                        (bits[0] << 4) | (bits[1] << 3) | (bits[2] << 2) | (bits[3] << 1) | bits[4];
                    if SHORT_DFS.contains(&df) {
                        let frame_time = timestamp + i as f64 / sample_rate;
                        frames.push(RawFrame {
                            hex_str,
                            timestamp: frame_time,
                            signal_level,
                        });
                        i = msg_start + SHORT_MSG_SAMPLES;
                        continue;
                    }
                }
            }
        }

        // Not a valid message — advance past false preamble
        i += 1;
    }

    frames
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mag_lut_center() {
        // At center (127, 128): (127-127.5)² + (128-127.5)² = 0.25 + 0.25 = 0.5
        let lut = &*MAG_LUT;
        let val = lut[127 * 256 + 128];
        assert!(
            (val - 0.5).abs() < 0.01,
            "Center value should be ~0.5, got {val}"
        );
    }

    #[test]
    fn test_mag_lut_corner() {
        // At (0, 0): (0-127.5)² + (0-127.5)² = 16256.25 + 16256.25 = 32512.5
        let lut = &*MAG_LUT;
        let val = lut[0];
        assert!(
            (val - 32512.5).abs() < 1.0,
            "Corner value should be ~32512.5, got {val}"
        );
    }

    #[test]
    fn test_mag_lut_opposite_corner() {
        // At (255, 255): (255-127.5)² + (255-127.5)² = same as (0,0)
        let lut = &*MAG_LUT;
        assert!((lut[255 * 256 + 255] - lut[0]).abs() < 0.01);
    }

    #[test]
    fn test_iq_to_magnitude_basic() {
        // 4 bytes = 2 IQ pairs
        let raw = [127u8, 128, 0, 0];
        let mag = iq_to_magnitude(&raw);
        assert_eq!(mag.len(), 2);
        assert!((mag[0] - 0.5).abs() < 0.01); // center
        assert!((mag[1] - 32512.5).abs() < 1.0); // corner
    }

    #[test]
    fn test_iq_to_magnitude_length() {
        let raw = vec![128u8; 200]; // 100 IQ pairs
        let mag = iq_to_magnitude(&raw);
        assert_eq!(mag.len(), 100);
    }

    #[test]
    fn test_bits_to_hex_simple() {
        // 0x8D = 10001101
        let bits = vec![1, 0, 0, 0, 1, 1, 0, 1];
        assert_eq!(bits_to_hex(&bits), "8D");
    }

    #[test]
    fn test_bits_to_hex_full_byte() {
        let bits = vec![1, 1, 1, 1, 0, 0, 0, 0];
        assert_eq!(bits_to_hex(&bits), "F0");
    }

    #[test]
    fn test_bits_to_hex_zero() {
        let bits = vec![0, 0, 0, 0];
        assert_eq!(bits_to_hex(&bits), "0");
    }

    #[test]
    fn test_recover_bits_clear_signal() {
        // Simulate clear 1-0-1-0 pattern
        // Bit 1: high sample > low sample
        // Bit 0: low sample > high sample
        let mut mag = vec![0.0f32; 20];
        // Bit 1: mag[0]=1000, mag[1]=100
        mag[0] = 1000.0;
        mag[1] = 100.0;
        // Bit 0: mag[2]=100, mag[3]=1000
        mag[2] = 100.0;
        mag[3] = 1000.0;
        // Bit 1: mag[4]=1000, mag[5]=100
        mag[4] = 1000.0;
        mag[5] = 100.0;
        // Bit 0: mag[6]=100, mag[7]=1000
        mag[6] = 100.0;
        mag[7] = 1000.0;

        let (bits, uncertain) = recover_bits(&mag, 0, 4);
        assert_eq!(bits, vec![1, 0, 1, 0]);
        assert_eq!(uncertain, 0);
    }

    #[test]
    fn test_recover_bits_weak_transition() {
        // Simulate weak transition where delta/signal < BIT_DELTA_THRESHOLD
        let mut mag = vec![0.0f32; 10];
        // Clear bit 1 first
        mag[0] = 1000.0;
        mag[1] = 100.0;
        // Weak transition: both nearly equal
        mag[2] = 500.0;
        mag[3] = 495.0; // delta/signal = 5/500 = 0.01 < 0.15

        let (bits, uncertain) = recover_bits(&mag, 0, 2);
        assert_eq!(bits[0], 1); // clear
        assert_eq!(bits[1], 1); // continuity from prev_bit
        assert_eq!(uncertain, 1);
    }

    #[test]
    fn test_check_preamble_no_signal() {
        let mag = vec![0.0f32; WINDOW_SIZE + 10];
        assert!(check_preamble(&mag, 0, Some(MIN_SIGNAL_LEVEL)).is_none());
    }

    #[test]
    fn test_check_preamble_valid() {
        // Build a synthetic preamble
        let mut mag = vec![10.0f32; WINDOW_SIZE + 10];

        // Set pulse positions high, gap positions low
        for &p in &PULSE_POSITIONS {
            mag[p] = 1000.0;
        }
        for &g in &GAP_POSITIONS {
            mag[g] = 50.0;
        }
        // Quiet zone must be low
        for &q in &QUIET_ZONE_POSITIONS {
            mag[q] = 50.0;
        }

        let result = check_preamble(&mag, 0, Some(100.0));
        assert!(result.is_some(), "Valid preamble should be detected");
    }

    #[test]
    fn test_check_preamble_too_short() {
        let mag = vec![1000.0f32; WINDOW_SIZE - 1];
        assert!(check_preamble(&mag, 0, Some(100.0)).is_none());
    }

    #[test]
    fn test_noise_floor_tracker_initial() {
        let tracker = NoiseFloorTracker::new();
        assert_eq!(tracker.threshold(), MIN_SIGNAL_LEVEL * SNR_ADAPTIVE_FACTOR);
    }

    #[test]
    fn test_noise_floor_tracker_update() {
        let mut tracker = NoiseFloorTracker::new();
        // Feed low-noise buffer — should lower the threshold over time
        let mag = vec![10.0f32; 1000];
        for _ in 0..100 {
            tracker.update(&mag);
        }
        // Threshold should have converged toward 10 * SNR_ADAPTIVE_FACTOR = 30
        // But MIN_ADAPTIVE_LEVEL = 50 clamps it
        assert!(
            tracker.threshold() >= MIN_ADAPTIVE_LEVEL,
            "Threshold should not go below MIN_ADAPTIVE_LEVEL"
        );
    }

    #[test]
    fn test_noise_floor_tracker_reset() {
        let mut tracker = NoiseFloorTracker::new();
        let mag = vec![10.0f32; 1000];
        tracker.update(&mag);
        tracker.reset();
        assert_eq!(tracker.threshold(), MIN_SIGNAL_LEVEL * SNR_ADAPTIVE_FACTOR);
    }

    #[test]
    fn test_demodulate_buffer_empty() {
        let mag = vec![0.0f32; 1000];
        let mut tracker = NoiseFloorTracker::new();
        let frames = demodulate_buffer(&mag, 0.0, &mut tracker);
        assert!(frames.is_empty());
    }

    #[test]
    fn test_demodulate_buffer_noise() {
        // Random-ish noise should not produce frames
        let mag: Vec<f32> = (0..2000).map(|i| ((i * 37) % 100) as f32).collect();
        let mut tracker = NoiseFloorTracker::new();
        let frames = demodulate_buffer(&mag, 0.0, &mut tracker);
        assert!(frames.is_empty(), "Noise should not produce frames");
    }
}
