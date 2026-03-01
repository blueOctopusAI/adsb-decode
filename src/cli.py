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
@click.option("--webhook", type=str, default=None, help="Webhook URL for event notifications")
def track(file: str | None, live: bool, db_path: str, ref_lat: float | None,
          ref_lon: float | None, receiver: str, port: int | None, webhook: str | None):
    """Track aircraft from a capture file or live RTL-SDR.

    \b
    Examples:
      adsb track data/capture.txt                    # From file
      adsb track --live --port 8080                  # Live with dashboard
      adsb track --live --ref-lat 35.18 --ref-lon -83.43  # Live with position
    """
    if not file and not live:
        raise click.UsageError("Provide a FILE or use --live for RTL-SDR capture")

    # Apply config defaults if flags not explicitly set
    from .config import load_config
    cfg = load_config()
    if ref_lat is None and cfg["receiver"]["lat"] is not None:
        ref_lat = cfg["receiver"]["lat"]
    if ref_lon is None and cfg["receiver"]["lon"] is not None:
        ref_lon = cfg["receiver"]["lon"]
    if receiver == "default" and cfg["receiver"]["name"] != "default":
        receiver = cfg["receiver"]["name"]
    if webhook is None and cfg.get("webhook"):
        webhook = cfg["webhook"]

    import os
    os.makedirs(Path(db_path).parent, exist_ok=True)

    db = Database(db_path, autocommit=False)
    source = "rtl_adsb:live" if live else file
    rid = db.add_receiver(receiver, lat=ref_lat, lon=ref_lon)
    cap_id = db.start_capture(source=source, receiver_id=rid)

    filter_engine = FilterEngine()

    # Webhook notifications
    notifier = None
    if webhook:
        from .notifications import NotificationDispatcher, WebhookConfig
        notifier = NotificationDispatcher([WebhookConfig(url=webhook)])
        console.print(f"[bold green]Webhook[/] → {webhook}")

    tracker = Tracker(
        db=db, receiver_id=rid, capture_id=cap_id,
        ref_lat=ref_lat, ref_lon=ref_lon,
    )

    # Start web dashboard in background thread if requested
    if port:
        import threading
        from .web.app import create_app
        app = create_app(db_path=db_path, tracker=tracker)
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
        last_prune = 0.0
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
                            if notifier:
                                notifier.notify({"icao": event.icao, "event_type": event.event_type,
                                                 "description": event.description, "lat": event.lat,
                                                 "lon": event.lon, "altitude_ft": event.altitude_ft})

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
                    # Proximity checks across all active aircraft
                    prox_events = filter_engine.check_proximity(active)
                    for event in prox_events:
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
                        if notifier:
                            notifier.notify({"icao": event.icao, "event_type": event.event_type,
                                             "description": event.description, "lat": event.lat,
                                             "lon": event.lon, "altitude_ft": event.altitude_ft})

                    last_print = now
                    tracker.prune_stale()
                    db.flush()

                    # Prune + downsample old data every 10 minutes
                    if now - last_prune > 600:
                        # Tier 1: >24h old → keep every 30s
                        ds1 = db.downsample_positions(older_than_hours=24, keep_interval_sec=30)
                        # Tier 2: >7d old → keep every 60s
                        ds2 = db.downsample_positions(older_than_hours=168, keep_interval_sec=60)
                        # Tier 3: >30d old → delete entirely
                        pruned = db.prune_positions(max_age_hours=720)
                        # Prune phantom aircraft (CRC ghosts with no positions)
                        phantoms = db.prune_phantom_aircraft()
                        total_cleaned = ds1 + ds2 + pruned + phantoms
                        if total_cleaned:
                            parts = []
                            if ds1 + ds2: parts.append(f"downsample: {ds1+ds2}")
                            if pruned: parts.append(f"old: {pruned}")
                            if phantoms: parts.append(f"phantoms: {phantoms}")
                            console.print(f"  [dim]Cleaned {total_cleaned} rows ({', '.join(parts)})[/]")
                            db.vacuum()
                        last_prune = now

    except KeyboardInterrupt:
        console.print("\n[yellow]Stopping...[/]")
        if live and isinstance(frame_source, LiveCapture):
            frame_source.stop()

    db.flush()
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

    ac_events = db.get_events(icao=icao)
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


