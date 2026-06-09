//! Raw append-only sink — the **pure writer** half of Phase-0 Area A2 (the real ADS-B
//! bleed-stop; see intel-hub `phase-0-execution-spec.md` correction #4: the off-box raw
//! sink is the durable moat, the DB stays the bounded hot tier).
//!
//! This module is intentionally PURE — no filesystem, no network, no time-of-day calls.
//! It produces (a) one self-describing, versioned **common-record** NDJSON line per
//! observation (the `data-pipeline-plan.md §2.4` envelope) and (b) the dated NAS path a
//! line belongs in. The thin `RawSink` adapter (buffered `OpenOptions::append` writer,
//! env-gated by `ADSB_RAW_SINK_DIR`) wires these into the live ingest path
//! (`web/ingest.rs` for ADS-B, `bin/ais-ingester.rs` for AIS) — that wire-in + deploy is
//! the Jason/pair step. Everything here is unit-tested headless with zero deps beyond
//! `serde_json` + std (the date math is no-chrono on purpose, since chrono is only a
//! `timescaledb`-feature dep).

use serde_json::{json, Value};
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
}
