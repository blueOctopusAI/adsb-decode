"""ICAO address resolution — country lookup, military detection, N-number decode.

Every aircraft has a unique 24-bit ICAO address assigned by country of registration.
Address ranges are allocated in blocks (e.g., 0xA00000-0xAFFFFF = United States).

Features:
- Country lookup from ICAO address
- Military address block detection (reserved ranges per country)
- US N-number algorithm: civil addresses 0xA00001-0xADF7C7 -> tail number (e.g., N12345)
- Callsign pattern matching for military flights (RCH*, DUKE*, REACH*, etc.)
"""

from __future__ import annotations

# ICAO address allocation blocks: (start, end, country)
# Source: ICAO Doc 9861 — comprehensive list of major allocations
_COUNTRY_BLOCKS: list[tuple[int, int, str]] = [
    (0x004000, 0x0043FF, "Zimbabwe"),
    (0x006000, 0x006FFF, "Mozambique"),
    (0x008000, 0x00FFFF, "South Africa"),
    (0x010000, 0x017FFF, "Egypt"),
    (0x018000, 0x01FFFF, "Libya"),
    (0x020000, 0x027FFF, "Morocco"),
    (0x028000, 0x02FFFF, "Tunisia"),
    (0x030000, 0x0303FF, "Botswana"),
    (0x032000, 0x032FFF, "Burundi"),
    (0x034000, 0x034FFF, "Cameroon"),
    (0x038000, 0x038FFF, "Congo"),
    (0x03E000, 0x03EFFF, "Ivory Coast"),
    (0x040000, 0x040FFF, "DR Congo"),
    (0x042000, 0x042FFF, "Ethiopia"),
    (0x044000, 0x044FFF, "Equatorial Guinea"),
    (0x046000, 0x046FFF, "Gabon"),
    (0x048000, 0x048FFF, "Ghana"),
    (0x04A000, 0x04AFFF, "Guinea"),
    (0x04C000, 0x04CFFF, "Kenya"),
    (0x050000, 0x050FFF, "Liberia"),
    (0x054000, 0x054FFF, "Madagascar"),
    (0x058000, 0x058FFF, "Malawi"),
    (0x05A000, 0x05AFFF, "Mali"),
    (0x05C000, 0x05CFFF, "Mauritania"),
    (0x060000, 0x060FFF, "Niger"),
    (0x062000, 0x062FFF, "Nigeria"),
    (0x064000, 0x064FFF, "Uganda"),
    (0x068000, 0x068FFF, "Senegal"),
    (0x06A000, 0x06AFFF, "Sierra Leone"),
    (0x06C000, 0x06CFFF, "Somalia"),
    (0x070000, 0x070FFF, "Sudan"),
    (0x074000, 0x074FFF, "Tanzania"),
    (0x078000, 0x078FFF, "Chad"),
    (0x07C000, 0x07CFFF, "Zambia"),
    (0x080000, 0x080FFF, "Comoros"),
    (0x084000, 0x084FFF, "Djibouti"),
    (0x088000, 0x088FFF, "Eritrea"),
    (0x08A000, 0x08AFFF, "Gambia"),
    (0x08C000, 0x08CFFF, "Burkina Faso"),
    (0x098000, 0x098FFF, "Lesotho"),
    (0x09A000, 0x09AFFF, "Namibia"),
    (0x0A0000, 0x0A7FFF, "Algeria"),
    (0x0C0000, 0x0C4FFF, "Angola"),
    (0x0C8000, 0x0C8FFF, "Rwanda"),
    (0x0CA000, 0x0CAFFF, "Togo"),
    (0x0CC000, 0x0CCFFF, "Benin"),
    (0x0D0000, 0x0D7FFF, "Bahamas"),
    (0x0D8000, 0x0DFFFF, "Barbados"),
    (0x0E0000, 0x0E3FFF, "Belize"),
    (0x0E4000, 0x0E7FFF, "Colombia"),
    (0x0E8000, 0x0EBFFF, "Costa Rica"),
    (0x0EC000, 0x0EFFFF, "Cuba"),
    (0x0F0000, 0x0F3FFF, "El Salvador"),
    (0x0F4000, 0x0F7FFF, "Guatemala"),
    (0x0F8000, 0x0FBFFF, "Guyana"),
    (0x0FC000, 0x0FFFFF, "Haiti"),
    (0x100000, 0x103FFF, "Honduras"),
    (0x108000, 0x10BFFF, "Jamaica"),
    (0x110000, 0x113FFF, "Nicaragua"),
    (0x114000, 0x117FFF, "Panama"),
    (0x118000, 0x11BFFF, "Dominican Republic"),
    (0x11C000, 0x11FFFF, "Trinidad and Tobago"),
    (0x120000, 0x123FFF, "Suriname"),
    (0x140000, 0x143FFF, "Antigua and Barbuda"),
    (0x200000, 0x27FFFF, "Unassigned"),
    (0x300000, 0x33FFFF, "Italy"),
    (0x340000, 0x37FFFF, "Spain"),
    (0x380000, 0x3BFFFF, "France"),
    (0x3C0000, 0x3FFFFF, "Germany"),
    (0x400000, 0x43FFFF, "United Kingdom"),
    (0x440000, 0x447FFF, "Austria"),
    (0x448000, 0x44FFFF, "Belgium"),
    (0x450000, 0x457FFF, "Bulgaria"),
    (0x458000, 0x45FFFF, "Denmark"),
    (0x460000, 0x467FFF, "Finland"),
    (0x468000, 0x46FFFF, "Greece"),
    (0x470000, 0x477FFF, "Hungary"),
    (0x478000, 0x47FFFF, "Norway"),
    (0x480000, 0x487FFF, "Netherlands"),
    (0x488000, 0x48FFFF, "Poland"),
    (0x490000, 0x497FFF, "Portugal"),
    (0x498000, 0x49FFFF, "Czech Republic"),
    (0x4A0000, 0x4A7FFF, "Romania"),
    (0x4A8000, 0x4AFFFF, "Sweden"),
    (0x4B0000, 0x4B7FFF, "Switzerland"),
    (0x4B8000, 0x4BFFFF, "Turkey"),
    (0x4C0000, 0x4C7FFF, "Yugoslavia/Serbia"),
    (0x4CA000, 0x4CAFFF, "Cyprus"),
    (0x4CC000, 0x4CCFFF, "Ireland"),
    (0x4D0000, 0x4D03FF, "Iceland"),
    (0x500000, 0x5003FF, "Sri Lanka"),
    (0x501000, 0x5013FF, "Malaysia"),
    (0x508000, 0x50FFFF, "Indonesia"),
    (0x510000, 0x5107FF, "Iraq"),
    (0x600000, 0x6003FF, "Singapore"),
    (0x680000, 0x6803FF, "Thailand"),
    (0x681000, 0x6813FF, "Vietnam"),
    (0x700000, 0x700FFF, "Afghanistan"),
    (0x710000, 0x717FFF, "Pakistan"),
    (0x718000, 0x71FFFF, "Bangladesh"),
    (0x720000, 0x727FFF, "Myanmar"),
    (0x730000, 0x737FFF, "Kuwait"),
    (0x738000, 0x73FFFF, "Laos"),
    (0x740000, 0x747FFF, "Nepal"),
    (0x748000, 0x74FFFF, "Oman"),
    (0x750000, 0x757FFF, "Saudi Arabia"),
    (0x758000, 0x75FFFF, "South Korea"),
    (0x760000, 0x767FFF, "North Korea"),
    (0x768000, 0x76FFFF, "Syria"),
    (0x770000, 0x777FFF, "Taiwan"),
    (0x778000, 0x77FFFF, "Jordan"),
    (0x780000, 0x7BFFFF, "China"),
    (0x7C0000, 0x7FFFFF, "Australia"),
    (0x800000, 0x83FFFF, "India"),
    (0x840000, 0x87FFFF, "Japan"),
    (0x880000, 0x887FFF, "Thailand"),
    (0x890000, 0x890FFF, "Vietnam"),
    (0x894000, 0x894FFF, "Hong Kong"),
    (0x895000, 0x8953FF, "Macau"),
    (0x896000, 0x896FFF, "Cambodia"),
    (0x897000, 0x8973FF, "Philippines"),
    (0x898000, 0x898FFF, "Mongolia"),
    (0x899000, 0x8993FF, "Maldives"),
    (0x8A0000, 0x8A7FFF, "UAE"),
    (0x900000, 0x9003FF, "Israel"),
    (0xA00000, 0xAFFFFF, "United States"),
    (0xC00000, 0xC3FFFF, "Canada"),
    (0xC80000, 0xC87FFF, "New Zealand"),
    (0xC88000, 0xC88FFF, "Fiji"),
    (0xE00000, 0xE3FFFF, "Argentina"),
    (0xE40000, 0xE7FFFF, "Brazil"),
    (0xE80000, 0xE83FFF, "Chile"),
    (0xE84000, 0xE87FFF, "Ecuador"),
    (0xE88000, 0xE8BFFF, "Paraguay"),
    (0xE8C000, 0xE8FFFF, "Peru"),
    (0xE90000, 0xE93FFF, "Uruguay"),
    (0xE94000, 0xE97FFF, "Venezuela"),
    (0xF00000, 0xF07FFF, "ICAO (special)"),
    (0xF09000, 0xF093FF, "ICAO (special)"),
]

