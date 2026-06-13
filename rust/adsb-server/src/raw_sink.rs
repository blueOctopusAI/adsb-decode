//! Raw append-only sink — Phase-0 Area A2 (the real ADS-B bleed-stop; see intel-hub
//! `phase-0-execution-spec.md` correction #4: the off-box raw sink is the durable moat,
//! the DB stays the bounded hot tier).
//!
//! Two halves:
//!
//! **Pure functions** (no I/O, fully unit-tested): `ndjson_line` builds one versioned
//! NDJSON envelope; `dated_path` returns the NAS path it belongs in.
//!
//! **`RawSink` adapter** (thin I/O wrapper): buffered `OpenOptions::append` writer,
//! env-gated by `ADSB_RAW_SINK_DIR`. Call `RawSink::from_env()` once at startup —
//! returns `None` when the env var is absent (no-op, live service is unaffected). Call
//! `sink.write_adsb(...)` on the hot path inside `web/ingest.rs`; it picks the right
//! dated file, creates parent dirs on first write of a new day, and never panics —
//! errors are logged to stderr and swallowed so a NAS hiccup cannot take down the
//! receiver loop.
//!
//! Everything here depends only on `serde_json` + std (date math is no-chrono on
//! purpose, since chrono is only a `timescaledb`-feature dep).

use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Build one common-record NDJSON line (the versioned envelope; `payload` is typed-per-layer).
/// Pure: same inputs → identical string. A `None` coordinate serializes as JSON `null`
/// (versioned-envelope discipline — a missing optional field is explicit, never dropped).
pub fn ndjson_line(
    layer: &str,
    observed_at: f64,
    lat: Option<f64>,
    lon: Option<f64>,
    elev_m: Option<f64>,
    payload: &Value,
    source: &str,
) -> String {
    let envelope = json!({
        "layer": layer,
        "schema_version": "1.0",
        "observed_at": observed_at,
        "geom": { "lat": lat, "lon": lon, "elev_m": elev_m },
        "payload": payload,
        "provenance": { "source": source, "as_of": observed_at },
    });
    envelope.to_string()
}

/// `root/<layer>/YYYY-MM-DD/<layer>-YYYY-MM-DD.ndjson`, the date being the UTC day of
/// `observed_at`. Pure; the UTC Y/M/D is computed without chrono.
pub fn dated_path(root: &Path, layer: &str, observed_at: f64) -> PathBuf {
    let (y, m, d) = ymd_utc(observed_at);
    let date = format!("{:04}-{:02}-{:02}", y, m, d);
    root.join(layer)
        .join(&date)
        .join(format!("{}-{}.ndjson", layer, date))
}

// ---------------------------------------------------------------------------
// RawSink — thin I/O adapter (env-gated; no-op when ADSB_RAW_SINK_DIR absent)
// ---------------------------------------------------------------------------

/// Append-only NDJSON sink for the raw ADS-B + AIS retention stream.
///
/// Construct with `RawSink::from_env()`. Returns `None` if `ADSB_RAW_SINK_DIR`
/// is not set — the caller can then skip all sink calls with zero overhead.
///
/// Each call to `write_adsb` / `write_ais` appends one NDJSON line to the
/// correct dated file under `<root>/<layer>/YYYY-MM-DD/<layer>-YYYY-MM-DD.ndjson`.
/// Parent directories are created on demand. Errors are written to stderr and
/// swallowed — a NAS hiccup must not take down the ingest loop.
pub struct RawSink {
    root: PathBuf,
}

