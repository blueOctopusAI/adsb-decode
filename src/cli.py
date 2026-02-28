"""Click CLI — the main entry point for adsb-decode.

Commands:
  adsb decode FILE        Decode a capture file and print aircraft table
  adsb track FILE         Track aircraft from a capture file with persistence
  adsb stats              Show database statistics
  adsb history ICAO       Show history for a specific aircraft
  adsb export             Export data (--format csv|json|kml|geojson)
  adsb serve              Launch web dashboard
"""

from __future__ import annotations

from pathlib import Path

import click
from rich.console import Console
from rich.table import Table

from .capture import FrameReader, LiveCapture
from .database import Database
from .frame_parser import parse_frame
from .tracker import Tracker
from . import exporters
from .filters import FilterEngine, Geofence

console = Console()

DEFAULT_DB = Path("data/adsb.db")


@click.group()
@click.version_option(version="0.1.0", prog_name="adsb-decode")
def cli():
    """ADS-B radio protocol decoder — from raw RF to identified aircraft."""


@cli.command()
@click.argument("file", type=click.Path(exists=True))
@click.option("--ref-lat", type=float, default=None, help="Receiver latitude for local CPR decode")
@click.option("--ref-lon", type=float, default=None, help="Receiver longitude for local CPR decode")
def decode(file: str, ref_lat: float | None, ref_lon: float | None):
    """Decode a capture file and print aircraft table."""
    tracker = Tracker(ref_lat=ref_lat, ref_lon=ref_lon)
    reader = FrameReader(file)

    for raw_frame in reader:
        frame = parse_frame(raw_frame.hex_str, timestamp=raw_frame.timestamp)
        if frame:
            tracker.update(frame)

    _print_aircraft_table(tracker)
    _print_summary(tracker)


@cli.command()
@click.argument("file", type=click.Path(exists=True), required=False)
@click.option("--live", is_flag=True, help="Live capture from RTL-SDR dongle")
@click.option("--db-path", type=click.Path(), default=str(DEFAULT_DB), help="Database file path")
@click.option("--ref-lat", type=float, default=None, help="Receiver latitude")
@click.option("--ref-lon", type=float, default=None, help="Receiver longitude")
@click.option("--receiver", type=str, default="default", help="Receiver name")
@click.option("--port", type=int, default=None, help="Launch web dashboard on this port")
def track(file: str | None, live: bool, db_path: str, ref_lat: float | None,
          ref_lon: float | None, receiver: str, port: int | None):
    """Track aircraft from a capture file or live RTL-SDR.

    \b
    Examples:
      adsb track data/capture.txt                    # From file
      adsb track --live --port 8080                  # Live with dashboard
      adsb track --live --ref-lat 35.18 --ref-lon -83.43  # Live with position
    """
    if not file and not live:
        raise click.UsageError("Provide a FILE or use --live for RTL-SDR capture")

    import os
    os.makedirs(Path(db_path).parent, exist_ok=True)

    db = Database(db_path)
    source = "rtl_adsb:live" if live else file
    rid = db.add_receiver(receiver, lat=ref_lat, lon=ref_lon)
    cap_id = db.start_capture(source=source, receiver_id=rid)

    filter_engine = FilterEngine()
    tracker = Tracker(
        db=db, receiver_id=rid, capture_id=cap_id,
        ref_lat=ref_lat, ref_lon=ref_lon,
    )

    # Start web dashboard in background thread if requested
    if port:
        import threading
        from .web.app import create_app
        app = create_app(db_path=db_path)
        thread = threading.Thread(
            target=lambda: app.run(host="127.0.0.1", port=port, use_reloader=False),
            daemon=True,
        )
        thread.start()
        console.print(f"[bold green]Dashboard[/] → http://127.0.0.1:{port}")

    frame_source = LiveCapture() if live else FrameReader(file)

    try:
        if live:
            console.print("[bold green]Live tracking started[/] — Ctrl+C to stop\n")

        last_print = 0.0
        for raw_frame in frame_source:
            frame = parse_frame(raw_frame.hex_str, timestamp=raw_frame.timestamp)
            if frame:
                msg = tracker.update(frame)
                if msg:
                    ac = tracker.aircraft.get(msg.icao)
                    if ac:
                        events = filter_engine.check(ac)
                        for event in events:
                            db.add_event(
                                icao=event.icao,
                                event_type=event.event_type,
                                description=event.description,
                                lat=event.lat,
                                lon=event.lon,
                                altitude_ft=event.altitude_ft,
                                timestamp=event.timestamp,
                            )
                            console.print(f"  [bold red]EVENT:[/] {event.description}")

            # In live mode, print status every 10 seconds
            if live:
                import time as _time
                now = _time.time()
                if now - last_print > 10:
                    active = tracker.get_active()
                    console.print(
                        f"  [dim]{tracker.total_frames} frames, "
                        f"{tracker.valid_frames} valid, "
                        f"{len(active)} active aircraft, "
                        f"{tracker.position_decodes} positions[/]"
                    )
                    last_print = now
                    tracker.prune_stale()

    except KeyboardInterrupt:
        console.print("\n[yellow]Stopping...[/]")
        if live and isinstance(frame_source, LiveCapture):
            frame_source.stop()

    db.end_capture(
        cap_id,
        total_frames=tracker.total_frames,
        valid_frames=tracker.valid_frames,
        aircraft_count=len(tracker.aircraft),
    )

    _print_aircraft_table(tracker)
    _print_summary(tracker)
    console.print(f"\nDatabase: {db_path}")
    db.close()


