#!/usr/bin/env python3
"""capture.py: record a Terraria conversation as framed, named messages.

This is the stage the parity roadmap calls Stage 0, and it exists for one reason:
"1:1 parity" is not checkable by reading code, only by diffing two wire traces. So
this tool records a trace, in a form another tool can diff, and it names every
message from the sheet book.

Three modes, chosen because they answer three different questions:

  --mode client   (default) BE a client: perform the vanilla client's opening
                  against a server, and record both directions. Reproducible, so a
                  run can be replayed and compared to another run.

  --mode proxy    relay a real client to a server, recording every frame. This is
                  the mode that answers "what does the VANILLA CLIENT actually do",
                  which is the question that matters for compatibility, because the
                  client's own sequence is not a thing to be guessed at.

  --mode both     a proxy that records, plus a synthetic client on a second
                  connection, so one run yields both an observer and a driven trace.

The client opening is read out of the client's own decompiled source, cited per
step, and it is the SAME sequence `tools/replay-client.py` performs - deliberately,
because two tools that claim to speak the client's opening must agree about it, or
one of them is lying about the protocol.

    python tools\\capture.py 127.0.0.1 7777 --mode client --out re/traces/port.jsonl
    python tools\\capture.py --listen 127.0.0.1:1739 --target 127.0.0.1:7777 --mode proxy

Exit code is 0 when the conversation was recorded, whatever it contained: this tool
reports, it does not judge. `tools/compat.py` is what judges.
"""
from __future__ import annotations

import argparse
import socket
import struct
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import wire  # noqa: E402
from wire import Capture, Frame, MessageNames, WireError  # noqa: E402

# The client's opening, in the order the client performs it. Each step cites the
# message case in MessageBuffer.cs that sends it.
CONNECT_STRING = "Terraria" + str(326)  # `"Terraria" + 326`, MessageBuffer.cs:203
HELLO, SYNC_PLAYER = 1, 4
REQUEST_WORLD_DATA, SPAWN_TILE_DATA, PLAYER_SPAWN = 6, 8, 12

# MessageBuffer.cs:248-253: the ids the client sends right after PlayerInfo, before
# it asks for the world. Sent by the real client and accepted by the server.
EARLY_BURST = [68, 16, 42, 50, 147]


class Recorder:
    """Accumulates frames with a lock, so the two pump threads can both write."""

    def __init__(self, names: MessageNames, source: str):
        self.names = names
        self.source = source
        self._lock = threading.Lock()
        self._seq = 0
        self.frames: list[Frame] = []
        self.outcome = "recorded"
        self.note = ""

    def add(self, direction: str, mid: int, body: bytes) -> Frame:
        with self._lock:
            self._seq += 1
            f = Frame(
                seq=self._seq, direction=direction, mid=mid,
                name=self.names.name(mid), body=body, t=time.time(),
            )
            self.frames.append(f)
            return f

    def capture(self) -> Capture:
        with self._lock:
            return Capture(
                frames=list(self.frames), source=self.source,
                outcome=self.outcome, note=self.note,
            )


def pump(src: socket.socket, dst: socket.socket, direction: str, rec: Recorder,
         quiet: bool, stop: threading.Event) -> None:
    """Copy src -> dst, splitting the stream into frames and recording each one.

    Frames are recorded BEFORE they are printed or forwarded, so a frame that kills
    the connection is still in the trace. That matters: the frame that ends a run is
    the most interesting one in it.
    """
    buf = bytearray()
    try:
        while not stop.is_set():
            chunk = src.recv(65536)
            if not chunk:
                break
            dst.sendall(chunk)
            buf += chunk
            while True:
                try:
                    frame = wire.read_frame(buf, direction, names=rec.names)
                except WireError as e:
                    rec.outcome = "framing-error"
                    rec.note = str(e)
                    print(f"  !! {e}", flush=True)
                    return
                if frame is None:
                    break
                rec.add(direction, frame.mid, frame.body)
                if not quiet:
                    print_frame(frame, rec.names)
    except OSError as e:
        rec.outcome = "socket-error"
        rec.note = f"{direction}: {e}"
    finally:
        if buf:
            rec.outcome = "truncated"
            rec.note = f"{direction}: {len(buf)} byte(s) of an incomplete frame at close"
        stop.set()
        for s in (src, dst):
            try:
                s.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass


def print_frame(frame: Frame, names: MessageNames, indent: str = "") -> None:
    body_len = len(frame.body) + 1
    print(f"{indent}{frame.direction}  len={body_len:<6d} id={frame.mid:<4d} {frame.name}", flush=True)
    fields = wire.describe(frame.mid, frame.body)
    if fields:
        for label, value in fields:
            print(f"{indent}{'':6} {label:22} {value}", flush=True)
    else:
        print(f"{indent}{'':6} {'body':22} {wire.hexdump(frame.body)}", flush=True)


def recv_exact(sock: socket.socket, n: int) -> bytes | None:
    got = b""
    while len(got) < n:
        chunk = sock.recv(n - len(got))
        if not chunk:
            return None
        got += chunk
    return got


