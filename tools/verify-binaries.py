"""Verify the checksummed working copy in re/binaries/ against its own .sha256
files AND against the provenance roots recorded in sheets/00-doctrine.tsv.

Three things have to agree, or the book's root claim is unbacked:
  the bytes on disk, the .sha256 file next to them, and the doctrine row.

Run from the repo root:  python tools/verify-binaries.py
"""
import hashlib
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "re", "binaries")
DOCTRINE = os.path.join(ROOT, "sheets", "00-doctrine.tsv")

# doctrine key -> binary file name
TARGETS = {
    "target_sha256": "Terraria.exe",
    "target_server_sha256": "TerrariaServer.exe",
}

# read the doctrine roots
roots = {}
for line in open(DOCTRINE, encoding="utf-8"):
    if line.startswith("#"):
        continue
    parts = line.rstrip("\n").split("\t")
    if len(parts) >= 3 and parts[1] in TARGETS:
        roots[parts[1]] = parts[2]

bad = 0
for key, name in TARGETS.items():
    path = os.path.join(BIN, name)
    if not os.path.exists(path):
        print(f"MISSING  {name} is not in re/binaries/")
        bad += 1
        continue

    on_disk = hashlib.sha256(open(path, "rb").read()).hexdigest()

    side = os.path.join(BIN, name + ".sha256")
    if not os.path.exists(side):
        print(f"MISSING  {name}.sha256")
        bad += 1
        continue
    raw = open(side, "rb").read()
    text = raw.decode("utf-8")

    # the form has to be canonical: one line, 'hash  name', LF, no BOM, no CR
    problems = []
    if raw.startswith(b"\xef\xbb\xbf"):
        problems.append("BOM")
    if b"\r" in raw:
        problems.append("CRLF")
    if not raw.endswith(b"\n") or raw.count(b"\n") != 1:
        problems.append("not exactly one LF-terminated line")
    m = re.fullmatch(r"([0-9a-f]{64})  (\S+)\n", text)
    if not m:
        problems.append("not '<sha256>  <name>'")
    if problems:
        print(f"FORM     {name}.sha256: {', '.join(problems)}")
        bad += 1
        continue
    side_hash, side_name = m.group(1), m.group(2)

    row = roots.get(key)
    # A doctrine row that is ABSENT is not agreement. The docstring says three
    # things must agree; "the row is missing but the sidecar matches" used to
    # print OK and exit 0, so the bound could be dropped silently.
    if row is None:
        ok = False
        why = f"no {key} row in sheets/00-doctrine.tsv"
    elif on_disk != row:
        ok = False
        why = f"doctrine {key} disagrees with the bytes on disk"
    elif side_name != name or on_disk != side_hash:
        ok = False
        why = f"{name}.sha256 disagrees with the bytes on disk"
    else:
        ok = True
        why = ""
    print(
        f"{'OK      ' if ok else 'MISMATCH'} {name:22} {on_disk[:16]}... "
        f"side={side_hash[:16]}... doctrine={key} {row[:16] + '...' if row else 'MISSING'}"
        + (f"  <- {why}" if why else "")
    )
    if not ok:
        bad += 1

print()
print("PROBLEMS:", bad)
sys.exit(1 if bad else 0)
