"""Capture and file I/O for ADS-B data.

Three input modes:
- IQReader: Raw IQ samples from RTL-SDR (.iq files, interleaved uint8 pairs)
- FrameReader: Pre-demodulated hex frame strings (one per line, from rtl_adsb/dump1090 --raw)
- LiveCapture: Real-time capture from RTL-SDR dongle via pyrtlsdr
"""
