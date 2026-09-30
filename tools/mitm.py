#!/usr/bin/env python3
"""mitm.py: a logging TCP proxy for the Terraria wire protocol.

Point the vanilla client at this proxy and it relays every byte to the real server
while printing each framed message in both directions. It exists because the
handshake is the only part of the port that is written, so the interesting
question - "what does the client actually send next, and when does the server stop
answering?" - is answered by watching the wire rather than by reading C#.

    python tools\\mitm.py --listen 127.0.0.1:1739 --target 127.0.0.1:1738

Framing, as `MessageBuffer` reads it: a 2-byte little-endian length that EXCLUDES
itself, then the message id byte, then the body. Strings are `BinaryWriter`'s
7-bit-length-prefixed UTF-8.

Message names are not typed here. They are read from the evidence book
(`sheets/re/server/fields.tsv`, the `terraria.id.messageid.*` rows), because that
table is the same one the Rust port projects, so the proxy and the server cannot
disagree about what message 4 is called. A body the proxy does not know is printed
as hex rather than guessed at.

Note on the server browser: Terraria finds servers by a UDP broadcast to
255.255.255.255:8888 once a second (`Netplay.cs:826-874`), which this proxy does
not relay. That is fine for a client you add by IP, which is the usual case when
you already know the port. Relaying the beacon too would need the client to trust a
port number the proxy then has to lie about.
"""
from __future__ import annotations

import argparse
import collections
import socket
import struct
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIELDS = ROOT / "sheets" / "re" / "server" / "fields.tsv"

# How much of an unknown body to show before eliding.
HEX_LIMIT = 48


def load_message_names() -> dict[int, str]:
    """id -> name, from the sheet rows for the MessageID enum.

    Read at runtime so a row added to the book is picked up without editing this
    file. A missing or unreadable book is not fatal: the proxy still logs, it just
    prints the bare number, which is the honest thing to do rather than inventing
    names.
    """
    names: dict[int, str] = {}
    try:
        for line in FIELDS.read_text(encoding="utf-8").splitlines():
            if not line.startswith("terraria.id.messageid."):
                continue
            cells = line.split("\t")
            # columns: id, type, name, field_type, kind, modifiers, value
            if len(cells) < 7 or not cells[6].isdigit():
                continue
            value = int(cells[6])
            # Rows that fold to the same id (dec019) share a value only by accident;
            # the first name wins and the collision is reported by --verbose if it
            # ever matters.
            names.setdefault(value, cells[2])
    except OSError as e:
        print(f"warning: cannot read {FIELDS}: {e}", file=sys.stderr)
    return names


class Reader:
    """A cursor over one message body, with the C#'s primitives."""

    def __init__(self, data: bytes):
        self.data = data
        self.off = 0

    def eof(self) -> bool:
        return self.off >= len(self.data)

    def byte(self) -> int | None:
        if self.eof():
            return None
        b = self.data[self.off]
        self.off += 1
        return b

    def boolean(self) -> int | None:
        return self.byte()

    def string(self) -> str | None:
        """A 7-bit-length-prefixed UTF-8 string, as BinaryWriter writes it."""
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
            if shift > 28:  # a hostile or corrupt length prefix
                return None
        raw = self.data[self.off:self.off + n]
        if len(raw) < n:
            return None
        self.off += n
        return raw.decode("utf-8", "replace")


def describe(mid: int, body: bytes, names: dict[int, str]) -> list[tuple[str, str]]:
    """Decode the bodies this tool knows, honestly declining the ones it does not.

    Each returned pair is (label, value). An empty list means "no decoder", and the
    caller then prints hex.
    """
    r = Reader(body)
    out: list[tuple[str, str]] = []
    if mid == 1:  # Hello: the greeting string
        s = r.string()
        if s is not None:
            out.append(("greeting", repr(s)))
    elif mid == 2:  # Kick: a NetworkText (mode, text, substitution count)
        mode = r.byte()
        if mode is not None:
            label = {0: "literal", 1: "formattable", 2: "localization key"}.get(mode, "?")
            out.append(("mode", f"{mode} ({label})"))
        s = r.string()
        if s is not None:
            out.append(("text", repr(s)))
        count = r.byte()
        if count is not None and not r.eof():
            out.append(("substitutions", str(count)))
    elif mid == 3:  # PlayerInfo: slot, then a bool the client reads as a flag
        slot = r.byte()
        flag = r.boolean()
        if slot is not None:
            out.append(("slot", str(slot)))
        if flag is not None:
            out.append(("serverSpecialFlags[2]", str(bool(flag))))
    elif mid == 4:  # SyncPlayer: MessageBuffer.cs:248 passes the client's own slot
        v = r.byte()
        if v is not None:
            out.append(("slot", str(v)))
    if out and not r.eof():
        out.append(("_trailing", f"{len(body) - r.off} undecoded byte(s)"))
    return out


def hexdump(body: bytes) -> str:
    if not body:
        return "(empty)"
    shown = body[:HEX_LIMIT]
    text = " ".join(f"{b:02x}" for b in shown)
    if len(body) > HEX_LIMIT:
        text += f" ... (+{len(body) - HEX_LIMIT} more)"
    return text


