#!/usr/bin/env python3
"""wire.py: the Terraria wire protocol, read out of the sheet book rather than typed.

Every compatibility tool in this directory speaks the same wire, so the wire lives
here once. Three things are shared:

  * `MessageNames` - id -> name, read from `sheets/re/server/fields.tsv`, the same
    rows the Rust port projects. A name is never typed here, so a tool and the
    server cannot disagree about what message 4 is called. This rule earned itself:
    `mitm.py` used to label body 4 as a connection `State` from a hand-written
    table, and the book says 4 is `SyncPlayer` (connection state is a separate
    client-side machine, `Netplay.Connection.State`, values 0-6).

  * `read_frame` / `write_frame` - the framing. A 2-byte little-endian length that
    EXCLUDES itself, then the message id byte, then the body (`MessageBuffer`).
    Reads are bounded: a frame length is a u16, so a claimed length cannot be
    trusted to size an allocation.

  * `Reader` - a cursor with .NET's primitives. `BinaryWriter.Write(string)` is a
    7-bit-length-prefixed UTF-8 string, and getting that wrong is not a typo, it is
    a desynchronised stream, so it is implemented in one place.

The decoder table below is deliberately PARTIAL and says so. Each entry cites the C#
that reads the field. A message with no entry returns no fields, and the caller
prints hex: guessing at a body layout would manufacture the exact evidence this
tooling exists to avoid.
"""
from __future__ import annotations

import json
import struct
import zlib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
FIELDS = ROOT / "sheets" / "re" / "server" / "fields.tsv"

# How much of an undecoded body to show before eliding.
HEX_LIMIT = 48

# The most a tile section can inflate to. A section is 200x150 tiles and each tile is
# at most a handful of bytes, so 1 MiB is generous; a stream claiming more is not a
# section and is refused rather than allocated. This bounds a hostile body.
RAW_DEFLATE_CAP = 1 << 20


class WireError(Exception):
    """A stream that cannot be a Terraria frame stream. Never silently tolerated."""


# ---------------------------------------------------------------------------
# message names, from the book
# ---------------------------------------------------------------------------


class MessageNames:
    """id -> name, and id -> wire ordinal, read from the evidence book.

    `terraria.id.messageid.*` rows carry the enum member and its value. The reader
    is tolerant of a missing book (it prints bare numbers rather than inventing
    names) but not of a corrupt one, because a tool that quietly mislabels a message
    is worse than one that refuses.
    """

    def __init__(self, rows: dict[int, str], duplicates: dict[int, list[str]]):
        self.by_id = rows
        self.duplicates = duplicates

    def __len__(self) -> int:
        return len(self.by_id)

    def name(self, mid: int) -> str:
        return self.by_id.get(mid, f"MessageID {mid}")

    def is_known(self, mid: int) -> bool:
        return mid in self.by_id

    @classmethod
    def from_book(cls, path: Path = FIELDS) -> "MessageNames":
        by_id: dict[int, str] = {}
        dup: dict[int, list[str]] = {}
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as e:
            raise WireError(f"cannot read the book at {path}: {e}") from e
        for line in text.splitlines():
            if not line.startswith("terraria.id.messageid."):
                continue
            cells = line.split("\t")
            # columns: id, type, name, field_type, kind, modifiers, value
            if len(cells) < 7 or not cells[6].lstrip("-").isdigit():
                continue  # a row with no value (`-`) is a book gap, not a name
            value = int(cells[6])
            name = cells[2]
            if value in by_id and by_id[value] != name:
                dup.setdefault(value, [by_id[value]]).append(name)
            else:
                by_id[value] = name
        return cls(by_id, dup)


# ---------------------------------------------------------------------------
# framing
# ---------------------------------------------------------------------------


def write_frame(mid: int, body: bytes = b"") -> bytes:
    payload = bytes([mid]) + body
    if len(payload) > 0xFFFF:
        raise WireError(f"message {mid} is {len(payload)} bytes; a frame length is a u16")
    return struct.pack("<H", len(payload)) + payload


