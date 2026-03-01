"""Configuration file management for adsb-decode.

Reads/writes ~/.adsb-decode/config.yaml with receiver settings,
database path, dashboard port, and webhook URL.
"""

from __future__ import annotations

import os
from pathlib import Path

CONFIG_DIR = Path.home() / ".adsb-decode"
CONFIG_FILE = CONFIG_DIR / "config.yaml"


def _parse_value(val: str):
    """Parse a YAML-like value string into a Python type."""
    if val == "null" or val == "~" or val == "":
        return None
    if val.lower() == "true":
        return True
    if val.lower() == "false":
        return False
    try:
        return int(val)
    except ValueError:
        pass
    try:
        return float(val)
    except ValueError:
        pass
    # Strip quotes
    if (val.startswith('"') and val.endswith('"')) or \
       (val.startswith("'") and val.endswith("'")):
        return val[1:-1]
    return val


def _default_config() -> dict:
    return {
        "receiver": {
            "name": "default",
            "lat": None,
            "lon": None,
        },
        "database": {
            "path": "data/adsb.db",
        },
        "dashboard": {
            "host": "127.0.0.1",
            "port": 8080,
        },
        "webhook": None,
    }


def load_config() -> dict:
    """Load config from ~/.adsb-decode/config.yaml.

    Returns default config if file doesn't exist.
    Uses simple key=value parsing to avoid PyYAML dependency.
    """
    config = _default_config()
    if not CONFIG_FILE.exists():
        return config

    try:
        text = CONFIG_FILE.read_text()
        # Simple YAML-like parser for our flat config
        current_section = None
        for line in text.splitlines():
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                continue

            # Indented line = belongs to current section
            is_indented = line.startswith("  ") or line.startswith("\t")

            if not is_indented and ":" in stripped:
                key, _, val = stripped.partition(":")
                key = key.strip()
                val = val.strip()

                if not val:
                    # Section header (e.g., "receiver:")
                    current_section = key
                    if current_section not in config:
                        config[current_section] = {}
                    continue
                else:
                    # Top-level key with value
                    current_section = None
                    config[key] = _parse_value(val)
                    continue

            if is_indented and ":" in stripped:
                key, _, val = stripped.partition(":")
                key = key.strip()
                val = val.strip()
                parsed = _parse_value(val)

                if current_section and isinstance(config.get(current_section), dict):
                    config[current_section][key] = parsed
                else:
                    config[key] = parsed
    except Exception:
        pass  # Fall back to defaults

    return config


def save_config(config: dict) -> Path:
    """Save config to ~/.adsb-decode/config.yaml.

    Returns the path to the config file.
    """
    CONFIG_DIR.mkdir(parents=True, exist_ok=True)

    lines = ["# adsb-decode configuration", ""]

    for section, values in config.items():
        if isinstance(values, dict):
            lines.append(f"{section}:")
            for key, val in values.items():
                if val is None:
                    lines.append(f"  {key}: null")
                elif isinstance(val, bool):
                    lines.append(f"  {key}: {'true' if val else 'false'}")
                elif isinstance(val, str):
                    lines.append(f"  {key}: \"{val}\"")
                else:
                    lines.append(f"  {key}: {val}")
            lines.append("")
        else:
            if values is None:
                lines.append(f"{section}: null")
            elif isinstance(values, str):
                lines.append(f"{section}: \"{values}\"")
            else:
                lines.append(f"{section}: {values}")

    CONFIG_FILE.write_text("\n".join(lines) + "\n")
    return CONFIG_FILE