@cli.command()
def setup():
    """Interactive setup wizard — detect dongle, test capture, save config."""
    from .hardware import find_dongles, check_rtl_tools, test_capture
    from .config import load_config, save_config

    console.print("[bold green]adsb-decode setup wizard[/]\n")

    # Step 1: Detect USB dongle
    console.print("[bold]Step 1:[/] Scanning for RTL-SDR dongle...")
    dongles = find_dongles()
    if dongles:
        for d in dongles:
            console.print(f"  [green]✓[/] Found: {d['description']}")
    else:
        console.print("  [yellow]No RTL-SDR dongle detected.[/]")
        console.print("  Make sure your dongle is plugged in.")
        console.print("  Supported: RTL-SDR Blog v3/v4, generic RTL2832U")

    # Step 2: Check drivers and tools
    console.print("\n[bold]Step 2:[/] Checking drivers and tools...")
    tools = check_rtl_tools()

    import sys
    if tools.get("librtlsdr"):
        console.print("  [green]✓[/] librtlsdr installed")
    else:
        if sys.platform == "darwin":
            console.print("  [red]✗[/] librtlsdr not found — install with: [cyan]brew install librtlsdr[/]")
        else:
            console.print("  [red]✗[/] librtlsdr not found — install with: [cyan]sudo apt install librtlsdr-dev[/]")

    if tools.get("pyrtlsdr"):
        console.print("  [green]✓[/] pyrtlsdr Python binding installed")
    else:
        console.print("  [red]✗[/] pyrtlsdr not found — install with: [cyan]pip install 'adsb-decode[rtlsdr]'[/]")

    if tools.get("rtl_test"):
        console.print("  [green]✓[/] rtl_test available")
    if tools.get("rtl_adsb"):
        console.print("  [green]✓[/] rtl_adsb available (fallback capture)")

    # Step 3: Receiver configuration
    console.print("\n[bold]Step 3:[/] Receiver configuration")
    config = load_config()

    name = click.prompt("  Receiver name", default=config["receiver"]["name"])
    lat = click.prompt("  Receiver latitude", default=config["receiver"]["lat"] or "", show_default=False)
    lon = click.prompt("  Receiver longitude", default=config["receiver"]["lon"] or "", show_default=False)

    try:
        lat = float(lat) if lat else None
    except ValueError:
        lat = None
    try:
        lon = float(lon) if lon else None
    except ValueError:
        lon = None

    config["receiver"]["name"] = name
    config["receiver"]["lat"] = lat
    config["receiver"]["lon"] = lon

    # Step 4: Dashboard port
    port = click.prompt("  Dashboard port", default=config["dashboard"]["port"], type=int)
    config["dashboard"]["port"] = port

    # Step 5: Test capture
    if dongles or tools.get("pyrtlsdr") or tools.get("rtl_adsb"):
        if click.confirm("\n  Run a 5-second test capture?", default=True):
            console.print("  Capturing for 5 seconds...")
            result = test_capture(seconds=5)
            if result["success"]:
                fps = result["frames"] / result["duration_sec"]
                console.print(
                    f"  [green]✓[/] Captured {result['frames']} frames "
                    f"({fps:.1f}/sec) via {result['method']}"
                )
            else:
                console.print(f"  [red]✗[/] Capture failed: {result['error']}")

    # Step 6: Save config
    path = save_config(config)
    console.print(f"\n[bold]Config saved:[/] {path}")

    console.print("\n[bold green]Ready![/] Start tracking with:")
    cmd = "adsb track --live"
    if lat and lon:
        cmd += f" --ref-lat {lat} --ref-lon {lon}"
    cmd += f" --port {port}"
    console.print(f"  [cyan]{cmd}[/]")


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