@cli.command()
@click.option("--db-path", type=click.Path(exists=True), default=str(DEFAULT_DB), help="Database path")
def stats(db_path: str):
    """Show database statistics."""
    db = Database(db_path)
    s = db.stats()

    table = Table(title="Database Statistics")
    table.add_column("Metric", style="cyan")
    table.add_column("Count", style="green", justify="right")

    table.add_row("Aircraft", str(s["aircraft"]))
    table.add_row("Positions", str(s["positions"]))
    table.add_row("Events", str(s["events"]))
    table.add_row("Receivers", str(s["receivers"]))
    table.add_row("Captures", str(s["captures"]))

    console.print(table)
    db.close()


@cli.command()
@click.argument("icao")
@click.option("--db-path", type=click.Path(exists=True), default=str(DEFAULT_DB), help="Database path")
@click.option("--limit", type=int, default=20, help="Number of positions to show")
def history(icao: str, db_path: str, limit: int):
    """Show history for a specific aircraft."""
    db = Database(db_path)
    icao = icao.upper()
    ac = db.get_aircraft(icao)

    if not ac:
        console.print(f"[red]Aircraft {icao} not found in database[/]")
        db.close()
        return

    console.print(f"\n[bold]Aircraft {icao}[/]")
    console.print(f"  Country: {ac['country'] or 'Unknown'}")
    console.print(f"  Registration: {ac['registration'] or 'Unknown'}")
    console.print(f"  Military: {'Yes' if ac['is_military'] else 'No'}")

    positions = db.get_positions(icao, limit=limit)
    if positions:
        table = Table(title=f"Recent Positions ({len(positions)})")
        table.add_column("Lat", justify="right")
        table.add_column("Lon", justify="right")
        table.add_column("Alt (ft)", justify="right")
        table.add_column("Speed (kts)", justify="right")
        table.add_column("Heading", justify="right")
        table.add_column("VRate", justify="right")

        for p in positions:
            table.add_row(
                f"{p['lat']:.4f}" if p['lat'] else "-",
                f"{p['lon']:.4f}" if p['lon'] else "-",
                str(p['altitude_ft'] or "-"),
                f"{p['speed_kts']:.0f}" if p['speed_kts'] else "-",
                f"{p['heading_deg']:.0f}°" if p['heading_deg'] else "-",
                str(p['vertical_rate_fpm'] or "-"),
            )
        console.print(table)
    else:
        console.print("  No positions recorded.")

    events = db.get_events()
    ac_events = [e for e in events if e["icao"] == icao]
    if ac_events:
        console.print(f"\n[bold]Events ({len(ac_events)})[/]")
        for e in ac_events:
            console.print(f"  [{e['event_type']}] {e['description']}")

    db.close()


