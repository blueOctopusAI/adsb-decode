//! Statistical anomaly scoring via spatial position-density baseline.
//!
//! Complement to `adsb-core::anomaly`'s rules-based scorer. The rules catch
//! physically-impossible behavior; this scorer catches *statistically*
//! unusual behavior — a position over an area where almost nothing has flown
//! in the past week scores higher than a position over a major airway.
//!
//! Approach (V1):
//! - Bucket the world into 0.1° × 0.1° cells (~11 km × ~11 km at the equator).
//! - Periodically count positions per cell over the past N hours via SQL.
//! - Score = -log((count + 1) / (total + num_cells_seen))
//!   That is: the negative log-probability of a position landing in this cell
//!   under the empirical distribution. Laplace smoothing prevents -inf for
//!   cells we haven't seen yet.
//! - Clamp to [0, 5] so a single ridiculous outlier can't blow up downstream
//!   sums.
//!
//! V2 follow-ons (kept out of this commit deliberately): hour-of-week
//! dimension, per-altitude-band buckets, per-aircraft-class baselines, fast
//! refresh via TimescaleDB continuous aggregates. The persistence shape
//! (`anomaly_score` column) is what later ML work will write to; the math
//! here is replaceable.

use std::collections::HashMap;

/// Resolution of the spatial grid in degrees. 0.1° ≈ 11 km lat / 9 km lon at
/// 35° latitude. Coarse enough to give each cell meaningful counts, fine
/// enough to distinguish "over the ocean" from "over an airport."
pub const GRID_SIZE_DEG: f64 = 0.1;

/// Maximum statistical-score contribution per position. Caps how much a
/// single rare-cell observation can swing the combined anomaly score.
const MAX_SCORE: f64 = 5.0;

/// How many hours back to look when computing the baseline. 168 = 7 days.
pub const LOOKBACK_HOURS: f64 = 168.0;

/// Spatial density baseline cache. Built by `refresh()` from a SQL query;
/// queried by `score()` per position update.
#[derive(Debug, Default)]
pub struct BaselineCache {
    cells: HashMap<(i32, i32), u64>,
    total: u64,
    /// Observation-weighted median of `-ln(p)` across cells, computed at
    /// `replace()` time. Subtracted from each per-position raw score before
    /// clamping so a position in a *typical* cell scores 0 — only cells
    /// significantly rarer than typical traffic contribute positive score.
    ///
    /// Without this offset, `-ln(p)` of any non-trivial grid produces scores
    /// of 2–5 for almost every cell (because traffic spreads across many
    /// cells), which made every routine commercial flight read as
    /// "medium/high anomaly" on the dashboard. See v0.2.24 ROADMAP entry.
    offset: f64,
    /// Unix timestamp of the last successful refresh. 0 if never refreshed.
    pub last_refresh: f64,
}

impl BaselineCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the cell counts. Called from the background refresh task.
    pub fn replace(&mut self, cells: HashMap<(i32, i32), u64>, refreshed_at: f64) {
        let total: u64 = cells.values().sum();
        self.offset = observation_weighted_median_neg_log_prob(&cells, total);
        self.total = total;
        self.cells = cells;
        self.last_refresh = refreshed_at;
    }

    /// Score a position. Higher = more unusual under the spatial density
    /// baseline. Returns 0.0 when the cache is empty (no baseline yet) so a
    /// fresh process doesn't penalize every aircraft until the first refresh.
    ///
    /// Score is `max(0, -ln(p) - offset)` clamped to [0, MAX_SCORE], where
    /// `offset` is the observation-weighted median of `-ln(p)` across cells.
    /// A position in a typical-density cell scores 0; only cells rarer than
    /// the median observation contribute positive score.
    pub fn score(&self, lat: f64, lon: f64) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        let bucket = lat_lon_bucket(lat, lon);
        let count = self.cells.get(&bucket).copied().unwrap_or(0);
        // Laplace smoothing: pretend we've seen one extra observation in
        // every cell we have any data for. Prevents -inf for unseen cells
        // and shrinks the score for sparsely-seen ones.
        let smoothed_total = self.total + self.cells.len() as u64;
        let probability = (count + 1) as f64 / smoothed_total as f64;
        let raw_score = -probability.ln();
        (raw_score - self.offset).clamp(0.0, MAX_SCORE)
    }

    /// Number of distinct grid cells with at least one observation.
    /// Surfaced on /api/stats so operators can tell at a glance whether the
    /// baseline scorer is producing signal vs silently dormant.
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    /// Total observations across all cells — the denominator in the score
    /// calculation. Zero means the scorer is short-circuiting to 0.0 for
    /// every position.
    pub fn total(&self) -> u64 {
        self.total
    }
}

