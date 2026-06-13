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
            .map(|s| RawSink {
                root: PathBuf::from(s),
            })
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
// Verifier — reads the archive back and proves integrity (Phase-0 A2 closes its loop)
// ---------------------------------------------------------------------------
//
// The sink swallows write errors so a NAS hiccup can't kill ingest. The cost of
// that resilience is that a partial write, a truncated file, or a misfiled record
// is silent. The verifier is the missing feedback loop: it walks the archive,
// re-parses every NDJSON line against the envelope contract, and reports any line
// that doesn't round-trip. Pure (`verify_line`, `check_envelope`) + a thin walker
// (`RawSink::verify`) so the parse contract is unit-tested without touching disk.

/// Why a single NDJSON line failed verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineFault {
    /// Not valid JSON at all (truncated / partial append / corruption).
    NotJson,
    /// Parsed as JSON but missing a required envelope field.
    MissingField(&'static str),
    /// `layer` field doesn't match the layer the file is filed under.
    LayerMismatch { expected: String, found: String },
    /// `schema_version` is not a recognized version.
    UnknownSchemaVersion(String),
}

impl std::fmt::Display for LineFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LineFault::NotJson => write!(f, "not valid JSON"),
            LineFault::MissingField(field) => write!(f, "missing required field `{}`", field),
            LineFault::LayerMismatch { expected, found } => {
                write!(
                    f,
                    "layer mismatch: file is `{}` but record says `{}`",
                    expected, found
                )
            }
            LineFault::UnknownSchemaVersion(v) => write!(f, "unknown schema_version `{}`", v),
        }
    }
}

/// Schema versions this build knows how to read. Append new versions here as the
/// envelope evolves; an archived line stamped with a version not in this set is a
/// fault, which forces a conscious decision rather than a silent skip.
const KNOWN_SCHEMA_VERSIONS: &[&str] = &["1.0"];

/// Required top-level envelope fields every archived record must carry.
const REQUIRED_FIELDS: &[&str] = &[
    "layer",
    "schema_version",
    "observed_at",
    "geom",
    "payload",
    "provenance",
];

/// Validate one already-parsed JSON envelope against the contract, given the
/// `layer` the containing file is filed under. Pure.
pub fn check_envelope(v: &Value, expected_layer: &str) -> Result<(), LineFault> {
    for field in REQUIRED_FIELDS {
        if v.get(field).is_none() {
            return Err(LineFault::MissingField(field));
        }
    }
    let schema = v["schema_version"].as_str().unwrap_or("");
    if !KNOWN_SCHEMA_VERSIONS.contains(&schema) {
        return Err(LineFault::UnknownSchemaVersion(schema.to_string()));
    }
    let layer = v["layer"].as_str().unwrap_or("");
    if layer != expected_layer {
        return Err(LineFault::LayerMismatch {
            expected: expected_layer.to_string(),
            found: layer.to_string(),
        });
    }
    Ok(())
}

/// Parse + validate one raw NDJSON line for a file filed under `expected_layer`. Pure.
pub fn verify_line(line: &str, expected_layer: &str) -> Result<(), LineFault> {
    let v: Value = serde_json::from_str(line).map_err(|_| LineFault::NotJson)?;
    check_envelope(&v, expected_layer)
}

/// Aggregate result of verifying a span of the archive.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// Number of NDJSON files inspected.
    pub files: usize,
    /// Total non-blank lines seen across all files.
    pub lines: usize,
    /// Lines that passed the envelope contract.
    pub ok: usize,
    /// `(path, line_number_1_based, fault)` for every line that failed.
    pub faults: Vec<(PathBuf, usize, LineFault)>,
}

impl VerifyReport {
    /// True when every line round-tripped (no faults).
    pub fn is_clean(&self) -> bool {
        self.faults.is_empty()
    }
}

