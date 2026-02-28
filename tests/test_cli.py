"""Tests for CLI commands."""

import pytest
from click.testing import CliRunner

from src.cli import cli
from src.database import Database


@pytest.fixture
def runner():
    return CliRunner()


@pytest.fixture
def hex_file(tmp_path):
    """Create a sample hex frame file."""
    frames = [
        "8D4840D6202CC371C32CE0576098",  # Identification: KLM1023
        "8D40621D58C382D690C8AC2863A7",  # Position even
        "8D40621D58C386435CC412692AD6",  # Position odd
        "8D485020994409940838175B284F",  # Velocity
    ]
    f = tmp_path / "test_frames.txt"
    f.write_text("\n".join(frames) + "\n")
    return str(f)


@pytest.fixture
def db_file(tmp_path):
    """Create a database with sample data."""
    db_path = tmp_path / "test.db"
    db = Database(db_path)
    db.upsert_aircraft("A00001", country="United States", registration="N12345", timestamp=1000.0)
    db.add_position("A00001", lat=35.18, lon=-83.38, altitude_ft=38000, timestamp=1000.0)
    db.add_receiver("test-rx")
    db.start_capture(source="test")
    db.close()
    return str(db_path)


class TestDecodeCommand:
    def test_decode_file(self, runner, hex_file):
        result = runner.invoke(cli, ["decode", hex_file])
        assert result.exit_code == 0
        assert "Aircraft" in result.output
        assert "Summary" in result.output

    def test_decode_with_ref(self, runner, hex_file):
        result = runner.invoke(cli, ["decode", hex_file, "--ref-lat", "52.0", "--ref-lon", "4.0"])
        assert result.exit_code == 0

    def test_decode_nonexistent_file(self, runner):
        result = runner.invoke(cli, ["decode", "/nonexistent/file.txt"])
        assert result.exit_code != 0


class TestTrackCommand:
    def test_track_creates_db(self, runner, hex_file, tmp_path):
        db_path = str(tmp_path / "track.db")
        result = runner.invoke(cli, ["track", hex_file, "--db-path", db_path])
        assert result.exit_code == 0
        # Verify DB was created with data
        db = Database(db_path)
        assert db.count_aircraft() > 0
        db.close()

    def test_track_with_receiver(self, runner, hex_file, tmp_path):
        db_path = str(tmp_path / "track.db")
        result = runner.invoke(cli, [
            "track", hex_file, "--db-path", db_path,
            "--receiver", "home", "--ref-lat", "35.18", "--ref-lon", "-83.38"
        ])
        assert result.exit_code == 0


class TestStatsCommand:
    def test_stats(self, runner, db_file):
        result = runner.invoke(cli, ["stats", "--db-path", db_file])
        assert result.exit_code == 0
        assert "Aircraft" in result.output

    def test_stats_nonexistent_db(self, runner, tmp_path):
        result = runner.invoke(cli, ["stats", "--db-path", str(tmp_path / "nope.db")])
        assert result.exit_code != 0


class TestHistoryCommand:
    def test_history(self, runner, db_file):
        result = runner.invoke(cli, ["history", "A00001", "--db-path", db_file])
        assert result.exit_code == 0
        assert "United States" in result.output

    def test_history_not_found(self, runner, db_file):
        result = runner.invoke(cli, ["history", "FFFFFF", "--db-path", db_file])
        assert result.exit_code == 0
        assert "not found" in result.output


class TestExportCommand:
    def test_export_json(self, runner, db_file):
        result = runner.invoke(cli, ["export", "--db-path", db_file, "--format", "json"])
        assert result.exit_code == 0
        assert "aircraft" in result.output

    def test_export_csv(self, runner, db_file):
        result = runner.invoke(cli, ["export", "--db-path", db_file, "--format", "csv"])
        assert result.exit_code == 0
        assert "icao" in result.output

    def test_export_kml(self, runner, db_file):
        result = runner.invoke(cli, ["export", "--db-path", db_file, "--format", "kml"])
        assert result.exit_code == 0
        assert "kml" in result.output.lower()

    def test_export_geojson(self, runner, db_file):
        result = runner.invoke(cli, ["export", "--db-path", db_file, "--format", "geojson"])
        assert result.exit_code == 0
        assert "FeatureCollection" in result.output

    def test_export_to_file(self, runner, db_file, tmp_path):
        out = str(tmp_path / "out.json")
        result = runner.invoke(cli, ["export", "--db-path", db_file, "--format", "json", "-o", out])
        assert result.exit_code == 0
        assert "Exported" in result.output


class TestVersionFlag:
    def test_version(self, runner):
        result = runner.invoke(cli, ["--version"])
        assert result.exit_code == 0
        assert "0.1.0" in result.output
