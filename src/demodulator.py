"""IQ sample demodulation — convert raw radio samples to ADS-B bitstreams.

Pipeline:
1. IQ → magnitude: sqrt(I² + Q²) per sample pair (or squared magnitude for speed)
2. Preamble detection: slide window looking for 8µs pulse pattern at positions 0,1,3.5,4.5µs
3. Bit recovery: PPM — compare first half-µs energy to second half-µs per bit period

At 2 MHz sample rate: 1 bit = 2 samples, preamble = 16 samples, short msg = 112 samples, long msg = 224 samples.
"""
