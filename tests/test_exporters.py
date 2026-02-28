"""Tests for data exporters — CSV, JSON, KML, GeoJSON."""

import json
import xml.etree.ElementTree as ET

import pytest

from src.database import Database
from src.exporters import export_csv, export_json, export_kml, export_geojson


@pytest.fixture
def db(tmp_path):
    """Database with sample data."""
    d = Database(tmp_path / "export.db")
    _ = d.conn

    d.upsert_aircraft("A00001", country="United States", registration="N12345", timestamp=1000.0)
    d.upsert_aircraft("480001", country="Netherlands", timestamp=1000.0)

    d.add_position("A00001", lat=35.18, lon=-83.38, altitude_ft=38000,
                   speed_kts=450.0, heading_deg=90.0, vertical_rate_fpm=-500, timestamp=1000.0)
    d.add_position("A00001", lat=35.20, lon=-83.30, altitude_ft=37500,
                   speed_kts=448.0, heading_deg=92.0, timestamp=1001.0)
    d.add_position("480001", lat=52.25, lon=3.92, altitude_ft=35000, timestamp=1000.0)

    yield d
    d.close()


@pytest.fixture
def empty_db(tmp_path):
    """Empty database."""
    d = Database(tmp_path / "empty.db")
    _ = d.conn
    yield d
    d.close()


class TestCSVExport:
    def test_returns_string(self, db):
        result = export_csv(db)
        assert isinstance(result, str)

    def test_has_header(self, db):
        result = export_csv(db)
        header = result.split("\n")[0]
        assert "icao" in header
        assert "lat" in header
        assert "lon" in header

    def test_has_data_rows(self, db):
        result = export_csv(db)
        lines = [l for l in result.strip().split("\n") if l]
        assert len(lines) == 4  # Header + 3 positions

    def test_writes_to_file(self, db, tmp_path):
        path = tmp_path / "out.csv"
        export_csv(db, path=path)
        assert path.exists()
        assert "A00001" in path.read_text()

    def test_empty_db(self, empty_db):
        result = export_csv(empty_db)
        lines = [l for l in result.strip().split("\n") if l]
        assert len(lines) == 1  # Header only


class TestJSONExport:
    def test_returns_valid_json(self, db):
        result = export_json(db)
        data = json.loads(result)
        assert "aircraft" in data
        assert "stats" in data

    def test_aircraft_count(self, db):
        data = json.loads(export_json(db))
        assert len(data["aircraft"]) == 2

    def test_positions_included(self, db):
        data = json.loads(export_json(db))
        a00001 = next(a for a in data["aircraft"] if a["icao"] == "A00001")
        assert len(a00001["positions"]) == 2

    def test_aircraft_fields(self, db):
        data = json.loads(export_json(db))
        ac = data["aircraft"][0]
        assert "icao" in ac
        assert "country" in ac
        assert "registration" in ac
        assert "is_military" in ac

    def test_writes_to_file(self, db, tmp_path):
        path = tmp_path / "out.json"
        export_json(db, path=path)
        assert path.exists()
        data = json.loads(path.read_text())
        assert len(data["aircraft"]) == 2

    def test_empty_db(self, empty_db):
        data = json.loads(export_json(empty_db))
        assert len(data["aircraft"]) == 0


class TestKMLExport:
    def test_returns_valid_xml(self, db):
        result = export_kml(db)
        root = ET.fromstring(result)
        assert "kml" in root.tag

    def test_has_placemarks(self, db):
        result = export_kml(db)
        root = ET.fromstring(result)
        ns = {"kml": "http://www.opengis.net/kml/2.2"}
        placemarks = root.findall(".//kml:Placemark", ns)
        assert len(placemarks) >= 1

    def test_linestring_has_coordinates(self, db):
        result = export_kml(db)
        root = ET.fromstring(result)
        ns = {"kml": "http://www.opengis.net/kml/2.2"}
        coords = root.find(".//kml:coordinates", ns)
        assert coords is not None
        assert coords.text.strip()

    def test_writes_to_file(self, db, tmp_path):
        path = tmp_path / "out.kml"
        export_kml(db, path=path)
        assert path.exists()

    def test_empty_db(self, empty_db):
        result = export_kml(empty_db)
        root = ET.fromstring(result)
        ns = {"kml": "http://www.opengis.net/kml/2.2"}
        placemarks = root.findall(".//kml:Placemark", ns)
        assert len(placemarks) == 0


class TestGeoJSONExport:
    def test_returns_valid_geojson(self, db):
        data = json.loads(export_geojson(db))
        assert data["type"] == "FeatureCollection"
        assert "features" in data

    def test_has_point_features(self, db):
        data = json.loads(export_geojson(db))
        points = [f for f in data["features"] if f["geometry"]["type"] == "Point"]
        assert len(points) >= 1

    def test_has_linestring_for_multi_position(self, db):
        data = json.loads(export_geojson(db))
        lines = [f for f in data["features"] if f["geometry"]["type"] == "LineString"]
        # A00001 has 2 positions, should get a LineString
        assert len(lines) >= 1

    def test_single_position_no_linestring(self, db):
        """480001 has only 1 position — no LineString."""
        data = json.loads(export_geojson(db))
        lines = [
            f for f in data["features"]
            if f["geometry"]["type"] == "LineString"
            and f["properties"]["icao"] == "480001"
        ]
        assert len(lines) == 0

    def test_properties_included(self, db):
        data = json.loads(export_geojson(db))
        point = next(f for f in data["features"] if f["geometry"]["type"] == "Point")
        assert "icao" in point["properties"]
        assert "country" in point["properties"]

    def test_writes_to_file(self, db, tmp_path):
        path = tmp_path / "out.geojson"
        export_geojson(db, path=path)
        assert path.exists()
        data = json.loads(path.read_text())
        assert data["type"] == "FeatureCollection"

    def test_empty_db(self, empty_db):
        data = json.loads(export_geojson(empty_db))
        assert len(data["features"]) == 0