@cli.command("export")
@click.option("--db-path", type=click.Path(exists=True), default=str(DEFAULT_DB), help="Database path")
@click.option("--format", "fmt", type=click.Choice(["csv", "json", "kml", "geojson"]), default="json")
@click.option("--output", "-o", type=click.Path(), default=None, help="Output file path")
def export_cmd(db_path: str, fmt: str, output: str | None):
    """Export data in various formats."""
    db = Database(db_path)

    export_fn = {
        "csv": exporters.export_csv,
        "json": exporters.export_json,
        "kml": exporters.export_kml,
        "geojson": exporters.export_geojson,
    }[fmt]

    if output:
        export_fn(db, path=output)
        console.print(f"Exported {fmt.upper()} to {output}")
    else:
        text = export_fn(db)
        click.echo(text)

    db.close()


@cli.command()
@click.option("--db-path", type=click.Path(), default=str(DEFAULT_DB), help="Database path")
@click.option("--host", default="127.0.0.1", help="Host to bind")
@click.option("--port", type=int, default=8080, help="Port to bind")
@click.option("--debug", is_flag=True, help="Enable Flask debug mode")
def serve(db_path: str, host: str, port: int, debug: bool):
    """Launch web dashboard."""
    from .web.app import create_app

    app = create_app(db_path=db_path)
    console.print(f"[bold green]adsb-decode dashboard[/] → http://{host}:{port}")
    app.run(host=host, port=port, debug=debug)


def _print_aircraft_table(tracker: Tracker):
    """Print Rich table of all tracked aircraft."""
    table = Table(title="Aircraft")
    table.add_column("ICAO", style="cyan")
    table.add_column("Callsign")
    table.add_column("Country")
    table.add_column("Reg")
    table.add_column("Squawk")
    table.add_column("Lat", justify="right")
    table.add_column("Lon", justify="right")
    table.add_column("Alt (ft)", justify="right")
    table.add_column("Speed", justify="right")
    table.add_column("Hdg", justify="right")
    table.add_column("VRate", justify="right")
    table.add_column("Msgs", justify="right")

    for ac in sorted(tracker.aircraft.values(), key=lambda a: a.message_count, reverse=True):
        mil_marker = " [red]MIL[/]" if ac.is_military else ""
        table.add_row(
            ac.icao + mil_marker,
            ac.callsign or "-",
            ac.country or "-",
            ac.registration or "-",
            ac.squawk or "-",
            f"{ac.lat:.4f}" if ac.lat is not None else "-",
            f"{ac.lon:.4f}" if ac.lon is not None else "-",
            str(ac.altitude_ft) if ac.altitude_ft is not None else "-",
            f"{ac.speed_kts:.0f}" if ac.speed_kts is not None else "-",
            f"{ac.heading_deg:.0f}°" if ac.heading_deg is not None else "-",
            str(ac.vertical_rate_fpm) if ac.vertical_rate_fpm is not None else "-",
            str(ac.message_count),
        )

    console.print(table)


def _print_summary(tracker: Tracker):
    """Print decode summary."""
    console.print(f"\n[bold]Summary:[/]")
    console.print(f"  Total frames:     {tracker.total_frames}")
    console.print(f"  Valid frames:     {tracker.valid_frames}")
    console.print(f"  Position decodes: {tracker.position_decodes}")
    console.print(f"  Aircraft seen:    {len(tracker.aircraft)}")
