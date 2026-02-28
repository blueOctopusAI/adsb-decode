"""Flask app factory for adsb-decode web dashboard.

Creates app with:
- REST API routes under /api/ (JSON endpoints for aircraft, positions, events)
- Page routes for map, table, detail, stats views
- CORS headers for local development
- Dark theme (avionics tradition)
"""
