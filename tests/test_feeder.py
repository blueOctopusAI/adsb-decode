"""Tests for feeder agent — unit tests for buffer and batch logic."""

import pytest

from src.feeder import Feeder


class TestFeederInit:
    def test_create_feeder(self):
        f = Feeder(
            server_url="http://localhost:8000",
            receiver_name="test-rx",
            api_key="abc123",
            lat=35.18,
            lon=-83.38,
        )
        assert f.receiver_name == "test-rx"
        assert f.api_key == "abc123"
        assert f.frames_captured == 0
        assert f.frames_sent == 0

    def test_url_trailing_slash_stripped(self):
        f = Feeder(server_url="http://localhost:8000/", receiver_name="test")
        assert f.server_url == "http://localhost:8000"

    def test_buffer_max_size(self):
        f = Feeder(server_url="http://localhost", receiver_name="test")
        for i in range(1500):
            f.buffer.append(f"frame{i}")
        assert len(f.buffer) == 1000

    def test_headers_with_key(self):
        f = Feeder(server_url="http://localhost", receiver_name="test", api_key="secret")
        h = f._headers()
        assert h["Authorization"] == "Bearer secret"

    def test_headers_without_key(self):
        f = Feeder(server_url="http://localhost", receiver_name="test")
        h = f._headers()
        assert "Authorization" not in h

    def test_default_batch_interval(self):
        f = Feeder(server_url="http://localhost", receiver_name="test")
        assert f.batch_interval == 2.0

    def test_custom_batch_interval(self):
        f = Feeder(server_url="http://localhost", receiver_name="test", batch_interval=5.0)
        assert f.batch_interval == 5.0