impl RawSink {
    /// Walk the whole archive under `root` and verify every NDJSON line.
    ///
    /// The layer a file is filed under is taken from the top-level directory name
    /// (`<root>/<layer>/YYYY-MM-DD/...`), so a record misfiled into the wrong layer
    /// is caught as a `LayerMismatch`. Blank lines are skipped (a trailing newline is
    /// normal). Unreadable files are surfaced as a single `NotJson` fault at line 0
    /// rather than aborting the walk, so one bad file doesn't hide the rest.
    ///
    /// The CLI verifies an arbitrary path via [`verify_archive`]; this method is the
    /// ergonomic wrapper for an already-constructed sink (used by callers + tests).
    #[allow(dead_code)]
    pub fn verify(&self) -> VerifyReport {
        verify_archive(&self.root)
    }
}

/// Verify every `*.ndjson` file under `root`. Standalone (not a method) so the CLI
/// can verify an arbitrary directory without constructing a `RawSink`.
pub fn verify_archive(root: &Path) -> VerifyReport {
    let mut report = VerifyReport::default();
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_ndjson(root, root, &mut files);
    files.sort();
    for (layer, path) in files {
        report.files += 1;
        let contents = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => {
                report.faults.push((path.clone(), 0, LineFault::NotJson));
                continue;
            }
        };
        for (i, line) in contents.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            report.lines += 1;
            match verify_line(line, &layer) {
                Ok(()) => report.ok += 1,
                Err(fault) => report.faults.push((path.clone(), i + 1, fault)),
            }
        }
    }
    report
}

/// Recursively gather `*.ndjson` files, tagging each with the layer it's filed under
/// (the first path component below `root`). Directories that aren't a layer/date tree
/// are still walked, so a flat directory of files verifies against its own dir name.
fn collect_ndjson(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_ndjson(root, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("ndjson") {
            let layer = layer_of(root, &path);
            out.push((layer, path));
        }
    }
}