@dataclass
class Frame:
    """One framed message, with its position in the stream."""

    seq: int
    direction: str  # "C->S" or "S->C"
    mid: int
    name: str
    body: bytes
    t: float
    # Filled in by the recorders, not by the framing.
    fields: list[tuple[str, str]] = field(default_factory=list)

    def as_row(self) -> dict[str, Any]:
        # The BODY is stored, as hex, not only a hash. A trace that kept only a sha1
        # could say "these two runs differ" and nothing more, which is useless: the
        # whole job of `compat.py` is to show WHICH field moved. Hex doubles the size
        # of a trace and is worth it, because a diff nobody can read is not evidence.
        return {
            "seq": self.seq,
            "dir": self.direction,
            "id": self.mid,
            "name": self.name,
            "len": len(self.body) + 1,
            "body_len": len(self.body),
            "t": round(self.t, 6),
            "sha1": __import__("hashlib").sha1(self.body).hexdigest(),
            "hex": self.body.hex() if self.body else "",
        }


def read_frame(buf: bytearray, direction: str = "?", seq: int = 0, t: float = 0.0,
               names: MessageNames | None = None) -> Frame | None:
    """Pop one frame off the front of `buf`, or None if it is not complete yet.

    A frame is complete once `length` bytes have arrived after the 2-byte header.
    `length` cannot be 0: the C# always writes at least the id byte, so a zero-length
    frame is a desynchronised stream and is refused rather than skipped.
    """
    if len(buf) < 2:
        return None
    (length,) = struct.unpack_from("<H", buf, 0)
    if length == 0:
        raise WireError(f"{direction}: a frame claims length 0; the stream is not framing")
    if len(buf) < 2 + length:
        return None
    frame = bytes(buf[2:2 + length])
    del buf[:2 + length]
    mid = frame[0]
    name = names.name(mid) if names else f"MessageID {mid}"
    return Frame(seq=seq, direction=direction, mid=mid, name=name, body=frame[1:], t=t)


# ---------------------------------------------------------------------------
# .NET primitives
# ---------------------------------------------------------------------------


class Reader:
    """A cursor over one message body, with the primitives `BinaryReader` gives the C#.

    Every accessor returns None rather than raising when the body is short: a body
    that ends early is data about the sender, and a decoder should report it, not
    die on it.
    """

    def __init__(self, data: bytes):
        self.data = data
        self.off = 0

    def eof(self) -> bool:
        return self.off >= len(self.data)

    def remaining(self) -> int:
        return len(self.data) - self.off

    def byte(self) -> int | None:
        if self.eof():
            return None
        b = self.data[self.off]
        self.off += 1
        return b

    def boolean(self) -> bool | None:
        b = self.byte()
        return None if b is None else bool(b)

    def u16(self) -> int | None:
        if self.remaining() < 2:
            return None
        (v,) = struct.unpack_from("<H", self.data, self.off)
        self.off += 2
        return v

    def i16(self) -> int | None:
        if self.remaining() < 2:
            return None
        (v,) = struct.unpack_from("<h", self.data, self.off)
        self.off += 2
        return v

    def i32(self) -> int | None:
        if self.remaining() < 4:
            return None
        (v,) = struct.unpack_from("<i", self.data, self.off)
        self.off += 4
        return v

    def f32(self) -> float | None:
        if self.remaining() < 4:
            return None
        (v,) = struct.unpack_from("<f", self.data, self.off)
        self.off += 4
        return v

    def string(self) -> str | None:
        """`BinaryWriter.Write(string)`: 7-bit length, then UTF-8."""
        n = 0
        shift = 0
        while True:
            b = self.byte()
            if b is None:
                return None
            n |= (b & 0x7F) << shift
            if not b & 0x80:
                break
            shift += 7
            if shift > 28:
                return None  # hostile or corrupt length prefix
        raw = self.data[self.off:self.off + n]
        if len(raw) < n:
            return None
        self.off += n
        return raw.decode("utf-8", "replace")


def write_string(buf: bytearray, s: str | bytes) -> None:
    data = s.encode() if isinstance(s, str) else s
    n = len(data)
    while True:
        byte = n & 0x7F
        n >>= 7
        if n:
            byte |= 0x80
        buf.append(byte)
        if not n:
            break
    buf.extend(data)


