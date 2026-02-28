"""WSGI entry point for production deployment."""

import os
import sys

# Add project root to path
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from src.web.app import create_app

db_path = os.environ.get("ADSB_DB_PATH", "/opt/adsb-decode/data/adsb.db")
app = create_app(db_path=db_path)

# Configure ingest API key if set
ingest_key = os.environ.get("ADSB_INGEST_KEY", "")
if ingest_key:
    app.config["INGEST_API_KEY"] = ingest_key
