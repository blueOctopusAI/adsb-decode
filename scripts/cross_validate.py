#!/usr/bin/env python3
"""Cross-validate Python vs Rust ADS-B decoders.

Processes the same capture file through both implementations and compares
every decoded field per aircraft. Reports mismatches and summary.

Usage:
    python scripts/cross_validate.py data/live_capture.txt
"""

import json
import re
import subprocess
import sys
from pathlib import Path

PROJECT_ROOT = Path(__file__).parent.parent

def run_python_decode(capture_file: str) -> dict:
    """Run Python decoder and extract per-aircraft state."""
    # Import Python decoder directly
    sys.path.insert(0, str(PROJECT_ROOT))
    from src.capture import FrameReader
    from src.frame_parser import parse_frame
    from src.tracker import Tracker

    tracker = Tracker()
    reader = FrameReader(capture_file)

    for raw_frame in reader:
        frame = parse_frame(raw_frame.hex_str, timestamp=raw_frame.timestamp)
        if frame:
            tracker.update(frame)

    result = {}
    for icao_str, ac in tracker.aircraft.items():
        result[icao_str.upper()] = {
            "callsign": ac.callsign,
            "squawk": ac.squawk,
            "altitude_ft": ac.altitude_ft,
            "speed_kts": round(ac.speed_kts, 1) if ac.speed_kts is not None else None,
            "heading_deg": round(ac.heading_deg, 1) if ac.heading_deg is not None else None,
            "vertical_rate": ac.vertical_rate_fpm,
            "lat": round(ac.lat, 4) if ac.lat is not None else None,
            "lon": round(ac.lon, 4) if ac.lon is not None else None,
            "messages": ac.message_count,
        }

    return {
        "total_frames": tracker.total_frames,
        "valid_frames": tracker.valid_frames,
        "aircraft_count": len(tracker.aircraft),
        "aircraft": result,
    }


def run_rust_decode(capture_file: str) -> dict:
    """Run Rust decoder and parse table output into structured data."""
    rust_bin = PROJECT_ROOT / "rust" / "target" / "debug" / "adsb"
    if not rust_bin.exists():
        # Build first
        subprocess.run(
            ["cargo", "build", "--quiet"],
            cwd=PROJECT_ROOT / "rust",
            check=True,
        )

    proc = subprocess.run(
        [str(rust_bin), "decode", capture_file],
        capture_output=True,
        text=True,
    )

    output = proc.stdout
    stderr = proc.stderr

    # Parse summary line: "Frames: X parsed, Y decoded, Z aircraft"
    summary_match = re.search(
        r"Frames:\s+(\d+)\s+parsed,\s+(\d+)\s+decoded,\s+(\d+)\s+aircraft",
        output,
    )
    if not summary_match:
        print("ERROR: Could not parse Rust summary line")
        print("STDOUT:", output[:500])
        print("STDERR:", stderr[:500])
        sys.exit(1)

    total_frames = int(summary_match.group(1))
    decoded_frames = int(summary_match.group(2))
    aircraft_count = int(summary_match.group(3))

    # Parse comfy_table output
    # Find header line and data lines
    # comfy_table uses box-drawing characters: │, ─, ┌, etc.
    # or ASCII: | - +
    lines = output.strip().split("\n")

    # Find header row — contains "ICAO"
    header_idx = None
    for i, line in enumerate(lines):
        if "ICAO" in line and "Callsign" in line:
            header_idx = i
            break

    if header_idx is None:
        print("ERROR: Could not find table header in Rust output")
        print(output[:1000])
        sys.exit(1)

    # Parse header columns
    header_line = lines[header_idx]
    # Split on │ or |
    sep = "│" if "│" in header_line else "|"
    headers = [h.strip() for h in header_line.split(sep) if h.strip()]

    # Map header names to indices
    col_map = {}
    for i, h in enumerate(headers):
        col_map[h] = i

    # Parse data rows (skip header and separator lines)
    aircraft = {}
    for line in lines[header_idx + 1 :]:
        if not line.strip():
            continue
        # Skip separator lines (contain only box-drawing chars, dashes, or plusses)
        stripped = line.strip()
        if all(c in "─━═+-|│┌┐└┘├┤┬┴┼ " for c in stripped):
            continue
        if sep not in line:
            continue

        cells = [c.strip() for c in line.split(sep)]
        # Remove empty edge cells from leading/trailing separators
        cells = [c for c in cells if c != ""]
        if len(cells) < len(headers):
            continue

        def get(name):
            idx = col_map.get(name)
            if idx is None or idx >= len(cells):
                return None
            val = cells[idx]
            return None if val == "-" else val

        icao = get("ICAO")
        if not icao:
            continue

        def parse_float(s):
            if s is None:
                return None
            try:
                return float(s)
            except ValueError:
                return None

        def parse_int(s):
            if s is None:
                return None
            try:
                # Handle "+100" format
                return int(s.replace("+", ""))
            except ValueError:
                return None

        aircraft[icao.upper()] = {
            "callsign": get("Callsign"),
            "squawk": get("Squawk"),
            "altitude_ft": parse_int(get("Alt (ft)")),
            "speed_kts": parse_float(get("Speed (kts)")),
            "heading_deg": parse_float(get("Hdg")),
            "vertical_rate": parse_int(get("VRate")),
            "lat": parse_float(get("Lat")),
            "lon": parse_float(get("Lon")),
            "messages": parse_int(get("Msgs")),
        }

    return {
        "total_frames": total_frames,
        "decoded_frames": decoded_frames,
        "aircraft_count": aircraft_count,
        "aircraft": aircraft,
    }