def write_string_bytes(s: str | bytes) -> bytes:
    b = bytearray()
    write_string(b, s)
    return bytes(b)


def hexdump(body: bytes, limit: int = HEX_LIMIT) -> str:
    if not body:
        return "(empty)"
    shown = body[:limit]
    text = " ".join(f"{b:02x}" for b in shown)
    if len(body) > limit:
        text += f" ... (+{len(body) - limit} more)"
    return text


# ---------------------------------------------------------------------------
# body decoders
# ---------------------------------------------------------------------------

NETWORK_TEXT_MODE = {0: "literal", 1: "formattable", 2: "localization key"}


def _kick(r: Reader) -> list[tuple[str, str]]:
    """Message 2, `Kick`: a NetworkText (mode, text, substitution count)."""
    out: list[tuple[str, str]] = []
    mode = r.byte()
    if mode is not None:
        out.append(("mode", f"{mode} ({NETWORK_TEXT_MODE.get(mode, '?')})"))
    s = r.string()
    if s is not None:
        out.append(("text", repr(s)))
    count = r.byte()
    if count is not None and not r.eof():
        out.append(("substitutions", str(count)))
    return out


def _tile_section(r: Reader, body: bytes) -> list[tuple[str, str]]:
    """Message 10, `TileSection` (`NetMessage.DecompressTileBlock`, :2266).

    The WHOLE body is one RAW deflate stream (Ionic's `DeflateStream` is raw: no zlib
    header, no checksum), and the section rectangle is the first 12 decompressed bytes.
    Reading x/y/w/h out of the compressed bytes - which this decoder did first, and
    which the port's own doc comment already said was wrong - produced
    `start (-369093375, 973055)`. The framing is decoded here as the client decodes it,
    so a section that a client could not read cannot look fine in a trace.

    Inflating uses `zlib.decompressobj(-15)`, and the length is capped: a section is
    20,000 tiles at most, so a stream that claims to inflate to gigabytes is a lie and
    is refused rather than allocated.
    """
    out: list[tuple[str, str]] = []
    try:
        raw = zlib.decompressobj(-15).decompress(body, RAW_DEFLATE_CAP)
    except zlib.error as e:
        out.append(("deflate", f"REFUSED: {e}"))
        return out
    if len(raw) < 12:
        out.append(("deflate", f"only {len(raw)} byte(s) inflated; the header needs 12"))
        return out
    x, y, w, h = struct.unpack_from("<iihh", raw, 0)
    out.append(("start", f"({x}, {y})"))
    out.append(("size", f"{w} x {h}"))
    framing = {1: "stored (final)", 0: "stored (continues)"}.get(body[0], "compressed")
    out.append(("framing", f"{framing}; {len(body)} byte(s) on the wire"))
    out.append(("tile bytes", str(len(raw) - 12)))
    return out


WORLD_DATA_BITS = {0: "dayTime", 1: "bloodMoon", 2: "eclipse"}


def _world_data(r: Reader, body: bytes) -> list[tuple[str, str]]:
    """Message 7, `WorldData` (`NetMessage.cs:233-302`), in the order the WIRE uses.

    Only the first eight fields are named, and each name comes from
    `kernel/worlddata.rs::write_world_data`, which is where the whole body is written
    and where the field order is pinned by tests. The rest of the body is counted, not
    labelled: a wrong label on the nineteenth field would be worse than no label, and
    the body is 165 bytes of world state whose real check is the byte diff in
    `compat.py`, not a pretty-print here.
    """
    out: list[tuple[str, str]] = []
    time = r.i32()
    if time is not None:
        out.append(("time", str(time)))
    bits = r.byte()
    if bits is not None:
        set_bits = [WORLD_DATA_BITS[i] for i in range(3) if bits & (1 << i)]
        out.append(("bits", f"0x{bits:02x} " + (", ".join(set_bits) if set_bits else "(none)")))
    moon = r.byte()
    if moon is not None:
        out.append(("moonPhase", str(moon)))
    mx = r.i16()
    my = r.i16()
    if mx is not None and my is not None:
        out.append(("world size", f"{mx} x {my}"))
    sx = r.i16()
    sy = r.i16()
    if sx is not None and sy is not None:
        out.append(("spawn tile", f"({sx}, {sy})"))
    surf = r.i16()
    rock = r.i16()
    if surf is not None and rock is not None:
        out.append(("surface / rock", f"{surf} / {rock}"))
    out.append(("tail", f"{r.remaining()} byte(s) of world state (id, name, GUID, "
                        f"backgrounds, wind, tree/cave styles, rain, bosses)"))
    return out


