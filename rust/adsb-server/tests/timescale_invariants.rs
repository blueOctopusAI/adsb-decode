//! Static invariants on the TimescaleDB schema + policy SQL.
//!
//! These tests parse `src/db_pg.rs` as text and assert the policy intervals
//! satisfy the rules learned from production incidents. They run unconditionally
//! (no `timescaledb` feature required), so a stray edit can't slip past CI by
//! disabling the feature flag.
//!
//! Why text-parsing instead of running real SQL: db_pg.rs is feature-gated
//! behind `timescaledb` and won't even compile without the dep tree. We want
//! these guardrails to fire in default `cargo test` runs.

use std::collections::HashMap;

const DB_PG_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/db_pg.rs");

#[derive(Debug, Clone, Copy)]
struct IntervalDays(u32);

impl IntervalDays {
    /// Parse a TimescaleDB INTERVAL literal like `INTERVAL '7 days'` or `INTERVAL '1 day'`
    /// into a day count. Returns None on shapes we don't expect — those should be
    /// added explicitly rather than silently passing.
    fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let n_str: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
        let n: u32 = n_str.parse().ok()?;
        let rest = s[n_str.len()..].trim().to_lowercase();
        match rest.as_str() {
            "day" | "days" => Some(IntervalDays(n)),
            "hour" | "hours" => Some(IntervalDays(n / 24)),
            other => panic!(
                "Unexpected INTERVAL unit {other:?}. If this is intentional, \
                 extend IntervalDays::parse to handle it."
            ),
        }
    }
}

/// Extract every `add_compression_policy('table', INTERVAL '...')` call into a map.
/// Same for `add_retention_policy`. Returns (compression, retention) per hypertable.
fn parse_policies(source: &str) -> (HashMap<String, IntervalDays>, HashMap<String, IntervalDays>) {
    let mut compression = HashMap::new();
    let mut retention = HashMap::new();

    for line in source.lines() {
        let trimmed = line.trim();
        for (kind, target) in [
            ("add_compression_policy", &mut compression),
            ("add_retention_policy", &mut retention),
        ] {
            if let Some(rest) = trimmed.strip_prefix(&format!("SELECT {kind}(")) {
                let table = rest.split('\'').nth(1).unwrap_or("").to_string();
                let interval_start = rest.find("INTERVAL '").map(|i| i + "INTERVAL '".len());
                let interval = interval_start
                    .and_then(|s| rest[s..].find('\'').map(|e| &rest[s..s + e]))
                    .unwrap_or_else(|| {
                        panic!("could not find INTERVAL in {kind} call: {trimmed:?}")
                    });
                let parsed = IntervalDays::parse(interval)
                    .unwrap_or_else(|| panic!("unparseable interval {interval:?} in {kind} call"));
                target.insert(table, parsed);
            }
        }
    }

    (compression, retention)
}

#[test]
fn compression_fires_strictly_before_retention_for_every_hypertable() {
    let source = std::fs::read_to_string(DB_PG_PATH)
        .unwrap_or_else(|e| panic!("could not read {DB_PG_PATH}: {e}"));
    let (compression, retention) = parse_policies(&source);

    assert!(
        !compression.is_empty(),
        "no compression policies parsed — did the SQL constants move?"
    );
    assert!(
        !retention.is_empty(),
        "no retention policies parsed — did the SQL constants move?"
    );

    // Every hypertable with a compression policy must have a retention policy
    // and the compression interval must be strictly less than retention.
    //
    // Why strict: at exactly equal intervals, a chunk hits both the compression
    // worker and the retention worker on the same scheduler tick. TimescaleDB
    // does not guarantee compression runs first, so chunks may drop uncompressed.
    // The 2026-04-14 events incident was 30-day-compress / 7-day-retain — way
    // worse than the boundary case, but the safe invariant is `compress < retain`.
    for (table, comp_days) in &compression {
        let ret_days = retention.get(table).unwrap_or_else(|| {
            panic!(
                "Hypertable {table:?} has a compression policy but no retention policy. \
                 Either add one, or remove the compression policy."
            )
        });
        assert!(
            comp_days.0 < ret_days.0,
            "Hypertable {table:?}: compression interval {} days >= retention interval {} days. \
             Compression must fire BEFORE retention drops the chunk, or you ship the \
             2026-04-14 events incident again (29 GB uncompressed hypertable). Bring \
             compression below retention.",
            comp_days.0,
            ret_days.0,
        );
    }
}

#[test]
fn every_hypertable_has_both_policies_or_explicit_exemption() {
    // Exempt hypertables that intentionally lack policies (e.g. small lookup tables
    // converted to hypertables). Today there are none. Add to this list with a
    // comment explaining why.
    const EXEMPT: &[&str] = &[];

    let source = std::fs::read_to_string(DB_PG_PATH).expect("could not read db_pg.rs");

    // Find every create_hypertable call.
    let hypertables: Vec<String> = source
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            l.strip_prefix("SELECT create_hypertable(")
                .and_then(|r| r.split('\'').nth(1))
                .map(|s| s.to_string())
        })
        .collect();

    assert!(
        !hypertables.is_empty(),
        "no create_hypertable calls parsed — did the SQL constants move?"
    );

    let (compression, retention) = parse_policies(&source);

    for table in &hypertables {
        if EXEMPT.contains(&table.as_str()) {
            continue;
        }
        assert!(
            compression.contains_key(table),
            "Hypertable {table:?} has no compression policy. Add one, or add to EXEMPT \
             with a comment explaining why."
        );
        assert!(
            retention.contains_key(table),
            "Hypertable {table:?} has no retention policy. Add one, or add to EXEMPT \
             with a comment explaining why."
        );
    }
}

#[test]
fn parse_policies_extracts_known_tables() {
    // Smoke test the parser itself against the current schema so a parser regression
    // doesn't silently let the invariant tests pass with empty maps.
    let source = std::fs::read_to_string(DB_PG_PATH).expect("could not read db_pg.rs");
    let (compression, retention) = parse_policies(&source);

    for expected in ["positions", "events", "vessel_positions"] {
        assert!(
            compression.contains_key(expected),
            "parser missed compression policy for {expected:?}"
        );
        assert!(
            retention.contains_key(expected),
            "parser missed retention policy for {expected:?}"
        );
    }
}
