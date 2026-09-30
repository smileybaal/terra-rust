"""Read gameMode out of a captured WorldData frame, using the port's own field order.

`kernel/worlddata.rs::write_world_data` is the authority on the order, and it is pinned
by tests, so this reads the same bytes the port writes rather than re-deriving the .wld
header (which needs the version guards and is already done, byte-exactly, in
`kernel/worldfile.rs`).

The point: the native server boots a client whose claimed difficulty disagrees with the
world's mode (`MessageBuffer.cs:385-389`), so a capture script needs the world's mode.
Getting it from the port's own WorldData is one source, not two.
"""
import json
import struct
import sys
from pathlib import Path

trace = sys.argv[1] if len(sys.argv) > 1 else "re/traces/port_d.jsonl"

for line in Path(trace).read_text(encoding="utf-8").splitlines():
    row = json.loads(line)
    if row.get("kind") != "frame" or row["id"] != 7:
        continue
    b = bytes.fromhex(row["hex"])
    off = 0
    (time,) = struct.unpack_from("<i", b, off); off += 4
    bits = b[off]; off += 1
    moon = b[off]; off += 1
    (mx, my, sx, sy, surf, rock) = struct.unpack_from("<hhhhhh", b, off); off += 12
    (world_id,) = struct.unpack_from("<i", b, off); off += 4
    n = b[off]; off += 1
    name = b[off:off + n].decode("utf-8", "replace"); off += n
    game_mode = b[off]
    print(f"trace     {trace}")
    print(f"time      {time}   bits 0x{bits:02x}   moon {moon}")
    print(f"size      {mx} x {my}   spawn ({sx}, {sy})   surface {surf}   rock {rock}")
    print(f"worldId   {world_id}   name {name!r}")
    mode = {0: "Classic", 1: "Expert", 2: "Master", 3: "Journey"}.get(game_mode, "?")
    print(f"gameMode  {game_mode} ({mode})")
    print()
    print(f"-> a joining client must send difficulty "
          f"{'3' if game_mode == 3 else '0'}, or the native server boots it")
    break
else:
    print(f"no WorldData (7) frame in {trace}")