def _status_text_size(r: Reader) -> list[tuple[str, str]]:
    """Message 9, `StatusTextSize`: an int32 max, a NetworkText, then progress bytes.

    The max is an int32, NOT a byte: `kernel/worlddata.rs::write_status_text_size`
    pushes `status_max` with `push_i32`, and reading it as a byte desynchronised the
    rest of the body - the tool then reported `text mode 0 (literal)` and an empty key
    for a frame that really carries mode 2 and `LegacyInterface.44`. That bug was found
    by this decoder disagreeing with the server, which is the point of decoding at all.

    The key is a LOCALIZATION KEY, not a sentence: the client renders it in its own
    language (`Lang.inter[44]` is `LegacyInterface.44`, "Receiving tile data").
    """
    out: list[tuple[str, str]] = []
    mx = r.i32()
    if mx is not None:
        out.append(("max", str(mx)))
    mode = r.byte()
    if mode is not None:
        out.append(("text mode", f"{mode} ({NETWORK_TEXT_MODE.get(mode, '?')})"))
    key = r.string()
    if key is not None:
        out.append(("key", repr(key)))
    subs = r.byte()
    if subs is not None:
        out.append(("substitutions", str(subs)))
    out.append(("progress bytes", str(r.remaining())))
    return out


def _spawn_tile_data(r: Reader) -> list[tuple[str, str]]:
    """Message 8, `SpawnTileData`: where the client would rather appear."""
    x = r.i32()
    y = r.i32()
    style = r.byte()
    out: list[tuple[str, str]] = []
    if x is not None and y is not None:
        out.append(("requested tile", f"({x}, {y})"))
    if style is not None:
        out.append(("team/style (raw)", str(style)))
    return out


def _player_info(r: Reader) -> list[tuple[str, str]]:
    """Message 3, `PlayerInfo`: the slot, then the server-special-flags bool."""
    out: list[tuple[str, str]] = []
    slot = r.byte()
    flag = r.boolean()
    if slot is not None:
        out.append(("slot", str(slot)))
    if flag is not None:
        out.append(("serverSpecialFlags[2]", str(flag)))
    return out


def _sync_player(r: Reader) -> list[tuple[str, str]]:
    """Message 4, `SyncPlayer`: the slot on the wire, then the player's look and name.

    The slot is written by the client and DISCARDED by the server (`MessageBuffer.cs`
    sets `num188 = whoAmI`), which is why it is labelled as untrusted here.
    """
    out: list[tuple[str, str]] = []
    slot = r.byte()
    if slot is not None:
        out.append(("slot (untrusted)", str(slot)))
    variant = r.byte()
    hair = r.byte()
    if variant is not None:
        out.append(("skinVariant", str(variant)))
    if hair is not None:
        out.append(("hair", str(hair)))
    r.f32()
    r.byte()
    name = r.string()
    if name is not None:
        out.append(("name", repr(name)))
    return out


def _player_spawn(r: Reader) -> list[tuple[str, str]]:
    """Message 12, `PlayerSpawn`: the slot, the spawn tile, then the player's state."""
    out: list[tuple[str, str]] = []
    slot = r.byte()
    if slot is not None:
        out.append(("slot", str(slot)))
    x = r.i16()
    y = r.i16()
    if x is not None and y is not None:
        out.append(("spawn tile", f"({x}, {y})"))
    return out


