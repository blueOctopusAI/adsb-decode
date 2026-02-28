"""CRC-24 validation for Mode S messages.

ICAO standard polynomial: x²⁴ + x²³ + x²¹ + x²⁰ + x¹⁷ + x¹⁵ + x¹³ + x¹² + x¹⁰ + x⁸ + x⁵ + x⁴ + x³ + 1
Generator: 0xFFF409

For DF17 (ADS-B): last 24 bits are pure CRC. Valid frames → remainder 0x000000.
For DF11 (all-call): last 24 bits are CRC XORed with ICAO address.
"""