# US military ICAO block
_US_MILITARY_START = 0xADF7C8
_US_MILITARY_END = 0xAFFFFF

# US civil N-number range
_US_CIVIL_START = 0xA00001
_US_CIVIL_END = 0xADF7C7

# Characters for N-number suffix (after digits)
_NNUM_CHARS = "ABCDEFGHJKLMNPQRSTUVWXYZ"  # No I or O

# Military callsign prefixes
_MILITARY_CALLSIGNS = frozenset({
    "RCH",    # Reach (USAF tanker/transport)
    "DUKE",   # US Army
    "DOOM",   # USAF fighter
    "JAKE",   # USN
    "TOPCAT", # USMC
    "REACH",  # USAF
    "EVAC",   # Aeromedical
    "TEAL",   # US Special Ops
    "SPAR",   # Air Force special air mission
    "SAM",    # Special Air Mission
    "EXEC",   # Executive transport
    "CRZR",   # USAF Cruiser
    "MOOSE",  # Canadian military
    "CANAF",  # Canadian Forces
    "ASCOT",  # Royal Air Force
    "RAFR",   # RAF Reserve
    "GAF",    # German Air Force
    "URAN",   # Russian military
    "CNV",    # French Navy
    "FAF",    # French Air Force
    "IAM",    # Italian Air Force
    "SUI",    # Swiss Air Force
})