def compare(python_data: dict, rust_data: dict) -> tuple[int, int]:
    """Compare Python vs Rust results field-by-field. Returns (matches, mismatches)."""
    matches = 0
    mismatches = 0

    print("=" * 72)
    print("CROSS-VALIDATION: Python vs Rust ADS-B Decoder")
    print("=" * 72)
    print()

    # Summary comparison
    print("--- Summary ---")
    py_frames = python_data["total_frames"]
    rs_frames = rust_data["total_frames"]
    py_ac = python_data["aircraft_count"]
    rs_ac = rust_data["aircraft_count"]

    status = "MATCH" if py_frames == rs_frames else "MISMATCH"
    sym = "  " if status == "MATCH" else "**"
    print(f"{sym}Total frames:   Python={py_frames}, Rust={rs_frames} [{status}]")
    if status == "MATCH":
        matches += 1
    else:
        mismatches += 1

    status = "MATCH" if py_ac == rs_ac else "MISMATCH"
    sym = "  " if status == "MATCH" else "**"
    print(f"{sym}Aircraft count: Python={py_ac}, Rust={rs_ac} [{status}]")
    if status == "MATCH":
        matches += 1
    else:
        mismatches += 1

    print()

    # Per-aircraft comparison
    py_aircraft = python_data["aircraft"]
    rs_aircraft = rust_data["aircraft"]

    all_icaos = sorted(set(list(py_aircraft.keys()) + list(rs_aircraft.keys())))

    py_only = [i for i in all_icaos if i in py_aircraft and i not in rs_aircraft]
    rs_only = [i for i in all_icaos if i not in py_aircraft and i in rs_aircraft]
    common = [i for i in all_icaos if i in py_aircraft and i in rs_aircraft]

    if py_only:
        print(f"** Python-only aircraft ({len(py_only)}): {', '.join(py_only)}")
        mismatches += len(py_only)
    if rs_only:
        print(f"** Rust-only aircraft ({len(rs_only)}): {', '.join(rs_only)}")
        mismatches += len(rs_only)

    # Compare fields for common aircraft
    fields = ["callsign", "squawk", "altitude_ft", "speed_kts", "heading_deg",
              "vertical_rate", "lat", "lon", "messages"]

    aircraft_mismatches = {}

    for icao in common:
        py_ac = py_aircraft[icao]
        rs_ac = rs_aircraft[icao]
        ac_mismatches = []

        for field in fields:
            py_val = py_ac.get(field)
            rs_val = rs_ac.get(field)

            # Normalize: strip trailing spaces from callsign
            if field == "callsign" and isinstance(py_val, str):
                py_val = py_val.strip()
            if field == "callsign" and isinstance(rs_val, str):
                rs_val = rs_val.strip()

            # Float comparison with tolerance
            if isinstance(py_val, float) and isinstance(rs_val, float):
                if abs(py_val - rs_val) < 0.15:  # Allow small rounding difference
                    matches += 1
                    continue
                else:
                    ac_mismatches.append((field, py_val, rs_val))
                    mismatches += 1
                    continue

            if py_val == rs_val:
                matches += 1
            else:
                ac_mismatches.append((field, py_val, rs_val))
                mismatches += 1

        if ac_mismatches:
            aircraft_mismatches[icao] = ac_mismatches

    print()
    print(f"--- Per-Aircraft Field Comparison ({len(common)} common aircraft) ---")
    print()

    if not aircraft_mismatches:
        print("  ALL FIELDS MATCH across all aircraft!")
    else:
        for icao, mm in sorted(aircraft_mismatches.items()):
            print(f"  {icao}:")
            for field, py_val, rs_val in mm:
                print(f"    ** {field}: Python={py_val!r}, Rust={rs_val!r}")

    print()
    print("=" * 72)
    total = matches + mismatches
    pct = matches / total * 100 if total > 0 else 0
    status = "PASS" if mismatches == 0 else "FAIL"
    print(f"Result: {status} — {matches}/{total} fields match ({pct:.1f}%)")
    if mismatches > 0:
        print(f"  {mismatches} mismatches found")
    print("=" * 72)

    return matches, mismatches


def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <capture_file>")
        sys.exit(1)

    capture_file = sys.argv[1]
    if not Path(capture_file).exists():
        print(f"Error: {capture_file} not found")
        sys.exit(1)

    print(f"Capture file: {capture_file}")
    print(f"Lines: {sum(1 for _ in open(capture_file))}")
    print()

    print("Running Python decoder...")
    py_data = run_python_decode(capture_file)
    print(f"  -> {py_data['total_frames']} frames, {py_data['aircraft_count']} aircraft")

    print("Running Rust decoder...")
    rs_data = run_rust_decode(capture_file)
    print(f"  -> {rs_data['total_frames']} frames, {rs_data['aircraft_count']} aircraft")
    print()

    matches, mismatches = compare(py_data, rs_data)

    # Dump raw data for debugging
    debug_file = PROJECT_ROOT / "scripts" / "cross_validate_debug.json"
    with open(debug_file, "w") as f:
        json.dump({"python": py_data, "rust": rs_data}, f, indent=2)
    print(f"\nDebug data: {debug_file}")

    sys.exit(0 if mismatches == 0 else 1)


if __name__ == "__main__":
    main()
