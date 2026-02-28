"""Capture and file I/O for ADS-B data.

Input modes:
- FrameReader:       Pre-demodulated hex frame strings (one per line, from dump1090 --raw etc.)
- IQReader:          Raw IQ samples from RTL-SDR (.iq files, interleaved uint8 pairs)
- LiveDemodCapture:  Native real-time capture: pyrtlsdr → our demodulator → frames (preferred)
- LiveCapture:       Auto-selects native demod or rtl_adsb fallback
"""

from __future__ import annotations

import re
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable, Iterator

import numpy as np


@dataclass
class RawFrame:
    """A raw Mode S frame before parsing."""

    hex_str: str
    timestamp: float = 0.0
    signal_level: float | None = None
    source: str = ""


# Pattern for valid Mode S hex: 14 chars (56-bit) or 28 chars (112-bit)
_HEX_PATTERN = re.compile(r"^[0-9A-Fa-f]{14}$|^[0-9A-Fa-f]{28}$")

# dump1090 raw format: *<hex>;
_DUMP1090_PATTERN = re.compile(r"^\*([0-9A-Fa-f]{14}|[0-9A-Fa-f]{28});$")


def _clean_hex_line(line: str) -> str | None:
    """Extract a valid Mode S hex string from a line.

    Handles:
    - Plain hex: "8D4840D6202CC371C32CE0576098"
    - dump1090 raw: "*8D4840D6202CC371C32CE0576098;"
    - With leading/trailing whitespace
    """
    line = line.strip()
    if not line or line.startswith("#"):
        return None

    # Try dump1090 format first
    m = _DUMP1090_PATTERN.match(line)
    if m:
        return m.group(1).upper()

    # Try plain hex
    if _HEX_PATTERN.match(line):
        return line.upper()

    return None


class FrameReader:
    """Read pre-demodulated hex frames from a file or iterable.

    Accepts hex strings from tools like rtl_adsb, dump1090 --raw, or
    any source that produces one hex frame per line.
    """

    def __init__(self, source: str | Path | Iterable[str], label: str = ""):
        """Initialize frame reader.

        Args:
            source: File path or iterable of hex strings.
            label: Optional label for the source (used in RawFrame.source).
        """
        self._source = source
        self._label = label or (str(source) if isinstance(source, (str, Path)) else "iterable")

    def __iter__(self) -> Iterator[RawFrame]:
        lines: Iterable[str]
        if isinstance(self._source, (str, Path)):
            path = Path(self._source)
            if not path.exists():
                raise FileNotFoundError(f"Frame file not found: {path}")
            lines = path.read_text().splitlines()
        else:
            lines = self._source

        t0 = time.time()
        for i, line in enumerate(lines):
            hex_str = _clean_hex_line(line)
            if hex_str is None:
                continue
            yield RawFrame(
                hex_str=hex_str,
                timestamp=t0 + i * 0.001,  # Synthetic timestamps, 1ms apart
                source=self._label,
            )

    def read_all(self) -> list[RawFrame]:
        """Read all frames into a list."""
        return list(self)


class IQReader:
    """Read raw IQ samples from a binary file.

    RTL-SDR produces interleaved unsigned 8-bit IQ pairs:
    [I0, Q0, I1, Q1, I2, Q2, ...]

    This reader loads them into numpy arrays for DSP processing
    by the demodulator module.
    """

    def __init__(self, path: str | Path, sample_rate: int = 2_000_000):
        """Initialize IQ reader.

        Args:
            path: Path to raw IQ binary file (.iq or .bin).
            sample_rate: Sample rate in Hz (default 2 MHz for ADS-B).
        """
        self.path = Path(path)
        self.sample_rate = sample_rate

        if not self.path.exists():
            raise FileNotFoundError(f"IQ file not found: {self.path}")

        self._file_size = self.path.stat().st_size
        self.n_samples = self._file_size // 2  # 2 bytes per IQ pair

    @property
    def duration_seconds(self) -> float:
        """Duration of the recording in seconds."""
        return self.n_samples / self.sample_rate

    def read_samples(self, count: int | None = None, offset: int = 0) -> np.ndarray:
        """Read IQ samples as complex64 numpy array.

        Args:
            count: Number of IQ pairs to read. None = read all.
            offset: Sample pair offset to start reading from.

        Returns:
            Complex numpy array where real=I, imag=Q, centered at 0.
        """
        byte_offset = offset * 2

        if count is not None:
            byte_count = count * 2
        else:
            byte_count = self._file_size - byte_offset

        raw = np.fromfile(self.path, dtype=np.uint8, count=byte_count, offset=byte_offset)

        # Reshape to Nx2 (I, Q pairs), center around 0, convert to complex
        iq = raw.reshape(-1, 2).astype(np.float32) - 127.5
        return iq[:, 0] + 1j * iq[:, 1]

    def read_magnitude_chunked(self, chunk_samples: int = 2_000_000):
        """Yield magnitude chunks for streaming demodulation.

        Args:
            chunk_samples: Samples per chunk (default 1 second at 2 MHz).

        Yields:
            (magnitude_array, sample_offset) tuples.
        """
        overlap = 240  # WINDOW_SIZE from demodulator
        offset = 0
        while offset < self.n_samples:
            count = min(chunk_samples, self.n_samples - offset)
            if count < overlap:
                break
            mag = self.read_magnitude(count=count, offset=offset)
            yield mag, offset
            offset += count - overlap

    def read_magnitude(self, count: int | None = None, offset: int = 0) -> np.ndarray:
        """Read IQ samples and return magnitude (envelope).

        Uses squared magnitude (I^2 + Q^2) to avoid sqrt overhead.
        Relative comparisons still work for preamble detection.

        Returns:
            Float32 numpy array of squared magnitudes.
        """
        byte_offset = offset * 2

        if count is not None:
            byte_count = count * 2
        else:
            byte_count = self._file_size - byte_offset

        raw = np.fromfile(self.path, dtype=np.uint8, count=byte_count, offset=byte_offset)
        iq = raw.reshape(-1, 2).astype(np.float32) - 127.5
        return iq[:, 0] ** 2 + iq[:, 1] ** 2