impl RawSink {
    /// Create a `RawSink` from the `ADSB_RAW_SINK_DIR` env var.
    /// Returns `None` when the variable is absent (opt-out is the default so
    /// existing deployments that don't set the var are unaffected).
    pub fn from_env() -> Option<Self> {
        std::env::var("ADSB_RAW_SINK_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| RawSink { root: PathBuf::from(s) })
    }

    /// Append one ADS-B observation. `receiver` is used as the provenance source tag.
    /// `lat`/`lon` are the aircraft position (may be `None` if not yet decoded).
    /// `altitude_ft` is the pressure altitude reported by the transponder.
    pub fn write_adsb(
        &self,
        icao: &str,
        observed_at: f64,
        lat: Option<f64>,
        lon: Option<f64>,
        altitude_ft: Option<i32>,
        receiver: &str,
    ) {
        let payload = json!({
            "icao": icao,
            "altitude_ft": altitude_ft,
        });
        let source = format!("receiver:{}", receiver);
        let line = ndjson_line("adsb", observed_at, lat, lon, None, &payload, &source);
        self.append("adsb", observed_at, &line);
    }

    /// Append one AIS observation. Wired into `bin/ais-ingester.rs` when that
    /// ingest path is plumbed (the ADS-B tee is already live; AIS follows).
    #[allow(dead_code)]
    pub fn write_ais(
        &self,
        mmsi: &str,
        observed_at: f64,
        lat: Option<f64>,
        lon: Option<f64>,
        source: &str,
    ) {
        let payload = json!({ "mmsi": mmsi });
        let line = ndjson_line("ais", observed_at, lat, lon, None, &payload, source);
        self.append("ais", observed_at, &line);
    }

    /// Core append: pick the dated path, create dirs, open in append mode, write line + newline.
    /// All errors are logged to stderr and swallowed.
    fn append(&self, layer: &str, observed_at: f64, line: &str) {
        let path = dated_path(&self.root, layer, observed_at);
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!("[raw_sink] create_dir_all {:?}: {}", parent, e);
                return;
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(&path);
        match file {
            Ok(f) => {
                let mut w = BufWriter::new(f);
                if let Err(e) = writeln!(w, "{}", line) {
                    eprintln!("[raw_sink] write {:?}: {}", path, e);
                }
            }
            Err(e) => {
                eprintln!("[raw_sink] open {:?}: {}", path, e);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Private date math
// ---------------------------------------------------------------------------

/// UTC (year, month, day) for a unix timestamp. Days-since-epoch → civil date via Howard
/// Hinnant's `civil_from_days` (pure integer math, valid across the proleptic Gregorian
/// calendar). `div_euclid` handles pre-epoch timestamps correctly.
fn ymd_utc(observed_at: f64) -> (i64, u32, u32) {
    let days = (observed_at as i64).div_euclid(86_400);
    civil_from_days(days)
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m as u32, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelope_shape_is_versioned_and_complete() {
        let line = ndjson_line(
            "adsb",
            1_733_673_600.0,
            Some(35.59),
            Some(-82.55),
            Some(650.0),
            &json!({"icao": "a1b2c3", "alt": 32000}),
            "receiver:pi-01",
        );
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["layer"], "adsb");
        assert_eq!(v["schema_version"], "1.0");
        assert_eq!(v["observed_at"], 1_733_673_600.0);
        assert_eq!(v["geom"]["lat"], 35.59);
        assert_eq!(v["geom"]["elev_m"], 650.0);
        assert_eq!(v["payload"]["icao"], "a1b2c3");
        assert_eq!(v["provenance"]["source"], "receiver:pi-01");
        assert_eq!(v["provenance"]["as_of"], 1_733_673_600.0);
        // single line, no embedded newline (NDJSON invariant)
        assert!(!line.contains('\n'));
    }

    #[test]
    fn missing_coords_serialize_as_null_not_dropped() {
        let line = ndjson_line("ais", 1.0, None, None, None, &json!({"mmsi": 1}), "ais:stream");
        let v: Value = serde_json::from_str(&line).unwrap();
        assert!(v["geom"]["lat"].is_null());
        assert!(v["geom"]["lon"].is_null());
        assert!(v["geom"]["elev_m"].is_null());
    }

    #[test]
    fn deterministic() {
        let a = ndjson_line("adsb", 5.0, Some(1.0), Some(2.0), None, &json!({"x": 1}), "s");
        let b = ndjson_line("adsb", 5.0, Some(1.0), Some(2.0), None, &json!({"x": 1}), "s");
        assert_eq!(a, b);
    }

    #[test]
    fn dated_path_buckets_by_utc_day() {
        let root = Path::new("/mnt/nas/raw");
        // 2024-12-08 16:00:00 UTC
        let p = dated_path(root, "adsb", 1_733_673_600.0);
        assert_eq!(p, Path::new("/mnt/nas/raw/adsb/2024-12-08/adsb-2024-12-08.ndjson"));
    }

    #[test]
    fn utc_day_boundaries_and_epoch() {
        assert_eq!(ymd_utc(0.0), (1970, 1, 1));
        assert_eq!(ymd_utc(86_399.0), (1970, 1, 1)); // 23:59:59 — same day
        assert_eq!(ymd_utc(86_400.0), (1970, 1, 2)); // next midnight rolls over
        assert_eq!(ymd_utc(1_704_067_200.0), (2024, 1, 1)); // 2024-01-01T00:00:00Z
        assert_eq!(ymd_utc(951_782_400.0), (2000, 2, 29)); // leap day
    }

    // -----------------------------------------------------------------------
    // RawSink integration tests (real filesystem via tempdir)
    // -----------------------------------------------------------------------

    #[test]
    fn raw_sink_from_env_absent_returns_none() {
        // Ensure the var is not set for this test.
        std::env::remove_var("ADSB_RAW_SINK_DIR");
        assert!(RawSink::from_env().is_none());
    }

    #[test]
    fn raw_sink_from_env_present_returns_some() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ADSB_RAW_SINK_DIR", dir.path().to_str().unwrap());
        let sink = RawSink::from_env();
        std::env::remove_var("ADSB_RAW_SINK_DIR");
        assert!(sink.is_some());
    }

    #[test]
    fn raw_sink_write_adsb_creates_dated_ndjson() {
        let dir = tempfile::tempdir().unwrap();
        let sink = RawSink { root: dir.path().to_path_buf() };

        // 2024-12-08 16:00:00 UTC
        let ts = 1_733_673_600.0_f64;
        sink.write_adsb("a1b2c3", ts, Some(35.59), Some(-82.55), Some(32000), "pi-01");

        let expected = dir.path()
            .join("adsb")
            .join("2024-12-08")
            .join("adsb-2024-12-08.ndjson");
        assert!(expected.exists(), "dated NDJSON file was not created");

        let contents = std::fs::read_to_string(&expected).unwrap();
        let v: Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(v["layer"], "adsb");
        assert_eq!(v["schema_version"], "1.0");
        assert_eq!(v["observed_at"], ts);
        assert_eq!(v["geom"]["lat"], 35.59);
        assert_eq!(v["geom"]["lon"], -82.55);
        assert_eq!(v["payload"]["icao"], "a1b2c3");
        assert_eq!(v["payload"]["altitude_ft"], 32000);
        assert_eq!(v["provenance"]["source"], "receiver:pi-01");
    }

    #[test]
    fn raw_sink_appends_multiple_lines() {
        let dir = tempfile::tempdir().unwrap();
        let sink = RawSink { root: dir.path().to_path_buf() };
        let ts = 1_733_673_600.0_f64;

        sink.write_adsb("aaa111", ts, Some(35.0), Some(-82.0), Some(10000), "rx1");
        sink.write_adsb("bbb222", ts + 1.0, Some(36.0), Some(-83.0), Some(20000), "rx1");

        let expected = dir.path()
            .join("adsb")
            .join("2024-12-08")
            .join("adsb-2024-12-08.ndjson");
        let contents = std::fs::read_to_string(&expected).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "expected 2 NDJSON lines, got {}", lines.len());

        let v0: Value = serde_json::from_str(lines[0]).unwrap();
        let v1: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(v0["payload"]["icao"], "aaa111");
        assert_eq!(v1["payload"]["icao"], "bbb222");
    }

    #[test]
    fn raw_sink_day_rollover_creates_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let sink = RawSink { root: dir.path().to_path_buf() };

        // Two different UTC days
        let ts_day1 = 1_733_673_600.0_f64; // 2024-12-08
        let ts_day2 = ts_day1 + 86_400.0;  // 2024-12-09

        sink.write_adsb("day1", ts_day1, None, None, None, "rx");
        sink.write_adsb("day2", ts_day2, None, None, None, "rx");

        let file_day1 = dir.path().join("adsb").join("2024-12-08").join("adsb-2024-12-08.ndjson");
        let file_day2 = dir.path().join("adsb").join("2024-12-09").join("adsb-2024-12-09.ndjson");
        assert!(file_day1.exists(), "day-1 file missing");
        assert!(file_day2.exists(), "day-2 file missing: day rollover did not create new file");
    }

    #[test]
    fn raw_sink_write_ais_layer_is_ais() {
        let dir = tempfile::tempdir().unwrap();
        let sink = RawSink { root: dir.path().to_path_buf() };
        let ts = 1_733_673_600.0_f64;

        sink.write_ais("123456789", ts, Some(35.0), Some(-82.0), "ais:stream");

        let expected = dir.path()
            .join("ais")
            .join("2024-12-08")
            .join("ais-2024-12-08.ndjson");
        assert!(expected.exists());
        let contents = std::fs::read_to_string(&expected).unwrap();
        let v: Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(v["layer"], "ais");
        assert_eq!(v["payload"]["mmsi"], "123456789");
    }
}
