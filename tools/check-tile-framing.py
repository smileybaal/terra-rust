"""Inflate a REAL tile section exactly as the client does, with raw DEFLATE.

`NetMessage.DecompressTileBlock` (NetMessage.cs:2266) wraps the NetworkStream in
`Ionic.Zlib.DeflateStream` - which is RAW deflate, no zlib header and no checksum -
and reads xStart, yStart, width, height from INSIDE the stream. So:

  * the whole body is one raw-deflate stream (`zlib.decompressobj(-15)`), and
  * the section rectangle is the first 12 decompressed bytes.

This tool checks a recorded section against that, using Python's zlib rather than the
port's own code, so it is independent evidence rather than a round trip.

It also reports the framing the section was WRITTEN with, which is worth knowing: the
port emits stored blocks (legal deflate, byte-identical after inflation, but larger
than the native), and the first byte of a stored block is `01`.
"""
import json
import struct
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))


def sections(path: str, min_body: int = 1):
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        row = json.loads(line)
        if row.get("kind") == "frame" and row["id"] == 10 and row["body_len"] >= min_body:
            yield row


def check(body: bytes) -> tuple[bool, str]:
    """Decompress as the client does. Returns (ok, a sentence)."""
    try:
        out = zlib.decompressobj(-15).decompress(body)
    except zlib.error as e:
        return False, f"raw deflate refused it: {e}"
    if len(out) < 12:
        return False, f"only {len(out)} byte(s) decompressed; the header needs 12"
    x, y, w, h = struct.unpack_from("<iihh", out, 0)
    if w <= 0 or h <= 0:
        return False, f"header says {w} x {h}, which is not a section"
    return True, f"x={x} y={y} w={w} h={h}, {len(out)} byte(s) of tile data"


def main() -> int:
    traces = [p for p in ("re/traces/native.jsonl", "re/traces/port_c.jsonl")
              if Path(p).exists()]
    if not traces:
        print("no traces to check; run capture.py first")
        return 0
    bad = 0
    for name in traces:
        rows = list(sections(name))
        print(f"==== {name}: {len(rows)} section frame(s)")
        if not rows:
            print("   no section frames in this trace (the run never reached them)")
            continue
        for row in rows[:3]:
            body = bytes.fromhex(row["hex"])
            ok, why = check(body)
            mark = "ok  " if ok else "FAIL"
            if not ok:
                bad += 1
            first = body[0] if body else None
            framing = {1: "stored (final block)", 0: "stored (continuation)"}.get(first, "compressed")
            print(f"  {mark} frame {row['seq']:>3}: {len(body):>6} byte(s), framing={framing}")
            print(f"       {why}")
        if len(rows) > 3:
            print(f"  ... and {len(rows) - 3} more")
    return 0 if bad == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
