"""Click CLI — the main entry point for adsb-decode.

Commands:
  adsb setup              Check RTL-SDR hardware and dependencies
  adsb capture            Capture frames from RTL-SDR dongle
  adsb decode FILE        Decode a capture file and print aircraft table
  adsb track --file FILE  Track aircraft from a capture file
  adsb track --live       Live tracking from RTL-SDR (optional --port for web dashboard)
  adsb stats              Show database statistics
  adsb history ICAO       Show history for a specific aircraft
  adsb export             Export data (--format csv|json|kml|geojson)
"""