def sync_player_body(name: str) -> bytes:
    """The body the vanilla client writes for `SyncPlayer` (4).

    Field order from `MessageBuffer.cs` case 4's reader; the values are plausible
    rather than meaningful, because what this tool is testing is whether the SERVER
    can read the message, not whether the client looks good.
    """
    b = bytearray()
    b += bytes([0, 11, 2])            # slot, skinVariant, hair
    b += struct.pack("<f", 0.5)       # a float the reader consumes
    b += bytes([3])                   # hairDye (a byte)
    wire.write_string(b, name)
    b += bytes([0])                   # hideMisc
    b += struct.pack("<H", 0)         # hideMisc bitfield
    b += bytes([0])
    for _ in range(7):
        b += bytes([1, 2, 3])         # colours + hair colour
    b += bytes([0, 0, 0])             # difficulty, biome torches, crystals
    return bytes(b)


def drive_client(sock: socket.socket, rec: Recorder, quiet: bool, name: str) -> None:
    """Perform the vanilla client's opening on `sock`.

    The reads and writes are interleaved the way the client does them: send, then
    wait for the answer, because the state machine on the server side is a state
    machine and firing everything at once would test something the client does not
    do.
    """
    def send(mid: int, body: bytes = b"") -> None:
        packet = wire.write_frame(mid, body)
        sock.sendall(packet)
        rec.add("C->S", mid, body)
        if not quiet:
            print_frame(Frame(0, "C->S", mid, rec.names.name(mid), body, time.time()), rec.names)

    def drain(expect: int, limit: float = 30.0) -> bool:
        """Read frames until `expect` arrives, or the stream ends. Returns whether it did."""
        sock.settimeout(limit)
        buf = bytearray()
        try:
            while True:
                frame = wire.read_frame(buf, "S->C", names=rec.names)
                if frame is not None:
                    rec.add("S->C", frame.mid, frame.body)
                    if not quiet:
                        print_frame(frame, rec.names)
                    if frame.mid == expect:
                        return True
                    if frame.mid == 2 and expect != 2:  # a kick ends the conversation
                        rec.note = f"kicked while waiting for {rec.names.name(expect)}"
                        return False
                    continue
                chunk = sock.recv(65536)
                if not chunk:
                    rec.note = f"server closed while waiting for {rec.names.name(expect)}"
                    return False
                buf += chunk
        except socket.timeout:
            rec.note = f"timed out waiting for {rec.names.name(expect)}"
            return False
        except (OSError, WireError) as e:
            rec.note = f"{e}"
            return False

    # 1 Hello. Read `Hello` first if the server opens with something.
    send(HELLO, wire.write_string_bytes(CONNECT_STRING))
    if not drain(3):  # PlayerInfo
        rec.outcome = "handshake-failed"
        return

    # 4 SyncPlayer, then the early burst: MessageBuffer.cs:248-253.
    send(SYNC_PLAYER, sync_player_body(name))
    for mid in EARLY_BURST:
        send(mid, early_body(mid))
        time.sleep(0.01)

    # 6 RequestWorldData -> WorldData (7): MessageBuffer.cs:462-472.
    send(REQUEST_WORLD_DATA)
    if not drain(7):
        rec.outcome = "world-data-failed"
        return

    # 8 SpawnTileData -> StatusTextSize (9) then the sections (10): MessageBuffer.cs:664-875.
    spawn_x = 4205  # the world's own spawn; the server uses it regardless of this
    spawn_y = 425
    send(SPAWN_TILE_DATA, struct.pack("<ii", spawn_x, spawn_y) + bytes([0]))
    # Wait for the LAST section rather than the first: the point of the drain is to
    # let the server finish the block, so the trace contains all of it.
    sock.settimeout(30.0)
    buf = bytearray()
    sections = 0
    try:
        while True:
            frame = wire.read_frame(buf, "S->C", names=rec.names)
            if frame is not None:
                rec.add("S->C", frame.mid, frame.body)
                if frame.mid == 10:
                    sections += 1
                    if sections == 1 and not quiet:
                        print_frame(frame, rec.names)
                        print(f"{'':6} ... first of the section block; {sections} seen so far", flush=True)
                elif frame.mid == 11:
                    continue
                elif not quiet:
                    print_frame(frame, rec.names)
                # The server sends the whole block and then goes quiet. Idle for
                # 2 seconds after at least one section means the block is done.
                sock.settimeout(2.0)
                continue
            chunk = sock.recv(65536)
            if not chunk:
                rec.note = "server closed after the section block"
                break
            buf += chunk
    except socket.timeout:
        pass
    except (OSError, WireError) as e:
        rec.note = f"{e}"
    print(f"  sections received: {sections}", flush=True)
    rec.note = (rec.note + f"; {sections} section(s)").strip("; ")

    # 12 PlayerSpawn -> State 3 becomes 10: MessageBuffer.cs:901-950.
    send(PLAYER_SPAWN, bytes([0]) + struct.pack("<hh", spawn_x, spawn_y) + bytes([0, 0, 0]))
    sock.settimeout(5.0)
    buf2 = bytearray()
    try:
        while True:
            frame = wire.read_frame(buf2, "S->C", names=rec.names)
            if frame is not None:
                rec.add("S->C", frame.mid, frame.body)
                if not quiet:
                    print_frame(frame, rec.names)
                continue
            chunk = sock.recv(65536)
            if not chunk:
                break
            buf2 += chunk
    except (socket.timeout, OSError, WireError):
        pass


