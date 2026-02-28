"""REST API and page routes for the web dashboard.

API endpoints (JSON):
  GET /api/aircraft          List all tracked aircraft (with optional filters)
  GET /api/aircraft/<icao>   Single aircraft detail + recent positions
  GET /api/positions         Recent positions (for map updates, 2s polling)
  GET /api/events            Recent events (military, emergency, anomaly)
  GET /api/stats             Database statistics

Page routes (HTML):
  GET /                      Map view (Leaflet.js)
  GET /table                 Aircraft table with sort/filter
  GET /aircraft/<icao>       Single aircraft detail + history
  GET /stats                 Statistics dashboard
"""
