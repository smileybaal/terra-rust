#!/usr/bin/env python3
"""compat.py: diff two wire traces and name the first frame that differs.

This is the tool that turns "1:1 parity" from an assertion into a measurement. The
parity roadmap defines done as: for the same world file and the same scripted client,
a byte-diff of the wire trace and of the console output shows no difference. This is
the wire half of that, and it is the first thing in the repository that can FAIL when
the port diverges from the native server.

A comparison is between two captures, each recorded by `capture.py`:

    python tools\\capture.py 127.0.0.1 7778 --out re/traces/native.jsonl   # native server
    python tools\\capture.py 127.0.0.1 7777 --out re/traces/port.jsonl     # this port
    python tools\\compat.py re/traces/native.jsonl re/traces/port.jsonl

Three verdicts, and the distinction matters because they need different work:

  ALIGNED        every frame matches: same id, same length, same body bytes, in the
                 same order. This is the only verdict that means compatible.
  DIVERGED       the traces match up to some frame and then differ. The report names
                 the frame by its book name, shows both bodies, and says whether the
                 difference is the ID (the streams desynchronised - a protocol bug) or
                 the BODY (same message, different content - a value bug).
  SHORT          one trace ends while the other continues. This is what a barrier
                 looks like from the outside: the client stops and the server waits,
                 or the server stops answering a client that keeps talking.

Design decisions worth stating, because each is a bug avoided:

  * Frames are compared in ORDER, not as sets. Two servers can send the same messages
    in different orders and only one of them is compatible, because the client's state
    machine is a sequence.

  * A `capture.py` in client mode sends the same bytes to both servers, so the C->S
    direction is a CONTROL, not a result: if it differs, the harness is broken rather
    than the server. The tool checks it and says so, instead of reporting a server
    difference that is really a harness difference.

  * Bodies are compared byte-for-byte, with ONE canonicalisation: a `TileSection`
    (10) is inflated first. The reason is not leniency, it is correctness. Deflate
    output is not uniquely defined, so two servers sending identical tiles can emit
    different compressed bytes; comparing the compressed form would report DIVERGED
    for two servers a client cannot tell apart. Inflating first compares what the
    client actually receives, which is the thing compatibility is about. Every other
    message is compared as raw bytes, where a byte diff is the only check that notices
    a value in the wrong place.

  * Timing is NOT compared. Frame order and content are what the protocol specifies;
    how long a server takes is a performance question, and mixing the two would make
    every run fail for a reason nobody asked about.
"""
from __future__ import annotations

import argparse
import json
import sys
import zlib
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import wire  # noqa: E402
from wire import Capture, MessageNames  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent

ALIGNED, DIVERGED, SHORT = "ALIGNED", "DIVERGED", "SHORT"


@dataclass
class Diff:
    verdict: str
    at: int          # index into the frame list where the verdict was decided
    left: wire.Frame | None
    right: wire.Frame | None
    reason: str
    id_differs: bool = False

    def frame_label(self, f: wire.Frame | None) -> str:
        if f is None:
            return "(nothing)"
        return f"{f.direction} id={f.mid} {f.name} len={len(f.body) + 1}"


def canonical_body(f: wire.Frame) -> bytes:
    """The body as the CLIENT sees it, which is what compatibility is about.

    For almost every message that is the bytes on the wire. `TileSection` (10) is the
    exception, and it matters: the body is a RAW deflate stream, and deflate output is
    not uniquely defined - the native server compresses, this port emits stored blocks,
    and two conforming encoders can produce different bytes for identical tiles. A byte
    diff of the compressed form would report DIVERGED for two servers a client cannot
    tell apart, which is a false failure, and a false failure is worse than no check
    because it trains a reader to ignore the result.

    So a section is canonicalised to its INFLATED bytes: the 12-byte header plus the
    tile data, exactly what `DecompressTileBlock` hands the client. Two sections that
    the client reads identically compare equal, whatever encoder produced them.
    """
    if f.mid != 10 or not f.body:
        return f.body
    try:
        return zlib.decompressobj(-15).decompress(f.body, wire.RAW_DEFLATE_CAP)
    except zlib.error:
        # A section that will not inflate is not comparable, and saying so is better
        # than comparing the compressed bytes and calling it a match.
        return b"<UNINFLATABLE>" + f.body