class LiveDemodCapture:
    """Real-time capture using pyrtlsdr + our custom demodulator.

    This is the native path — raw IQ samples from the dongle go through
    our demodulator (preamble detection, PPM bit recovery, CRC check).
    No external tools required, just the dongle.

    Falls back to rtl_adsb subprocess if pyrtlsdr is not installed.
    """

    SAMPLE_RATE = 2_000_000  # 2 MHz for ADS-B
    CENTER_FREQ = 1_090_000_000  # 1090 MHz
    CHUNK_SIZE = 262_144  # ~131ms of data per read

    def __init__(self, device_index: int = 0, gain: float | str = "auto"):
        self._device_index = device_index
        self._gain = gain
        self._sdr = None
        self._running = False

    def start(self) -> None:
        """Open RTL-SDR device and configure for ADS-B reception."""
        try:
            from rtlsdr import RtlSdr
        except ImportError:
            raise ImportError(
                "pyrtlsdr required for native capture. "
                "Install with: pip install pyrtlsdr"
            )

        self._sdr = RtlSdr(self._device_index)
        self._sdr.sample_rate = self.SAMPLE_RATE
        self._sdr.center_freq = self.CENTER_FREQ
        if isinstance(self._gain, str) and self._gain == "auto":
            self._sdr.gain = "auto"
        else:
            self._sdr.gain = float(self._gain)
        self._running = True

    def stop(self) -> None:
        """Close the RTL-SDR device."""
        self._running = False
        if self._sdr:
            self._sdr.close()
            self._sdr = None

    def __iter__(self) -> Iterator[RawFrame]:
        from .demodulator import iq_to_magnitude, demodulate_buffer

        if self._sdr is None:
            self.start()

        while self._running:
            # Read raw bytes from dongle
            raw = self._sdr.read_bytes(self.CHUNK_SIZE * 2)
            raw_np = np.frombuffer(raw, dtype=np.uint8)

            if len(raw_np) < 480:  # Need at least WINDOW_SIZE * 2
                continue

            mag = iq_to_magnitude(raw_np)
            frames = demodulate_buffer(mag, timestamp=time.time())

            for frame in frames:
                yield frame

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, *args):
        self.stop()


class LiveCapture:
    """Real-time frame capture — uses our demodulator if pyrtlsdr is available,
    falls back to rtl_adsb subprocess otherwise.

    Preferred path: pyrtlsdr → our demodulator (no external tools)
    Fallback path: rtl_adsb subprocess (requires rtl-sdr tools installed)
    """

    def __init__(self, device_index: int = 0, gain: str = "auto"):
        self._device_index = device_index
        self._gain = gain
        self._inner = None
        self._use_native = False

    def start(self) -> None:
        """Start capture — try native demod first, fall back to rtl_adsb."""
        try:
            self._inner = LiveDemodCapture(self._device_index, self._gain)
            self._inner.start()
            self._use_native = True
        except ImportError:
            # pyrtlsdr not installed — fall back to rtl_adsb
            self._inner = _RtlAdsbCapture(self._device_index, self._gain)
            self._inner.start()
            self._use_native = False

    def stop(self) -> None:
        """Stop capture."""
        if self._inner:
            self._inner.stop()
            self._inner = None

    @property
    def source_name(self) -> str:
        return "demodulator" if self._use_native else "rtl_adsb"

    def __iter__(self) -> Iterator[RawFrame]:
        if self._inner is None:
            self.start()
        yield from self._inner

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, *args):
        self.stop()


class _RtlAdsbCapture:
    """Fallback: shell out to rtl_adsb for frame capture."""

    def __init__(self, device_index: int = 0, gain: str = "auto"):
        self._device_index = device_index
        self._gain = gain
        self._proc: subprocess.Popen | None = None

    def start(self) -> None:
        cmd = ["rtl_adsb", "-d", str(self._device_index)]
        if self._gain != "auto":
            cmd.extend(["-g", self._gain])
        self._proc = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )

    def stop(self) -> None:
        if self._proc:
            self._proc.kill()
            self._proc.wait()
            self._proc = None

    def __iter__(self) -> Iterator[RawFrame]:
        if self._proc is None:
            self.start()
        assert self._proc and self._proc.stdout
        for line in self._proc.stdout:
            line = line.strip()
            if line.startswith("*") and line.endswith(";"):
                hex_str = line[1:-1].upper()
                if _HEX_PATTERN.match(hex_str):
                    yield RawFrame(
                        hex_str=hex_str,
                        timestamp=time.time(),
                        source="rtl_adsb",
                    )