class Tally:
    """Per-direction packet counts by id, printed when the proxy stops."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.connections = 0
        self.counts: dict[str, collections.Counter] = {
            "C->S": collections.Counter(),
            "S->C": collections.Counter(),
        }

    def add(self, direction: str, name: str) -> None:
        with self.lock:
            self.counts[direction][name] += 1

    def note_connection(self) -> None:
        with self.lock:
            self.connections += 1

    def summary(self) -> str:
        """A one-line liveness summary, so an idle proxy is distinguishable
        from a hung one. Silence here means nobody is connected, which is the
        normal state while waiting for a client."""
        with self.lock:
            c2s = sum(self.counts["C->S"].values())
            s2c = sum(self.counts["S->C"].values())
            return f"connections={self.connections}  C->S={c2s}  S->C={s2c}"

    def report(self) -> str:
        lines = []
        with self.lock:
            for direction in ("C->S", "S->C"):
                counter = self.counts[direction]
                total = sum(counter.values())
                if not total:
                    continue
                lines.append(f"  {direction}  {total} packet(s)")
                for name, n in counter.most_common():
                    lines.append(f"      {n:5d}  {name}")
        return "\n".join(lines)


def log(tally: Tally, direction: str, body: bytes, names: dict[int, str], quiet: bool):
    """Print one frame. `body` is the id byte plus the message body."""
    mid = body[0]
    name = names.get(mid, f"MessageID {mid}")
    tally.add(direction, name)
    if quiet:
        return
    stamp = time.strftime("%H:%M:%S") + f".{int(time.time() * 1000) % 1000:03d}"
    payload = body[1:]
    print(f"{stamp}  {direction}  len={len(body):<5d} id={mid:<3d} {name}", flush=True)
    fields = describe(mid, payload, names)
    if fields:
        for label, value in fields:
            print(f"{'':17} {label:22} {value}", flush=True)
    else:
        print(f"{'':17} {'body':22} {hexdump(payload)}", flush=True)


def pump(src: socket.socket, dst: socket.socket, direction: str, names, tally, quiet):
    """Copy src -> dst, splitting the stream into frames as it goes.

    The 2-byte length excludes itself, so a frame is complete once `have` bytes
    have arrived after it. Anything longer than a u16 cannot exist on this wire,
    and a stream that stops mid-frame is reported rather than silently dropped.
    """
    buf = bytearray()
    try:
        while True:
            chunk = src.recv(65536)
            if not chunk:
                break
            dst.sendall(chunk)
            buf += chunk
            while len(buf) >= 2:
                (length,) = struct.unpack("<H", buf[:2])
                if length == 0:
                    print(f"  !! {direction}: a frame claims length 0; dropping the connection", flush=True)
                    return
                if len(buf) < 2 + length:
                    break
                frame = bytes(buf[2:2 + length])
                del buf[:2 + length]
                log(tally, direction, frame, names, quiet)
    except OSError as e:
        print(f"  {direction}: {e}", flush=True)
    finally:
        for s in (src, dst):
            try:
                s.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass


def heartbeat(tally: Tally, interval: float, stop: threading.Event) -> None:
    """Print a liveness line every `interval` seconds until asked to stop.

    A proxy waiting for a client is silent by definition, which is
    indistinguishable from a hung process to anything watching it. This says
    "alive, and here is the running tally" on a timer instead.
    """
    while not stop.wait(interval):
        print(f"mitm: alive   {tally.summary()}", flush=True)


def parse_addr(text: str) -> tuple[str, int]:
    host, _, port = text.rpartition(":")
    if not host or not port.isdigit():
        raise argparse.ArgumentTypeError(f"expected host:port, got {text!r}")
    return host.strip("[]"), int(port)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--listen", type=parse_addr, default=("127.0.0.1", 1739), nargs="?",
                    help="host:port to accept on (default 127.0.0.1:1739)")
    ap.add_argument("--target", type=parse_addr, default=("127.0.0.1", 1738),
                    help="host:port of the real server (default 127.0.0.1:1738)")
    ap.add_argument("--quiet", action="store_true", help="count packets but print none")
    ap.add_argument("--heartbeat", type=float, default=30.0, metavar="SECONDS",
                    help="print a liveness line this often; 0 disables (default 30)")
    args = ap.parse_args()

    names = load_message_names()
    if not names and not args.quiet:
        print("warning: the book gave no message names; logging bare ids", file=sys.stderr)

    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(args.listen)
    listener.listen(16)
    tally = Tally()
    print(f"mitm: {args.listen[0]}:{args.listen[1]} -> {args.target[0]}:{args.target[1]}"
          f"   ({len(names)} message names from the book)", flush=True)

    stop = threading.Event()
    if args.heartbeat > 0:
        threading.Thread(target=heartbeat, args=(tally, args.heartbeat, stop), daemon=True).start()

    try:
        while True:
            client, addr = listener.accept()
            tally.note_connection()
            print(f"\n--- {addr[0]}:{addr[1]} connected ---", flush=True)
            try:
                upstream = socket.create_connection(args.target, timeout=10)
            except OSError as e:
                print(f"  cannot reach {args.target}: {e}", flush=True)
                client.close()
                continue
            client.settimeout(None)
            up = threading.Thread(target=pump, args=(client, upstream, "C->S", names, tally, args.quiet), daemon=True)
            down = threading.Thread(target=pump, args=(upstream, client, "S->C", names, tally, args.quiet), daemon=True)
            up.start()
            down.start()
            up.join()
            down.join()
            client.close()
            upstream.close()
            print(f"--- {addr[0]}:{addr[1]} disconnected ---", flush=True)
    except KeyboardInterrupt:
        print("\nmitm: stopping", flush=True)
    finally:
        stop.set()
        listener.close()
        report = tally.report()
        if report:
            print("\nmitm: totals by direction")
            print(report)
    return 0


if __name__ == "__main__":
    sys.exit(main())