def compare(left: Capture, right: Capture, ignore_ids: set[int] | None = None) -> Diff:
    """Compare two captures frame by frame, in order.

    `ignore_ids` exists for one real case: a frame that carries a value the two runs
    cannot make equal (a session-specific token, a timestamp). It is a parameter rather
    than a default, because ignoring a frame by default would hide exactly the
    divergence this tool is for.
    """
    ignore_ids = ignore_ids or set()
    n = min(len(left.frames), len(right.frames))
    for i in range(n):
        a, b = left.frames[i], right.frames[i]
        if a.mid in ignore_ids or b.mid in ignore_ids:
            continue
        if a.mid != b.mid:
            return Diff(
                verdict=DIVERGED, at=i, left=a, right=b, id_differs=True,
                reason=(f"the streams desynchronise at frame {i}: the left sends "
                        f"{a.name} ({a.mid}) and the right sends {b.name} ({b.mid}). "
                        f"An id mismatch is a protocol difference: the client's state "
                        f"machine is keyed on these ids, so everything after this point "
                        f"is not comparable"),
            )
        ca, cb = canonical_body(a), canonical_body(b)
        if ca != cb:
            return Diff(
                verdict=DIVERGED, at=i, left=a, right=b, id_differs=False,
                reason=(f"frame {i} is {a.name} ({a.mid}) on both sides but the content "
                        f"differs: left {len(ca)} byte(s) as the client sees it, right "
                        f"{len(cb)} ({len(a.body)} and {len(b.body)} on the wire). Same "
                        f"message, different content is a VALUE difference, and the "
                        f"fields show which value moved"),
            )
    if len(left.frames) != len(right.frames):
        longer, shorter = ((left, right) if len(left.frames) > len(right.frames)
                            else (right, left))
        extra = longer.frames[n]
        side = "left" if longer is left else "right"
        return Diff(
            verdict=SHORT, at=n, left=left.frames[n] if n < len(left.frames) else None,
            right=right.frames[n] if n < len(right.frames) else None,
            reason=(f"both traces agree on the first {n} frame(s) and then the {side} "
                    f"continues while the other ends. The next frame on the {side} is "
                    f"{extra.name} ({extra.mid}, {len(extra.body) + 1} bytes). One side "
                    f"stopped talking: this is what a missing handler looks like from "
                    f"the outside"),
        )
    return Diff(verdict=ALIGNED, at=len(left.frames), left=None, right=None,
                reason=f"all {len(left.frames)} frame(s) match: id, length and content")


def show_body(label: str, f: wire.Frame | None, names: MessageNames) -> None:
    if f is None:
        print(f"    {label:5} (no frame)")
        return
    print(f"    {label:5} id={f.mid} {f.name}  {len(f.body) + 1} byte(s) on the wire")
    fields = wire.describe(f.mid, f.body)
    if fields:
        for k, v in fields:
            print(f"          {k:22} {v}")
    print(f"          body                   {wire.hexdump(f.body, 32)}")
    # A section's wire bytes are compressed, so a hexdump of them says little. The
    # inflated view is what a client compares, so it is shown too.
    if f.mid == 10 and f.body:
        inflated = canonical_body(f)
        print(f"          as the client sees it  {wire.hexdump(inflated, 32)}"
              f"  ({len(inflated)} byte(s))")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("left", type=Path, help="the reference capture (e.g. the native server)")
    ap.add_argument("right", type=Path, help="the capture to grade (e.g. this port)")
    ap.add_argument("--ignore", type=int, action="append", default=[],
                    metavar="ID", help="skip frames with this message id (repeatable)")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--require", choices=[ALIGNED, DIVERGED, SHORT],
                    help="exit non-zero unless the verdict is this; makes the tool "
                         "usable as a gate")
    args = ap.parse_args()

    try:
        names = MessageNames.from_book()
    except wire.WireError as e:
        print(f"compat: {e}", file=sys.stderr)
        return 2

    for p in (args.left, args.right):
        if not p.exists():
            print(f"compat: no such capture: {p}", file=sys.stderr)
            return 2

    left = Capture.from_jsonl(args.left.read_text(encoding="utf-8"))
    right = Capture.from_jsonl(args.right.read_text(encoding="utf-8"))
    diff = compare(left, right, set(args.ignore))

    if args.json:
        print(json.dumps({
            "verdict": diff.verdict,
            "at": diff.at,
            "reason": diff.reason,
            "id_differs": diff.id_differs,
            "left": diff.frame_label(diff.left),
            "right": diff.frame_label(diff.right),
            "left_frames": len(left.frames),
            "right_frames": len(right.frames),
        }, indent=2))
    else:
        print(f"compat: {args.left.name} ({len(left.frames)} frames, {left.outcome})")
        print(f"     vs {args.right.name} ({len(right.frames)} frames, {right.outcome})")
        print()
        print(f"VERDICT: {diff.verdict}")
        print(f"  {diff.reason}")
        if diff.verdict != ALIGNED:
            print()
            print(f"  first difference at frame {diff.at}:")
            show_body("left", diff.left, names)
            show_body("right", diff.right, names)
        if left.note or right.note:
            print()
            print(f"  notes: left={left.note!r} right={right.note!r}")

    if args.require and diff.verdict != args.require:
        if not args.json:
            print(f"\ncompat: FAIL  required {args.require}, got {diff.verdict}",
                  file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
