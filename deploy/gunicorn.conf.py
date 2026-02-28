# Gunicorn config for adsb-decode
bind = "127.0.0.1:8000"
workers = 1  # SQLite is single-writer, 1 worker avoids lock contention
timeout = 30
accesslog = "-"
errorlog = "-"
loglevel = "info"
