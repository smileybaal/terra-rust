"""Check the kernel's stored-block DEFLATE framing against zlib, which is not our code.

The vectors here are the ones pinned in `tiles::tests::the_stored_blocks_are_framed_correctly`.
If zlib can read them, the client's `System.IO.Compression.DeflateStream` can too: both
implement RFC 1951 and neither knows anything about the other.

Run: python tools/check-deflate-framing.py
"""
import zlib

cases = [
    # (name, compressed, expected plaintext)
    ("empty", bytes([0x01, 0x00, 0x00, 0xFF, 0xFF]), b""),
    ("three bytes", bytes([0x01, 0x03, 0x00, 0xFC, 0xFF, 1, 2, 3]), bytes([1, 2, 3])),
]

# The over-65535 case: a non-final block of 65535 nines, then a final one of a single 9.
big = bytes([9]) * 65536
first = bytes([0x00, 0xFF, 0xFF, 0x00, 0x00]) + bytes([9]) * 65535
second = bytes([0x01, 0x01, 0x00, 0xFE, 0xFF]) + bytes([9])
cases.append(("65536 bytes", first + second, big))

# And a body shaped like a tile section: a 12-byte rectangle header, a few tile bytes,
# and the three short list counts, all of which the encoder emits in one stored block.
body = (
    (0).to_bytes(4, "little") + (0).to_bytes(4, "little")
    + (200).to_bytes(2, "little", signed=True) + (150).to_bytes(2, "little", signed=True)
    + bytes([0x42, 0x01, 0x00, 0x00])
    + (0).to_bytes(2, "little") + (0).to_bytes(2, "little") + (0).to_bytes(2, "little")
)
stored = bytes([0x01]) + len(body).to_bytes(2, "little") + ((~len(body)) & 0xFFFF).to_bytes(2, "little") + body
cases.append(("a section-shaped body", stored, body))

ok = True
for name, compressed, expected in cases:
    try:
        got = zlib.decompress(compressed, -15)
    except Exception as exc:  # noqa: BLE001 - the point is to report whatever it says
        print("FAIL %-24s zlib refused the stream: %s" % (name, exc))
        ok = False
        continue
    if got != expected:
        print("FAIL %-24s round trip differs (%d vs %d bytes)" % (name, len(got), len(expected)))
        ok = False
    else:
        print("ok   %-24s %d bytes in, %d bytes out" % (name, len(compressed), len(got)))

print("OK   zlib reads every framing the encoder produces" if ok else "FAILED")
raise SystemExit(0 if ok else 1)
