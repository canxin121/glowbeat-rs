#!/usr/bin/env python3
"""Independent golden-vector validation for the GLB1 UDP layout."""
from __future__ import annotations

import binascii
import pathlib
import struct

ROOT = pathlib.Path(__file__).resolve().parents[1]
PROTOCOL = ROOT / "src" / "protocol.rs"

MAGIC = b"GLB1"
VERSION = 1
HEADER_BYTES = 40
LIGHT_BYTES = 12
CRC_BYTES = 4
MAX_UDP_PAYLOAD = 1200
MAX_LIGHTS = (MAX_UDP_PAYLOAD - HEADER_BYTES - CRC_BYTES) // LIGHT_BYTES


def q(value: float) -> int:
    return round(max(0.0, min(1.0, value)) * 65535.0)


def main() -> None:
    source = PROTOCOL.read_text(encoding="utf-8")
    for needle in [
        'pub const MAGIC: [u8; 4] = *b"GLB1";',
        "pub const VERSION: u8 = 1;",
        "pub const HEADER_BYTES: usize = 40;",
        "pub const LIGHT_BYTES: usize = 12;",
        "pub const CRC_BYTES: usize = 4;",
    ]:
        assert needle in source, f"missing source invariant: {needle}"
    assert MAX_LIGHTS == 96

    flags = 0b1011
    header = struct.pack(
        "<4sBBHIQQHHHHHH",
        MAGIC,
        VERSION,
        flags,
        HEADER_BYTES,
        0x11223344,
        0x0102030405060708,
        987654321,
        12750,
        round(0.25 * 65535),
        0,
        1,
        2,
        0,
    )
    assert len(header) == HEADER_BYTES
    entries = b"".join(
        [
            struct.pack("<IHHHH", 7, q(0.1), q(0.2), q(0.3), q(0.4)),
            struct.pack("<IHHHH", 8, q(1.0), q(0.0), q(0.7), q(1.0)),
        ]
    )
    payload = header + entries
    checksum = binascii.crc32(payload) & 0xFFFFFFFF
    packet = payload + struct.pack("<I", checksum)
    assert len(packet) == HEADER_BYTES + 2 * LIGHT_BYTES + CRC_BYTES
    assert struct.unpack_from("<I", packet, len(packet) - 4)[0] == checksum
    damaged = bytearray(packet)
    damaged[10] ^= 0x80
    assert (binascii.crc32(damaged[:-4]) & 0xFFFFFFFF) != checksum

    print(
        f"protocol validation passed: header={HEADER_BYTES}, light={LIGHT_BYTES}, "
        f"max_lights={MAX_LIGHTS}, golden_crc=0x{checksum:08x}"
    )


if __name__ == "__main__":
    main()
