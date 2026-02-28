"""Tests for capture module — frame reading and IQ loading."""

import struct
import pytest

from src.capture import FrameReader, IQReader, RawFrame, _clean_hex_line


class TestCleanHexLine:
    """Hex line cleaning and validation."""

    def test_plain_hex_28_chars(self):
        assert _clean_hex_line("8D4840D6202CC371C32CE0576098") == "8D4840D6202CC371C32CE0576098"

    def test_plain_hex_14_chars(self):
        assert _clean_hex_line("02E197CE851A31") == "02E197CE851A31"

    def test_dump1090_format(self):
        assert _clean_hex_line("*8D4840D6202CC371C32CE0576098;") == "8D4840D6202CC371C32CE0576098"

    def test_lowercase_hex(self):
        assert _clean_hex_line("8d4840d6202cc371c32ce0576098") == "8D4840D6202CC371C32CE0576098"

    def test_whitespace_stripped(self):
        assert _clean_hex_line("  8D4840D6202CC371C32CE0576098  \n") == "8D4840D6202CC371C32CE0576098"

    def test_comment_lines_skipped(self):
        assert _clean_hex_line("# This is a comment") is None

    def test_empty_lines_skipped(self):
        assert _clean_hex_line("") is None
        assert _clean_hex_line("   ") is None

    def test_invalid_hex_rejected(self):
        assert _clean_hex_line("not_hex_at_all") is None

    def test_wrong_length_rejected(self):
        assert _clean_hex_line("8D4840D6202C") is None  # Too short
        assert _clean_hex_line("8D4840D6202CC371C32CE057609800") is None  # Too long


class TestFrameReader:
    """Read frames from files and iterables."""

    def test_read_from_iterable(self):
        lines = [
            "8D4840D6202CC371C32CE0576098",
            "8D406B902015A678D4D220AA4BDA",
        ]
        reader = FrameReader(lines)
        frames = reader.read_all()
        assert len(frames) == 2
        assert frames[0].hex_str == "8D4840D6202CC371C32CE0576098"
        assert frames[1].hex_str == "8D406B902015A678D4D220AA4BDA"

    def test_read_from_file(self, tmp_path):
        frame_file = tmp_path / "frames.txt"
        frame_file.write_text(
            "8D4840D6202CC371C32CE0576098\n"
            "8D406B902015A678D4D220AA4BDA\n"
        )
        reader = FrameReader(frame_file)
        frames = reader.read_all()
        assert len(frames) == 2

    def test_skips_comments_and_blanks(self):
        lines = [
            "# Header comment",
            "",
            "8D4840D6202CC371C32CE0576098",
            "   ",
            "# Another comment",
            "8D406B902015A678D4D220AA4BDA",
        ]
        reader = FrameReader(lines)
        frames = reader.read_all()
        assert len(frames) == 2

    def test_handles_dump1090_format(self):
        lines = [
            "*8D4840D6202CC371C32CE0576098;",
            "*8D406B902015A678D4D220AA4BDA;",
        ]
        reader = FrameReader(lines)
        frames = reader.read_all()
        assert len(frames) == 2
        assert frames[0].hex_str == "8D4840D6202CC371C32CE0576098"

    def test_frames_have_timestamps(self):
        lines = ["8D4840D6202CC371C32CE0576098"]
        reader = FrameReader(lines)
        frames = reader.read_all()
        assert frames[0].timestamp > 0

    def test_frames_have_source_label(self):
        reader = FrameReader(["8D4840D6202CC371C32CE0576098"], label="test-input")
        frames = reader.read_all()
        assert frames[0].source == "test-input"

    def test_file_not_found(self, tmp_path):
        reader = FrameReader(tmp_path / "nonexistent.txt")
        with pytest.raises(FileNotFoundError):
            reader.read_all()

    def test_iterator_protocol(self):
        lines = ["8D4840D6202CC371C32CE0576098", "8D406B902015A678D4D220AA4BDA"]
        reader = FrameReader(lines)
        count = 0
        for frame in reader:
            assert isinstance(frame, RawFrame)
            count += 1
        assert count == 2

    def test_mixed_valid_and_invalid(self):
        lines = [
            "8D4840D6202CC371C32CE0576098",  # Valid 112-bit
            "garbage_data",                    # Invalid
            "02E197CE851A31",                  # Valid 56-bit
            "short",                           # Invalid
        ]
        reader = FrameReader(lines)
        frames = reader.read_all()
        assert len(frames) == 2


class TestIQReader:
    """Raw IQ sample file reading."""

    def _write_iq_file(self, tmp_path, n_samples):
        """Write a synthetic IQ file with n_samples pairs."""
        path = tmp_path / "test.iq"
        data = bytearray()
        for i in range(n_samples):
            i_val = (128 + int(50 * (i % 7) / 7)) & 0xFF
            q_val = (128 - int(50 * (i % 5) / 5)) & 0xFF
            data.extend([i_val, q_val])
        path.write_bytes(bytes(data))
        return path

    def test_sample_count(self, tmp_path):
        path = self._write_iq_file(tmp_path, 1000)
        reader = IQReader(path)
        assert reader.n_samples == 1000

    def test_duration(self, tmp_path):
        path = self._write_iq_file(tmp_path, 2_000_000)
        reader = IQReader(path, sample_rate=2_000_000)
        assert reader.duration_seconds == pytest.approx(1.0)

    def test_read_all_samples(self, tmp_path):
        path = self._write_iq_file(tmp_path, 100)
        reader = IQReader(path)
        samples = reader.read_samples()
        assert len(samples) == 100
        assert samples.dtype.kind == "c"  # complex

    def test_read_partial_samples(self, tmp_path):
        path = self._write_iq_file(tmp_path, 100)
        reader = IQReader(path)
        samples = reader.read_samples(count=50)
        assert len(samples) == 50

    def test_read_with_offset(self, tmp_path):
        path = self._write_iq_file(tmp_path, 100)
        reader = IQReader(path)
        samples = reader.read_samples(count=50, offset=25)
        assert len(samples) == 50

    def test_read_magnitude(self, tmp_path):
        path = self._write_iq_file(tmp_path, 100)
        reader = IQReader(path)
        mag = reader.read_magnitude()
        assert len(mag) == 100
        assert mag.dtype.name == "float32"
        assert (mag >= 0).all()  # Squared magnitude is non-negative

    def test_file_not_found(self, tmp_path):
        with pytest.raises(FileNotFoundError):
            IQReader(tmp_path / "nonexistent.iq")
