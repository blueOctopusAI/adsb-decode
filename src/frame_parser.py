"""Parse raw bitstreams into structured Mode S frames.

Responsibilities:
- Classify Downlink Format (DF) from first 5 bits
- Extract ICAO address (bits 8-31 for DF17, or from CRC remainder for DF11)
- Package into ModeFrame dataclass with DF, ICAO, raw bytes, timestamp, signal level
- Reject frames that fail CRC validation
"""
