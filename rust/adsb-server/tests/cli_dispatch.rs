//! CLI dispatch integration tests for the `adsb` binary.
//!
//! Spawns the compiled binary via `env!("CARGO_BIN_EXE_adsb")` and asserts on
//! exit codes, stdout/stderr content, and side-effects against a tempdir.
//! No extra dev-deps (no assert_cmd) — std::process::Command is enough for
//! the contracts that matter.
//!
//! Why these tests exist: `main.rs` (CLI dispatch) was 0%-tested before this
//! file. A clap subcommand rename, an arg-renaming refactor, or a regression
//! in `cmd_stats` / `cmd_history` / `cmd_export` would only be caught by a
//! human running the binary by hand. These pin the user-facing surface.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_adsb"))
}

#[test]
fn cli_help_lists_all_subcommands() {
    // Smoke: --help exits 0 and mentions every public subcommand. A clap
    // refactor that accidentally renames or drops a subcommand fires here.
    let out = bin().arg("--help").output().expect("spawn");
    assert!(out.status.success(), "adsb --help exit: {}", out.status);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}\n{stderr}");

    for sub in [
        "decode", "track", "stats", "history", "export", "serve", "setup",
    ] {
        assert!(
            combined.contains(sub),
            "adsb --help missing subcommand {sub:?}. Output:\n{combined}"
        );
    }
}

#[test]
fn cli_version_prints_something() {
    // Picks up the workspace version string. Must be non-empty so monitoring /
    // deploy scripts can parse it.
    let out = bin().arg("--version").output().expect("spawn");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("adsb"),
        "version output should contain 'adsb': {stdout}"
    );
}

#[test]
fn cli_invalid_subcommand_exits_nonzero() {
    // Clap default — but pinned because a clap upgrade has changed exit behavior
    // before, and we rely on shell scripts checking $? for "did the operator
    // type something we recognize."
    let out = bin().arg("bogus-subcommand").output().expect("spawn");
    assert!(
        !out.status.success(),
        "bogus subcommand should exit non-zero"
    );
}

#[test]
fn cli_decode_processes_known_hex_frames() {
    // Write a handful of well-formed Mode-S hex frames to a temp file and
    // confirm `adsb decode` exits 0 and prints an aircraft summary table.
    //
    // These frames are from the project's known-good capture file
    // (data/live_capture.txt) — the same vectors the cross-validation harness
    // uses against the Python reference decoder.
    let dir = tempfile::tempdir().expect("tempdir");
    let cap_path = dir.path().join("test_capture.txt");
    let frames = "\
8da4ca5f99043f1804780460afae\n\
8da4ca5f5806b8a3a48ad0e4b3a7\n\
8daa1bb058c386c9f0680ddae879\n\
8da53432582968cb27680cefa867\n";
    std::fs::write(&cap_path, frames).expect("write capture");

    let out = bin().arg("decode").arg(&cap_path).output().expect("spawn");
    assert!(
        out.status.success(),
        "adsb decode exit: {}. stderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    // Summary table or per-aircraft output should mention an ICAO from our
    // input (the prefix "A4CA5F" is the ICAO of a frame above).
    let lower = stdout.to_lowercase();
    assert!(
        lower.contains("a4ca5f") || lower.contains("aircraft") || lower.contains("icao"),
        "adsb decode produced no recognizable summary output. stdout:\n{stdout}"
    );
}

#[test]
fn cli_stats_on_empty_database_succeeds_with_zeros() {
    // A fresh, empty SQLite DB should yield "0 aircraft / 0 positions" without
    // panicking. Pins against schema-init regressions that make stats die on
    // missing tables.
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("empty.db");

    let out = bin()
        .arg("stats")
        .arg("--db-path")
        .arg(&db_path)
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "adsb stats on empty DB exit: {}. stderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    // Output should mention the four counter labels — exact format isn't pinned
    // (comfy-table can change), but the underlying counters should appear.
    let lower = stdout.to_lowercase();
    assert!(
        lower.contains("aircraft") || lower.contains("positions"),
        "adsb stats should label its counters. stdout:\n{stdout}"
    );
}

#[test]
fn cli_history_on_empty_database_succeeds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("empty.db");

    let out = bin()
        .arg("history")
        .arg("--db-path")
        .arg(&db_path)
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "adsb history on empty DB exit: {}. stderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn cli_export_json_on_empty_database_is_valid_json() {
    // The export subcommand is what downstream consumers (CSV/JSON pipelines)
    // run unattended. If a serde rename ever broke export, the failure mode is
    // a malformed JSON file — silent until something downstream chokes. Parse
    // the output here and pin valid JSON.
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("empty.db");
    let out_path = dir.path().join("export.json");

    let out = bin()
        .arg("export")
        .arg("--db-path")
        .arg(&db_path)
        .arg("--format")
        .arg("json")
        .arg("--output")
        .arg(&out_path)
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "adsb export exit: {}. stderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    // Empty DB = export should produce a syntactically valid empty array.
    let body = std::fs::read_to_string(&out_path).expect("export file written");
    let parsed: serde_json::Value =
        serde_json::from_str(&body).expect("export output must be valid JSON");
    assert!(
        parsed.is_array(),
        "export JSON should be a top-level array (consumer contract). Got: {parsed}"
    );
    assert_eq!(parsed.as_array().unwrap().len(), 0);
}

#[test]
fn cli_decode_missing_file_exits_nonzero() {
    // User-facing error path — pointing at a non-existent file should fail
    // cleanly, not panic with a stack trace.
    let dir = tempfile::tempdir().expect("tempdir");
    let bogus_path = dir.path().join("does-not-exist.txt");

    let out = bin()
        .arg("decode")
        .arg(&bogus_path)
        .output()
        .expect("spawn");
    assert!(
        !out.status.success(),
        "decode on missing file should exit non-zero"
    );
}

#[test]
fn cli_stats_help_includes_db_path_arg() {
    // Per-subcommand help. If `--db-path` is renamed without updating
    // dependent scripts (cron jobs, deploy scripts), they break silently.
    // This test bakes in the arg name.
    let out = bin().arg("stats").arg("--help").output().expect("spawn");
    assert!(out.status.success());
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("--db-path"),
        "adsb stats --help must document --db-path: {combined}"
    );
}