/// Convert (lat, lon) in degrees to grid bucket (lat × 10, lon × 10) as i32.
/// Public so the database backends can use the same bucketing in their
/// aggregation SQL.
pub fn lat_lon_bucket(lat: f64, lon: f64) -> (i32, i32) {
    let lat_b = (lat / GRID_SIZE_DEG).floor() as i32;
    let lon_b = (lon / GRID_SIZE_DEG).floor() as i32;
    (lat_b, lon_b)
}

/// Observation-weighted median of `-ln(p)` across cells.
///
/// "Observation-weighted" means: walk cells in ascending score order,
/// accumulating their counts, and return the score of the cell that crosses
/// `total / 2`. The median *observation* — not the median *cell* — sets the
/// baseline. A cell with 10,000 observations dominates 1,000 cells with one
/// observation each, which is the right weighting: "typical" means "where
/// typical aircraft fly," not "what most empty grid squares look like."
fn observation_weighted_median_neg_log_prob(cells: &HashMap<(i32, i32), u64>, total: u64) -> f64 {
    if total == 0 || cells.is_empty() {
        return 0.0;
    }
    let smoothed_total = total + cells.len() as u64;
    let mut scored: Vec<(f64, u64)> = cells
        .values()
        .map(|&c| {
            let p = (c + 1) as f64 / smoothed_total as f64;
            (-p.ln(), c)
        })
        .collect();
    // Sort by score ascending; ties broken arbitrarily — ties only happen
    // between equal-count cells which contribute the same score anyway.
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let half = total / 2;
    let mut acc: u64 = 0;
    for (s, c) in scored {
        acc = acc.saturating_add(c);
        if acc >= half {
            return s;
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_cache_scores_zero() {
        let cache = BaselineCache::new();
        assert_eq!(cache.score(35.0, -82.0), 0.0);
        assert_eq!(cache.cell_count(), 0);
    }

    #[test]
    fn high_density_cell_scores_lower_than_low_density_cell() {
        let mut cache = BaselineCache::new();
        let mut cells = HashMap::new();
        // Bucket (350, -820) covers lat [35.0, 35.1), lon [-82.0, -81.9).
        // Insert a busy airway cell here.
        cells.insert((350, -820), 10_000u64);
        // Bucket (360, -830) covers lat [36.0, 36.1), lon [-83.0, -82.9).
        // Insert a sparse cell here.
        cells.insert((360, -830), 1u64);
        cache.replace(cells, 1.0);

        // Score points that fall inside each bucket. floor(-81.95 / 0.1) =
        // floor(-819.5) = -820; floor(-82.95 / 0.1) = floor(-829.5) = -830.
        let busy_score = cache.score(35.05, -81.95);
        let sparse_score = cache.score(36.05, -82.95);

        assert!(
            busy_score < sparse_score,
            "busy={busy_score}, sparse={sparse_score}"
        );
    }

    #[test]
    fn unseen_cell_scores_within_bounds() {
        let mut cache = BaselineCache::new();
        let mut cells = HashMap::new();
        cells.insert((350, -820), 1000u64);
        cache.replace(cells, 1.0);

        // A cell we've never seen should score in [0, MAX_SCORE], not -inf.
        let score = cache.score(45.0, -120.0);
        assert!((0.0..=MAX_SCORE).contains(&score), "got {score}");
    }

    /// Recentering pin: a position in the cell where the *median observation*
    /// lives scores 0, not 2–3. Before v0.2.24 every routine flight through
    /// the WNC airspace scored 2+ on the dashboard because the raw `-ln(p)` of
    /// any non-trivial grid is naturally non-zero. The fix subtracts the
    /// observation-weighted median, so the "typical" cell shows as normal.
    #[test]
    fn typical_traffic_cell_scores_zero_after_recentering() {
        let mut cache = BaselineCache::new();
        let mut cells = HashMap::new();
        // Three centerline cells with most of the traffic, plus a handful of
        // off-centerline cells with moderate traffic. The median observation
        // sits in a centerline cell — its score should land at 0.
        cells.insert((350, -820), 20_000u64);
        cells.insert((350, -821), 15_000u64);
        cells.insert((351, -820), 10_000u64);
        cells.insert((352, -825), 500u64); // off-centerline
        cells.insert((355, -828), 100u64); // sparse
        cache.replace(cells, 1.0);

        let centerline = cache.score(35.05, -81.95); // (350, -820)
        assert_eq!(
            centerline, 0.0,
            "centerline (densest) cell must score 0 after recentering, got {centerline}"
        );
    }

    /// Off-typical-but-still-trafficked cells produce positive but bounded
    /// scores. This is the "now you can tell signal from noise" case: a
    /// flight in a less-trafficked corner of the airspace scores above 0
    /// but well below the genuinely-rare bucket.
    #[test]
    fn off_typical_cell_scores_above_zero_below_max() {
        let mut cache = BaselineCache::new();
        let mut cells = HashMap::new();
        cells.insert((350, -820), 20_000u64);
        cells.insert((350, -821), 15_000u64);
        cells.insert((351, -820), 10_000u64);
        cells.insert((352, -825), 500u64);
        cells.insert((355, -828), 100u64);
        cache.replace(cells, 1.0);

        let off_typical = cache.score(35.25, -82.45); // (352, -825)
        assert!(
            off_typical > 0.0,
            "off-typical cell must score above 0, got {off_typical}"
        );
        assert!(
            off_typical < MAX_SCORE,
            "off-typical (still has 500 obs) should not max out, got {off_typical}"
        );
    }

    /// Unseen / never-recorded cells score at MAX_SCORE after recentering —
    /// the offset doesn't suppress the genuinely-rare signal we want to keep.
    #[test]
    fn unseen_cell_scores_at_max_after_recentering() {
        let mut cache = BaselineCache::new();
        let mut cells = HashMap::new();
        cells.insert((350, -820), 20_000u64);
        cells.insert((350, -821), 15_000u64);
        cells.insert((351, -820), 10_000u64);
        cells.insert((352, -825), 500u64);
        cache.replace(cells, 1.0);

        let unseen = cache.score(45.0, -120.0);
        assert!(
            unseen >= MAX_SCORE - 0.01,
            "unseen cell should still flag at ~MAX_SCORE, got {unseen}"
        );
    }

    #[test]
    fn score_is_clamped_to_max() {
        let mut cache = BaselineCache::new();
        let mut cells = HashMap::new();
        // Massive cell + tiny unseen cell → log probability is very negative,
        // raw score very large, but we clamp.
        for i in 0..1_000_000 {
            cells.insert((350, -820 + (i % 5) as i32), 1u64);
        }
        cache.replace(cells, 1.0);
        let score = cache.score(45.0, -120.0);
        assert!(score <= MAX_SCORE);
    }

    #[test]
    fn bucket_floor_handles_negative_lon() {
        // Western longitudes (negative) need floor, not truncation, so
        // -82.05 → -821 (bucket -821 covers -82.1 to -82.0), not -820.
        let (_, lon_b) = lat_lon_bucket(35.0, -82.05);
        assert_eq!(lon_b, -821);
    }

    #[test]
    fn bucket_floor_handles_positive_lon() {
        let (_, lon_b) = lat_lon_bucket(35.0, 82.05);
        assert_eq!(lon_b, 820);
    }
}
