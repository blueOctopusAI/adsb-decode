"""Tests for aircraft type enrichment — profile classification and type DB."""

import pytest

from src.enrichment import (
    classify_from_profile,
    lookup_operator,
    AircraftTypeDB,
    nearest_airport,
    classify_flight_phase,
    CAT_JET,
    CAT_PROP,
    CAT_TURBOPROP,
    CAT_HELICOPTER,
    CAT_MILITARY,
    CAT_CARGO,
    CAT_UNKNOWN,
)


class TestProfileClassification:
    def test_high_speed_is_jet(self):
        assert classify_from_profile(speed_kts=450) == CAT_JET

    def test_low_speed_low_alt_is_prop(self):
        assert classify_from_profile(speed_kts=120, altitude_ft=3000) == CAT_PROP

    def test_very_slow_low_alt_is_helicopter(self):
        assert classify_from_profile(speed_kts=60, altitude_ft=1500) == CAT_HELICOPTER

    def test_medium_speed_high_alt_is_turboprop(self):
        assert classify_from_profile(speed_kts=150, altitude_ft=20000) == CAT_TURBOPROP

    def test_military_flag_overrides(self):
        assert classify_from_profile(speed_kts=450, is_military=True) == CAT_MILITARY

    def test_cargo_callsign(self):
        assert classify_from_profile(callsign="UPS1234") == CAT_CARGO
        assert classify_from_profile(callsign="FDX456") == CAT_CARGO

    def test_high_altitude_fallback(self):
        assert classify_from_profile(altitude_ft=35000) == CAT_JET

    def test_low_altitude_fallback(self):
        assert classify_from_profile(altitude_ft=2000) == CAT_PROP

    def test_no_data_is_unknown(self):
        assert classify_from_profile() == CAT_UNKNOWN

    def test_medium_speed_range(self):
        assert classify_from_profile(speed_kts=200) == CAT_TURBOPROP


class TestOperatorLookup:
    def test_known_airline(self):
        assert lookup_operator("AAL123") == "American Airlines"
        assert lookup_operator("DAL456") == "Delta Air Lines"
        assert lookup_operator("SWA789") == "Southwest Airlines"

    def test_unknown_callsign(self):
        assert lookup_operator("XYZ999") is None

    def test_short_callsign(self):
        assert lookup_operator("AB") is None

    def test_none_callsign(self):
        assert lookup_operator(None) is None

    def test_case_insensitive(self):
        assert lookup_operator("aal123") == "American Airlines"


class TestAircraftTypeDB:
    def test_add_and_lookup(self):
        db = AircraftTypeDB()
        db.add("A00001", registration="N12345", type_code="B738",
               type_name="Boeing 737-800", operator="Southwest", category="jet")
        result = db.lookup("A00001")
        assert result is not None
        assert result["type_code"] == "B738"
        assert result["operator"] == "Southwest"
        db.close()

    def test_lookup_missing(self):
        db = AircraftTypeDB()
        assert db.lookup("FFFFFF") is None
        db.close()

    def test_case_insensitive_lookup(self):
        db = AircraftTypeDB()
        db.add("a00001", type_code="C172")
        result = db.lookup("A00001")
        assert result is not None
        db.close()

    def test_upsert(self):
        db = AircraftTypeDB()
        db.add("A00001", type_code="B738")
        db.add("A00001", type_code="B739", operator="United")
        result = db.lookup("A00001")
        assert result["type_code"] == "B739"
        assert result["operator"] == "United"
        db.close()

    def test_count(self):
        db = AircraftTypeDB()
        assert db.count() == 0
        db.add("A00001", type_code="B738")
        db.add("A00002", type_code="A320")
        assert db.count() == 2
        db.close()

    def test_load_csv(self, tmp_path):
        csv_file = tmp_path / "types.csv"
        csv_file.write_text(
            "icao,registration,type_code,type_name,operator,category\n"
            "A00001,N12345,B738,Boeing 737-800,Southwest,jet\n"
            "A00002,N67890,A320,Airbus A320,Delta,jet\n"
        )
        db = AircraftTypeDB()
        loaded = db.load_csv(csv_file)
        assert loaded == 2
        assert db.count() == 2
        r = db.lookup("A00001")
        assert r["registration"] == "N12345"
        db.close()

    def test_load_missing_csv(self):
        db = AircraftTypeDB()
        loaded = db.load_csv("/nonexistent/file.csv")
        assert loaded == 0
        db.close()


class TestAirportAwareness:
    def test_near_asheville(self):
        result = nearest_airport(35.44, -82.54)
        assert result is not None
        code, name, dist = result
        assert code == "KAVL"
        assert dist < 2  # Very close

    def test_near_atlanta(self):
        result = nearest_airport(33.64, -84.43)
        assert result is not None
        assert result[0] == "KATL"

    def test_no_airport_in_range(self):
        # Middle of the ocean
        result = nearest_airport(40.0, -50.0, max_nm=50)
        assert result is None

    def test_approaching(self):
        phase = classify_flight_phase(
            lat=35.42, lon=-82.55,  # Near KAVL
            altitude_ft=5000,
            vertical_rate_fpm=-1000,  # Descending
        )
        assert phase is not None
        assert "Approaching" in phase
        assert "KAVL" in phase

    def test_departing(self):
        phase = classify_flight_phase(
            lat=35.44, lon=-82.54,  # Near KAVL
            altitude_ft=3000,
            vertical_rate_fpm=1500,  # Climbing
        )
        assert phase is not None
        assert "Departing" in phase

    def test_overflying(self):
        phase = classify_flight_phase(
            lat=35.0, lon=-83.0,  # Between airports
            altitude_ft=35000,
            vertical_rate_fpm=0,
            max_airport_nm=100,
        )
        assert phase is not None
        assert "Overflying" in phase

    def test_no_airport_nearby(self):
        phase = classify_flight_phase(
            lat=40.0, lon=-50.0,
            altitude_ft=35000,
            vertical_rate_fpm=0,
        )
        assert phase is None