def lookup_country(icao_hex: str) -> str | None:
    """Look up country of registration from ICAO address.

    Returns country name or None if address is in an unallocated range.
    """
    addr = int(icao_hex, 16)
    for start, end, country in _COUNTRY_BLOCKS:
        if start <= addr <= end:
            return country
    return None


def is_military(icao_hex: str, callsign: str | None = None) -> bool:
    """Check if an aircraft is military.

    Two detection methods:
    1. ICAO address in a military allocation block (currently US military)
    2. Callsign matches known military patterns
    """
    addr = int(icao_hex, 16)

    # US military address block
    if _US_MILITARY_START <= addr <= _US_MILITARY_END:
        return True

    # Military callsign check
    if callsign:
        cs = callsign.strip().upper()
        for prefix in _MILITARY_CALLSIGNS:
            if cs.startswith(prefix):
                return True

    return False


def _letter_suffix(remainder: int, max_letters: int) -> str | None:
    """Decode 1 or 2 letter suffix from remainder.

    After all digit positions at a given level, remaining addresses encode
    letter suffixes: 24 single letters (A-Z minus I,O), optionally followed
    by a second letter (25 options: bare + 24 letters).

    Args:
        remainder: Offset into the letter suffix zone.
        max_letters: 1 for single letter only, 2 for optional second letter.

    Returns suffix string or None if out of range.
    """
    if max_letters == 1:
        if remainder < len(_NNUM_CHARS):
            return _NNUM_CHARS[remainder]
        return None

    # 2-letter zone: 24 first letters * 25 options each (bare + 24 second letters)
    first_idx = remainder // 25
    if first_idx >= len(_NNUM_CHARS):
        return None
    second_rem = remainder % 25
    if second_rem == 0:
        return _NNUM_CHARS[first_idx]
    second_idx = second_rem - 1
    if second_idx < len(_NNUM_CHARS):
        return _NNUM_CHARS[first_idx] + _NNUM_CHARS[second_idx]
    return None


