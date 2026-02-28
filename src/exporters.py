"""Export aircraft data in multiple formats.

Formats:
- CSV:     Flat position data for spreadsheet analysis
- JSON:    Structured aircraft + position data
- KML:     Google Earth flight paths with altitude (LineString + placemarks)
- GeoJSON: Map-ready FeatureCollection with Point/LineString geometries

All exporters accept a Database instance and write to a file path or return strings.
"""

from __future__ import annotations

import csv
import io
import json
import xml.etree.ElementTree as ET
from pathlib import Path

from .database import Database


def export_csv(db: Database, path: str | Path | None = None) -> str:
    """Export positions as CSV.

    Columns: icao, lat, lon, altitude_ft, speed_kts, heading_deg,
             vertical_rate_fpm, timestamp, receiver_id
    """
    output = io.StringIO()
    writer = csv.writer(output)
    writer.writerow([
        "icao", "lat", "lon", "altitude_ft", "speed_kts",
        "heading_deg", "vertical_rate_fpm", "timestamp", "receiver_id",
    ])

    # Get all aircraft, then their positions
    rows = db.conn.execute(
        "SELECT * FROM positions ORDER BY timestamp DESC"
    ).fetchall()

    for row in rows:
        writer.writerow([
            row["icao"], row["lat"], row["lon"], row["altitude_ft"],
            row["speed_kts"], row["heading_deg"], row["vertical_rate_fpm"],
            row["timestamp"], row["receiver_id"],
        ])

    text = output.getvalue()
    if path:
        Path(path).write_text(text)
    return text


def export_json(db: Database, path: str | Path | None = None) -> str:
    """Export aircraft with positions as JSON.

    Structure: { "aircraft": [ { "icao": ..., "positions": [...] } ] }
    """
    aircraft_rows = db.conn.execute(
        "SELECT * FROM aircraft ORDER BY last_seen DESC"
    ).fetchall()

    aircraft_list = []
    for ac in aircraft_rows:
        positions = db.get_positions(ac["icao"], limit=1000)
        aircraft_list.append({
            "icao": ac["icao"],
            "registration": ac["registration"],
            "country": ac["country"],
            "is_military": bool(ac["is_military"]),
            "first_seen": ac["first_seen"],
            "last_seen": ac["last_seen"],
            "positions": [
                {
                    "lat": p["lat"],
                    "lon": p["lon"],
                    "altitude_ft": p["altitude_ft"],
                    "speed_kts": p["speed_kts"],
                    "heading_deg": p["heading_deg"],
                    "vertical_rate_fpm": p["vertical_rate_fpm"],
                    "timestamp": p["timestamp"],
                }
                for p in positions
            ],
        })

    data = {"aircraft": aircraft_list, "stats": db.stats()}
    text = json.dumps(data, indent=2)
    if path:
        Path(path).write_text(text)
    return text


def export_kml(db: Database, path: str | Path | None = None) -> str:
    """Export flight paths as KML for Google Earth.

    Each aircraft gets a Placemark with a LineString showing its track.
    Altitude is included for 3D visualization.
    """
    kml = ET.Element("kml", xmlns="http://www.opengis.net/kml/2.2")
    doc = ET.SubElement(kml, "Document")
    ET.SubElement(doc, "name").text = "ADS-B Flight Tracks"
    ET.SubElement(doc, "description").text = "Exported from adsb-decode"

    # Style for flight tracks
    style = ET.SubElement(doc, "Style", id="flightTrack")
    line_style = ET.SubElement(style, "LineStyle")
    ET.SubElement(line_style, "color").text = "ff0088ff"  # Orange in ABGR
    ET.SubElement(line_style, "width").text = "2"

    aircraft_rows = db.conn.execute(
        "SELECT * FROM aircraft ORDER BY last_seen DESC"
    ).fetchall()

    for ac in aircraft_rows:
        positions = db.get_positions(ac["icao"], limit=1000)
        if not positions:
            continue

        # Reverse to chronological order (get_positions returns desc)
        positions = list(reversed(positions))

        pm = ET.SubElement(doc, "Placemark")
        label = ac["registration"] or ac["icao"]
        if ac["country"]:
            label = f"{label} ({ac['country']})"
        ET.SubElement(pm, "name").text = label
        ET.SubElement(pm, "styleUrl").text = "#flightTrack"

        line = ET.SubElement(pm, "LineString")
        ET.SubElement(line, "altitudeMode").text = "absolute"
        ET.SubElement(line, "tessellate").text = "1"

        coords_parts = []
        for p in positions:
            alt_m = (p["altitude_ft"] or 0) * 0.3048  # ft to meters
            coords_parts.append(f"{p['lon']},{p['lat']},{alt_m:.0f}")
        ET.SubElement(line, "coordinates").text = " ".join(coords_parts)

    text = ET.tostring(kml, encoding="unicode", xml_declaration=True)
    if path:
        Path(path).write_text(text)
    return text


def export_geojson(db: Database, path: str | Path | None = None) -> str:
    """Export as GeoJSON FeatureCollection.

    Each aircraft produces:
    - A LineString feature for the flight track
    - A Point feature for the last known position
    """
    features = []

    aircraft_rows = db.conn.execute(
        "SELECT * FROM aircraft ORDER BY last_seen DESC"
    ).fetchall()

    for ac in aircraft_rows:
        positions = db.get_positions(ac["icao"], limit=1000)
        if not positions:
            continue

        props = {
            "icao": ac["icao"],
            "registration": ac["registration"],
            "country": ac["country"],
            "is_military": bool(ac["is_military"]),
        }

        # Last known position as Point
        last = positions[0]
        features.append({
            "type": "Feature",
            "geometry": {
                "type": "Point",
                "coordinates": [last["lon"], last["lat"]],
            },
            "properties": {
                **props,
                "altitude_ft": last["altitude_ft"],
                "speed_kts": last["speed_kts"],
                "heading_deg": last["heading_deg"],
                "feature_type": "position",
            },
        })

        # Flight track as LineString (if >1 position)
        if len(positions) > 1:
            # Reverse to chronological
            coords = [
                [p["lon"], p["lat"]] for p in reversed(positions)
            ]
            features.append({
                "type": "Feature",
                "geometry": {
                    "type": "LineString",
                    "coordinates": coords,
                },
                "properties": {
                    **props,
                    "feature_type": "track",
                    "position_count": len(positions),
                },
            })

    collection = {
        "type": "FeatureCollection",
        "features": features,
    }
    text = json.dumps(collection, indent=2)
    if path:
        Path(path).write_text(text)
    return text
