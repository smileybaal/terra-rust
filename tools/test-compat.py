"""Prove that compat.py can FAIL, by mutating one byte of a real capture.

A check that cannot fail is not a check (the doctrine this repository is built on).
`compat.py` is not verified by finding agreement - any broken comparator agrees with
itself. It is verified by taking a real trace, changing ONE byte in ONE frame, and
asserting that the comparator reports exactly that frame and no other.

Three mutations, because there are three ways to be incompatible and the tool claims
to distinguish them:

  1. a BODY byte flipped   -> DIVERGED, same id, body differs
  2. a frame REMOVED       -> DIVERGED, ids desynchronise from that point
  3. a frame APPENDED      -> SHORT, one side keeps talking

Each case also asserts the reported frame INDEX, because "it found a difference
somewhere" is not the same as "it found the first difference and named it".
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import compat  # noqa: E402
from wire import Capture  # noqa: E402

TRACE = Path("re/traces/port_a.jsonl")

if not TRACE.exists():
    print(f"SKIP: {TRACE} does not exist; run capture.py first")
    sys.exit(0)

rows = [json.loads(line) for line in TRACE.read_text(encoding="utf-8").splitlines() if line.strip()]
head = rows[0]
frames = rows[1:]

failures = []


def check(label: str, mutated: list[dict], want_verdict: str, want_at: int):
    text = "\n".join(json.dumps(r) for r in [head] + mutated) + "\n"
    right = Capture.from_jsonl(text)
    left = Capture.from_jsonl(TRACE.read_text(encoding="utf-8"))
    diff = compat.compare(left, right)
    ok = diff.verdict == want_verdict and diff.at == want_at
    status = "ok  " if ok else "FAIL"
    print(f"  {status} {label}")
    print(f"        got {diff.verdict} at frame {diff.at}, wanted {want_verdict} at {want_at}")
    if not ok:
        failures.append(label)


# Pick a frame with a real body and a mid > 0 so the mutation is meaningful.
target = next(i for i, r in enumerate(frames) if r["body_len"] > 4)
print(f"mutating frame {target} ({frames[target]['name']} id={frames[target]['id']}) "
      f"of a {len(frames)}-frame trace")

# 1. One body byte flipped.
m1 = [dict(r) for r in frames]
hexs = list(m1[target]["hex"])
hexs[0] = "f" if hexs[0] != "f" else "0"
m1[target]["hex"] = "".join(hexs)
check("a flipped body byte is DIVERGED at that frame", m1, compat.DIVERGED, target)

# 2. A frame removed from the middle.
m2 = [dict(r) for r in frames[:target]] + [dict(r) for r in frames[target + 1:]]
check("a removed frame desynchronises the stream there", m2, compat.DIVERGED, target)

# 3. A frame appended to the end.
m3 = [dict(r) for r in frames] + [dict(frames[-1])]
check("an extra frame at the end is SHORT", m3, compat.SHORT, len(frames))

# 4. The control: an unmutated copy must be ALIGNED, or nothing above means anything.
check("an unmutated copy is ALIGNED (the control)",
      [dict(r) for r in frames], compat.ALIGNED, len(frames))

if failures:
    print(f"\nFAILED: {len(failures)} case(s): {', '.join(failures)}")
    sys.exit(1)
print("\nPASSED: compat.py reports DIVERGED, SHORT and ALIGNED, each at the right frame")