def icao_to_n_number(icao_hex: str) -> str | None:
    """Convert US civil ICAO address to N-number (tail number).

    US civil aircraft addresses (0xA00001-0xADF7C7) encode the registration
    using a base conversion: the offset from 0xA00001 maps to N-numbers
    from N1 to N99999 with optional 1-2 letter suffix.

    Block sizes per digit level:
    - d1 block = 101711 (1 bare + 10*10111 digit blocks + 24*25 letter suffixes)
    - d2 block = 10111  (1 bare + 10*951 digit blocks + 24*25 letter suffixes)
    - d3 block = 951    (1 bare + 10*35 digit blocks + 24*25 letter suffixes)
    - d4 block = 35     (1 bare + 10 digits + 24 single letters)

    Returns N-number string (e.g., "N12345") or None if not a US civil address.
    """
    addr = int(icao_hex, 16)
    if not (_US_CIVIL_START <= addr <= _US_CIVIL_END):
        return None

    offset = addr - _US_CIVIL_START

    d1 = offset // 101711
    if d1 > 8:
        return None
    remainder = offset % 101711
    prefix = f"N{d1 + 1}"

    if remainder == 0:
        return prefix

    remainder -= 1

    # After d1: 10 * 10111 digit addresses, then 600 letter suffix addresses
    if remainder < 10 * 10111:
        d2 = remainder // 10111
        remainder = remainder % 10111
        prefix = f"{prefix}{d2}"

        if remainder == 0:
            return prefix

        remainder -= 1

        # After d2: 10 * 951 digit addresses, then 600 letter suffix addresses
        if remainder < 10 * 951:
            d3 = remainder // 951
            remainder = remainder % 951
            prefix = f"{prefix}{d3}"

            if remainder == 0:
                return prefix

            remainder -= 1

            # After d3: 10 * 35 digit addresses, then 600 letter suffix addresses
            if remainder < 10 * 35:
                d4 = remainder // 35
                remainder = remainder % 35
                prefix = f"{prefix}{d4}"

                if remainder == 0:
                    return prefix

                remainder -= 1

                # After d4: 10 digits then 24 single letters
                if remainder < 10:
                    return f"{prefix}{remainder}"
                remainder -= 10
                suffix = _letter_suffix(remainder, max_letters=1)
                if suffix:
                    return f"{prefix}{suffix}"
            else:
                # Letter suffix after 3 digits (e.g., N123A, N123AB)
                remainder -= 10 * 35
                suffix = _letter_suffix(remainder, max_letters=2)
                if suffix:
                    return f"{prefix}{suffix}"
        else:
            # Letter suffix after 2 digits (e.g., N12A, N12AB)
            remainder -= 10 * 951
            suffix = _letter_suffix(remainder, max_letters=2)
            if suffix:
                return f"{prefix}{suffix}"
    else:
        # Letter suffix after 1 digit (e.g., N1A, N1AB)
        remainder -= 10 * 10111
        suffix = _letter_suffix(remainder, max_letters=2)
        if suffix:
            return f"{prefix}{suffix}"

    return None
