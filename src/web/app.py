"""Flask app factory for adsb-decode web dashboard.

Creates app with:
- REST API routes under /api/ (JSON endpoints for aircraft, positions, events)
- Page routes for map, table, detail, stats views
- CORS headers for local development
- Dark theme (avionics tradition)
"""

from __future__ import annotations

from flask import Flask

from ..database import Database
from .ingest import ingest
from .routes import register_routes


def create_app(db_path: str = "data/adsb.db") -> Flask:
    """Create and configure the Flask application."""
    app = Flask(
        __name__,
        template_folder="templates",
        static_folder="static",
    )

    # Store database in app config for route access
    app.config["DB_PATH"] = db_path

    @app.before_request
    def _open_db():
        from flask import g
        if not hasattr(g, "db"):
            g.db = Database(app.config["DB_PATH"])

    @app.teardown_appcontext
    def _close_db(exc):
        from flask import g
        db = g.pop("db", None)
        if db:
            db.close()

    # CORS for local dev
    @app.after_request
    def _cors(response):
        response.headers["Access-Control-Allow-Origin"] = "*"
        response.headers["Access-Control-Allow-Headers"] = "Content-Type"
        return response

    register_routes(app)
    app.register_blueprint(ingest)
    return app