/// The layer a file belongs to = the first path component of `path` relative to `root`.
/// Falls back to the file's parent dir name when `path` isn't under `root`.
fn layer_of(root: &Path, path: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(root) {
        if let Some(first) = rel.components().next() {
            return first.as_os_str().to_string_lossy().into_owned();
        }
    }
    path.parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
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
        let line = ndjson_line(
            "ais",
            1.0,
            None,
            None,
            None,
            &json!({"mmsi": 1}),
            "ais:stream",
        );
        let v: Value = serde_json::from_str(&line).unwrap();
        assert!(v["geom"]["lat"].is_null());
        assert!(v["geom"]["lon"].is_null());
        assert!(v["geom"]["elev_m"].is_null());
    }

    #[test]
    fn deterministic() {
        let a = ndjson_line(
            "adsb",
            5.0,
            Some(1.0),
            Some(2.0),
            None,
            &json!({"x": 1}),
            "s",
        );
        let b = ndjson_line(
            "adsb",
            5.0,
            Some(1.0),
            Some(2.0),
            None,
            &json!({"x": 1}),
            "s",
        );
        assert_eq!(a, b);
    }

    #[test]
    fn dated_path_buckets_by_utc_day() {
        let root = Path::new("/mnt/nas/raw");
        // 2024-12-08 16:00:00 UTC
        let p = dated_path(root, "adsb", 1_733_673_600.0);
        assert_eq!(
            p,
            Path::new("/mnt/nas/raw/adsb/2024-12-08/adsb-2024-12-08.ndjson")
        );
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
        let sink = RawSink {
            root: dir.path().to_path_buf(),
        };

        // 2024-12-08 16:00:00 UTC
        let ts = 1_733_673_600.0_f64;
        sink.write_adsb(
            "a1b2c3",
            ts,
            Some(35.59),
            Some(-82.55),
            Some(32000),
            "pi-01",
        );

        let expected = dir
            .path()
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
        let sink = RawSink {
            root: dir.path().to_path_buf(),
        };
        let ts = 1_733_673_600.0_f64;

        sink.write_adsb("aaa111", ts, Some(35.0), Some(-82.0), Some(10000), "rx1");
        sink.write_adsb(
            "bbb222",
            ts + 1.0,
            Some(36.0),
            Some(-83.0),
            Some(20000),
            "rx1",
        );

        let expected = dir
            .path()
            .join("adsb")
            .join("2024-12-08")
            .join("adsb-2024-12-08.ndjson");
        let contents = std::fs::read_to_string(&expected).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "expected 2 NDJSON lines, got {}",
            lines.len()
        );

        let v0: Value = serde_json::from_str(lines[0]).unwrap();
        let v1: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(v0["payload"]["icao"], "aaa111");
        assert_eq!(v1["payload"]["icao"], "bbb222");
    }

    #[test]
    fn raw_sink_day_rollover_creates_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let sink = RawSink {
            root: dir.path().to_path_buf(),
        };

        // Two different UTC days
        let ts_day1 = 1_733_673_600.0_f64; // 2024-12-08
        let ts_day2 = ts_day1 + 86_400.0; // 2024-12-09

        sink.write_adsb("day1", ts_day1, None, None, None, "rx");
        sink.write_adsb("day2", ts_day2, None, None, None, "rx");

        let file_day1 = dir
            .path()
            .join("adsb")
            .join("2024-12-08")
            .join("adsb-2024-12-08.ndjson");
        let file_day2 = dir
            .path()
            .join("adsb")
            .join("2024-12-09")
            .join("adsb-2024-12-09.ndjson");
        assert!(file_day1.exists(), "day-1 file missing");
        assert!(
            file_day2.exists(),
            "day-2 file missing: day rollover did not create new file"
        );
    }

    #[test]
    fn raw_sink_write_ais_layer_is_ais() {
        let dir = tempfile::tempdir().unwrap();
        let sink = RawSink {
            root: dir.path().to_path_buf(),
        };
        let ts = 1_733_673_600.0_f64;

        sink.write_ais("123456789", ts, Some(35.0), Some(-82.0), "ais:stream");

        let expected = dir
            .path()
            .join("ais")
            .join("2024-12-08")
            .join("ais-2024-12-08.ndjson");
        assert!(expected.exists());
        let contents = std::fs::read_to_string(&expected).unwrap();
        let v: Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(v["layer"], "ais");
        assert_eq!(v["payload"]["mmsi"], "123456789");
    }

    // -----------------------------------------------------------------------
    // Verifier — pure contract checks
    // -----------------------------------------------------------------------

    #[test]
    fn verify_line_accepts_a_well_formed_record() {
        let line = ndjson_line(
            "adsb",
            1_733_673_600.0,
            Some(35.59),
            Some(-82.55),
            None,
            &json!({"icao": "a1b2c3", "altitude_ft": 32000}),
            "receiver:pi-01",
        );
        assert_eq!(verify_line(&line, "adsb"), Ok(()));
    }

    #[test]
    fn verify_line_rejects_non_json() {
        assert_eq!(verify_line("{not json", "adsb"), Err(LineFault::NotJson));
        // A partial append (file truncated mid-line) — classic NAS-hiccup corruption.
        assert_eq!(
            verify_line(r#"{"layer":"adsb","schema_ver"#, "adsb"),
            Err(LineFault::NotJson)
        );
    }

    #[test]
    fn verify_line_rejects_missing_required_field() {
        // Valid JSON, but no `provenance`.
        let v = json!({
            "layer": "adsb",
            "schema_version": "1.0",
            "observed_at": 1.0,
            "geom": { "lat": null, "lon": null, "elev_m": null },
            "payload": { "icao": "x" },
        });
        assert_eq!(
            verify_line(&v.to_string(), "adsb"),
            Err(LineFault::MissingField("provenance"))
        );
    }

    #[test]
    fn verify_line_rejects_unknown_schema_version() {
        let v = json!({
            "layer": "adsb",
            "schema_version": "9.9",
            "observed_at": 1.0,
            "geom": {},
            "payload": {},
            "provenance": {},
        });
        match verify_line(&v.to_string(), "adsb") {
            Err(LineFault::UnknownSchemaVersion(got)) => assert_eq!(got, "9.9"),
            other => panic!("expected UnknownSchemaVersion, got {:?}", other),
        }
    }

    #[test]
    fn verify_line_rejects_misfiled_layer() {
        // An `ais` record sitting in an `adsb` file.
        let line = ndjson_line(
            "ais",
            1.0,
            None,
            None,
            None,
            &json!({"mmsi": "1"}),
            "ais:stream",
        );
        match verify_line(&line, "adsb") {
            Err(LineFault::LayerMismatch { expected, found }) => {
                assert_eq!(expected, "adsb");
                assert_eq!(found, "ais");
            }
            other => panic!("expected LayerMismatch, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Verifier — full archive walk over a real temp tree
    // -----------------------------------------------------------------------

    #[test]
    fn verify_archive_clean_tree_reports_no_faults() {
        let dir = tempfile::tempdir().unwrap();
        let sink = RawSink {
            root: dir.path().to_path_buf(),
        };
        let ts = 1_733_673_600.0_f64;

        sink.write_adsb("aaa111", ts, Some(35.0), Some(-82.0), Some(10000), "rx1");
        sink.write_adsb(
            "bbb222",
            ts + 1.0,
            Some(36.0),
            Some(-83.0),
            Some(20000),
            "rx1",
        );
        sink.write_adsb("ccc333", ts + 86_400.0, None, None, None, "rx1"); // next day → 2nd file
        sink.write_ais("123456789", ts, Some(35.0), Some(-82.0), "ais:stream");

        let report = sink.verify();
        assert!(report.is_clean(), "faults: {:?}", report.faults);
        assert_eq!(report.files, 3, "two adsb day files + one ais day file");
        assert_eq!(report.lines, 4);
        assert_eq!(report.ok, 4);
    }

    #[test]
    fn verify_archive_catches_a_corrupt_line() {
        let dir = tempfile::tempdir().unwrap();
        let sink = RawSink {
            root: dir.path().to_path_buf(),
        };
        let ts = 1_733_673_600.0_f64;
        sink.write_adsb("good01", ts, Some(35.0), Some(-82.0), Some(10000), "rx1");

        // Simulate a torn append: a half-written line at the end of the day file.
        let path = dir
            .path()
            .join("adsb")
            .join("2024-12-08")
            .join("adsb-2024-12-08.ndjson");
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, r#"{{"layer":"adsb","schema_ver"#).unwrap();
        drop(f);

        let report = sink.verify();
        assert!(!report.is_clean());
        assert_eq!(report.ok, 1, "the one good line still verifies");
        assert_eq!(report.faults.len(), 1);
        let (fault_path, lineno, fault) = &report.faults[0];
        assert_eq!(fault_path, &path);
        assert_eq!(*lineno, 2, "fault is on the second (torn) line, 1-based");
        assert_eq!(fault, &LineFault::NotJson);
    }

    #[test]
    fn verify_archive_blank_lines_are_not_counted() {
        let dir = tempfile::tempdir().unwrap();
        let sink = RawSink {
            root: dir.path().to_path_buf(),
        };
        let ts = 1_733_673_600.0_f64;
        sink.write_adsb("good01", ts, None, None, None, "rx1");

        let path = dir
            .path()
            .join("adsb")
            .join("2024-12-08")
            .join("adsb-2024-12-08.ndjson");
        // Trailing blank lines (a normal artifact of newline handling) must not fault.
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f).unwrap();
        writeln!(f, "   ").unwrap();
        drop(f);

        let report = sink.verify();
        assert!(
            report.is_clean(),
            "blank lines should not be faults: {:?}",
            report.faults
        );
        assert_eq!(report.lines, 1);
    }

    #[test]
    fn verify_archive_empty_dir_is_clean_zero() {
        let dir = tempfile::tempdir().unwrap();
        let report = verify_archive(dir.path());
        assert!(report.is_clean());
        assert_eq!(report.files, 0);
        assert_eq!(report.lines, 0);
    }
}
