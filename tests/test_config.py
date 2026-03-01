"""Tests for config file management."""

import os

from src.config import load_config, save_config, CONFIG_FILE


class TestConfig:
    def test_default_config(self, tmp_path, monkeypatch):
        monkeypatch.setattr("src.config.CONFIG_DIR", tmp_path)
        monkeypatch.setattr("src.config.CONFIG_FILE", tmp_path / "config.yaml")
        cfg = load_config()
        assert cfg["receiver"]["name"] == "default"
        assert cfg["receiver"]["lat"] is None
        assert cfg["dashboard"]["port"] == 8080

    def test_save_and_load(self, tmp_path, monkeypatch):
        monkeypatch.setattr("src.config.CONFIG_DIR", tmp_path)
        monkeypatch.setattr("src.config.CONFIG_FILE", tmp_path / "config.yaml")

        cfg = load_config()
        cfg["receiver"]["name"] = "Franklin NC"
        cfg["receiver"]["lat"] = 35.18
        cfg["receiver"]["lon"] = -83.38
        cfg["dashboard"]["port"] = 9090
        cfg["webhook"] = "https://hooks.example.com/adsb"

        path = save_config(cfg)
        assert path.exists()

        # Reload
        loaded = load_config()
        assert loaded["receiver"]["name"] == "Franklin NC"
        assert loaded["receiver"]["lat"] == 35.18
        assert loaded["receiver"]["lon"] == -83.38
        assert loaded["dashboard"]["port"] == 9090
        assert loaded["webhook"] == "https://hooks.example.com/adsb"

    def test_null_values_roundtrip(self, tmp_path, monkeypatch):
        monkeypatch.setattr("src.config.CONFIG_DIR", tmp_path)
        monkeypatch.setattr("src.config.CONFIG_FILE", tmp_path / "config.yaml")

        cfg = load_config()
        cfg["receiver"]["lat"] = None
        save_config(cfg)

        loaded = load_config()
        assert loaded["receiver"]["lat"] is None

    def test_bool_values_roundtrip(self, tmp_path, monkeypatch):
        monkeypatch.setattr("src.config.CONFIG_DIR", tmp_path)
        monkeypatch.setattr("src.config.CONFIG_FILE", tmp_path / "config.yaml")

        cfg = load_config()
        cfg["receiver"]["active"] = True
        save_config(cfg)

        loaded = load_config()
        assert loaded["receiver"]["active"] is True