def _player_life_mana(r: Reader) -> list[tuple[str, str]]:
    """Message 16, `PlayerLifeMana`: `statLife` then `statLifeMax` (both int16)."""
    out: list[tuple[str, str]] = []
    life = r.i16()
    mx = r.i16()
    if life is not None:
        out.append(("statLife", str(life)))
    if mx is not None:
        out.append(("statLifeMax", str(mx)))
    return out


def _hello(r: Reader) -> list[tuple[str, str]]:
    """Message 1, `Hello`: JUST the greeting string, `"Terraria" + <release>`.

    There is no NetworkText mode byte here. Putting one in is answered with the
    version-mismatch kick, which is what `replay-client.py` did first.
    """
    out: list[tuple[str, str]] = []
    s = r.string()
    if s is not None:
        out.append(("greeting", repr(s)))
    return out


# id -> decoder. A message absent from this table has no decoder, and the caller
# prints hex rather than guessing. That is the honest default, not a gap to hide.
DECODERS: dict[int, Any] = {
    1: _hello,
    2: _kick,
    3: _player_info,
    4: _sync_player,
    7: _world_data,
    8: _spawn_tile_data,
    9: _status_text_size,
    10: _tile_section,
    12: _player_spawn,
    16: _player_life_mana,
}


def describe(mid: int, body: bytes) -> list[tuple[str, str]]:
    """Decode a body with the decoder this message has, or return [] to mean "no".

    An empty list is a statement, not a failure: it says the body has no decoder in
    this table, and the caller then prints it as hex.

    Two decoder shapes are supported - `f(reader)` and `f(reader, body)` - because
    `TileSection` needs the whole body to count the DEFLATE stream it carries. The
    arity is inspected rather than guessed, so adding a decoder that wants the body
    does not need a second registration table.
    """
    fn = DECODERS.get(mid)
    if fn is None:
        return []
    r = Reader(body)
    try:
        out = fn(r, body) if fn in _BODY_AWARE else fn(r)
    except (struct.error, IndexError, UnicodeDecodeError):
        return []
    if out and not r.eof() and mid != 10:
        out.append(("_trailing", f"{r.remaining()} undecoded byte(s)"))
    return out


# The decoders that need the whole body as well as the cursor. `TileSection` reports
# the size of the DEFLATE stream it carries, which is what is left of the body rather
# than a field with a width; `WorldData` reports how many bytes of world state follow
# the fields it names.
_BODY_AWARE = {_tile_section, _world_data}


# ---------------------------------------------------------------------------
# captures
# ---------------------------------------------------------------------------


@dataclass
class Capture:
    """A recorded stream: the frames in order, plus how the run ended."""

    frames: list[Frame]
    source: str = ""
    outcome: str = "unknown"
    note: str = ""

    def of(self, mid: int) -> list[Frame]:
        return [f for f in self.frames if f.mid == mid]

    def ids(self, direction: str | None = None) -> list[int]:
        return [f.mid for f in self.frames if direction is None or f.direction == direction]

    def to_jsonl(self) -> str:
        lines = [
            json.dumps({
                "kind": "capture",
                "source": self.source,
                "outcome": self.outcome,
                "note": self.note,
                "frames": len(self.frames),
            })
        ]
        lines += [json.dumps({"kind": "frame", **f.as_row()}) for f in self.frames]
        return "\n".join(lines) + "\n"

    @classmethod
    def from_jsonl(cls, text: str) -> "Capture":
        frames: list[Frame] = []
        head: dict[str, Any] = {}
        for line in text.splitlines():
            if not line.strip():
                continue
            row = json.loads(line)
            if row.get("kind") == "capture":
                head = row
                continue
            body = bytes.fromhex(row["hex"]) if row.get("hex") else b""
            frames.append(Frame(
                seq=row["seq"], direction=row["dir"], mid=row["id"], name=row["name"],
                body=body, t=row.get("t", 0.0),
            ))
        return cls(
            frames=frames,
            source=head.get("source", ""),
            outcome=head.get("outcome", "unknown"),
            note=head.get("note", ""),
        )

    def save(self, path: Path) -> None:
        path.write_text(self.to_jsonl(), encoding="utf-8")