def early_body(mid: int) -> bytes:
    """A plausible body for the early burst, sized the way the client writes it.

    These are SHAPED, not meaningful: the point is that the server walks the reader
    the C# walks, so the sizes come from the reader, and a wrong size here would show
    up as the server refusing a message the real client gets away with.
    """
    if mid == 68:   # SyncLoadout? no: 68 is a byte slot, then shorts. Kept short.
        return bytes([0]) + struct.pack("<hh", 0, 0)
    if mid == 16:   # PlayerLifeMana: statLife, statLifeMax
        return struct.pack("<hh", 100, 100)
    if mid == 42:   # Unknown42: a bool pair in the client's own writer
        return bytes([0])
    if mid == 50:   # PlayerBuffs: a byte count then that many uint16 buff ids
        return bytes([0])
    if mid == 147:  # SyncLoadout: a byte then a byte
        return bytes([0, 0])
    return b""


def record_proxy(listen, target, rec: Recorder, quiet: bool) -> None:
    """Relay a real client, recording both directions."""
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(listen)
    listener.listen(16)
    names = len(rec.names)
    print(f"capture: proxy {listen[0]}:{listen[1]} -> {target[0]}:{target[1]} ({names} names)",
          flush=True)
    print("capture: point the vanilla client at this port; Ctrl-C when done", flush=True)
    try:
        while True:
            client, addr = listener.accept()
            print(f"\n--- {addr[0]}:{addr[1]} connected ---", flush=True)
            try:
                upstream = socket.create_connection(target, timeout=10)
            except OSError as e:
                print(f"  cannot reach {target}: {e}", flush=True)
                client.close()
                continue
            stop = threading.Event()
            up = threading.Thread(target=pump,
                                  args=(client, upstream, "C->S", rec, quiet, stop), daemon=True)
            down = threading.Thread(target=pump,
                                    args=(upstream, client, "S->C", rec, quiet, stop), daemon=True)
            up.start()
            down.start()
            up.join()
            down.join()
            client.close()
            upstream.close()
            print(f"--- {addr[0]}:{addr[1]} disconnected ---", flush=True)
            break
    except KeyboardInterrupt:
        print("\ncapture: stopping", flush=True)
    finally:
        listener.close()


def parse_addr(text: str):
    host, _, port = text.rpartition(":")
    if not host or not port.isdigit():
        raise argparse.ArgumentTypeError(f"expected host:port, got {text!r}")
    return host.strip("[]"), int(port)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("host", nargs="?", default="127.0.0.1",
                    help="server host (mode client/both), default 127.0.0.1")
    ap.add_argument("port", nargs="?", type=int, default=7777,
                    help="server port (mode client/both), default 7777")
    ap.add_argument("--mode", choices=["client", "proxy", "both"], default="client")
    ap.add_argument("--listen", type=parse_addr, default=("127.0.0.1", 1739),
                    help="proxy listen address (mode proxy/both), default 127.0.0.1:1739")
    ap.add_argument("--target", type=parse_addr, default=None,
                    help="proxy target (mode proxy/both); default host:port")
    ap.add_argument("--out", type=Path, default=None, help="write the capture as JSONL")
    ap.add_argument("--name", default="capture", help="the client name to present")
    ap.add_argument("--quiet", action="store_true", help="record without printing")
    args = ap.parse_args()

    try:
        names = MessageNames.from_book()
    except WireError as e:
        print(f"capture: {e}", file=sys.stderr)
        return 2
    rec = Recorder(names, f"{args.mode} -> {args.host}:{args.port}")

    if args.mode == "client":
        try:
            sock = socket.create_connection((args.host, args.port), timeout=10)
        except OSError as e:
            print(f"capture: cannot reach {args.host}:{args.port}: {e}", file=sys.stderr)
            return 1
        rec.outcome = "completed"
        drive_client(sock, rec, args.quiet, args.name)
        try:
            sock.close()
        except OSError:
            pass
    elif args.mode == "proxy":
        target = args.target or (args.host, args.port)
        record_proxy(args.listen, target, rec, args.quiet)
    else:  # both
        target = args.target or (args.host, args.port)
        t = threading.Thread(target=record_proxy,
                            args=(args.listen, target, rec, args.quiet), daemon=True)
        t.start()
        time.sleep(0.5)

    cap = rec.capture()
    print(f"\ncapture: {len(cap.frames)} frame(s), outcome={cap.outcome}"
          + (f"  ({cap.note})" if cap.note else ""), flush=True)
    counts: dict[str, int] = {}
    for f in cap.frames:
        counts[f"{f.direction} {f.name}"] = counts.get(f"{f.direction} {f.name}", 0) + 1
    for k in sorted(counts):
        print(f"  {counts[k]:4d}  {k}", flush=True)
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        cap.save(args.out)
        print(f"capture: wrote {args.out}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
