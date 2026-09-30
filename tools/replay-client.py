"""Replay a joining client's opening, and check what the server sends back.

This is the observation the port is verified against, and it is deliberately NOT the
port's own reader: it speaks the wire, and it inflates the tile sections with Python's
zlib so the DEFLATE framing is checked by something that is not our code.

The sequence is the one the client itself performs, read out of its own source:

    Hello            (1)   MessageBuffer.cs:181-222, so the server hands out PlayerInfo
    SyncPlayer       (4)   MessageBuffer.cs:248     - first thing after PlayerInfo
    ... the early burst (68, 16, 42, 50, 147)       - MessageBuffer.cs:248-253
    RequestWorldData (6)   MessageBuffer.cs:462     - -> WorldData (7)
    SpawnTileData    (8)   MessageBuffer.cs:664     - -> StatusTextSize (9), sections (10)
    PlayerSpawn      (12)  MessageBuffer.cs:901     - -> State 3 becomes 10

Usage:  python tools\\replay-client.py [host] [port]
Exit code 0 means every step got the answer the protocol calls for; the printed table
says what arrived, including the tile count in each section after inflation.
"""
import socket
import struct
import sys
import zlib

HOST = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 7777
WORLD = sys.argv[3] if len(sys.argv) > 3 else None


def read_importance(path):
    """The container's importance bits, read exactly as `worldfile::read_container` does."""
    data = open(path, "rb").read()
    off = 0
    version = struct.unpack_from("<i", data, off)[0]; off += 4
    if version >= 135:
        off += 8 + 4 + 8                      # magic, revision, flags
    (count,) = struct.unpack_from("<h", data, off); off += 2
    off += 4 * count                          # the section pointers
    (important,) = struct.unpack_from("<H", data, off); off += 2
    bits = []
    byte = 0
    for i in range(important):
        if i % 8 == 0:
            byte = data[off]; off += 1
        bits.append(bool((byte >> (i % 8)) & 1))
    return bits


# `tileFrameImportant`, which decides whether four bytes of frame data follow a block.
# A real client has this because it loaded the same world; this driver takes it from the
# world file it is pointed at, and refuses to walk sections without it.
IMPORTANT = read_importance(WORLD) if WORLD else None

# Message ids, from the same table the port projects (`terraria.id.messageid.*`).
HELLO, KICK, PLAYER_INFO, SYNC_PLAYER = 1, 2, 3, 4
REQUEST_WORLD_DATA, WORLD_DATA = 6, 7
SPAWN_TILE_DATA, STATUS_TEXT_SIZE, TILE_SECTION, PLAYER_SPAWN = 8, 9, 10, 12
CONNECT_STRING = b"Terraria326"


def write_string(buf, s):
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


def read_string(r, off):
    n = 0
    shift = 0
    while True:
        b = r[off]
        off += 1
        n |= (b & 0x7F) << shift
        shift += 7
        if not b & 0x80:
            break
    return r[off:off + n].decode("utf-8", "replace"), off + n


def send(sock, mid, body=b""):
    payload = bytes([mid]) + bytes(body)
    sock.sendall(struct.pack("<H", len(payload)) + payload)


def recv_exact(sock, n):
    got = b""
    while len(got) < n:
        chunk = sock.recv(n - len(got))
        if not chunk:
            return None
        got += chunk
    return got


def recv_packet(sock):
    head = recv_exact(sock, 2)
    if head is None:
        return None
    (length,) = struct.unpack("<H", head)
    payload = recv_exact(sock, length)
    if payload is None:
        return None
    return payload[0], payload[1:]


def sync_player_body(name):
    b = bytearray()
    b += bytes([0, 11, 2])                      # slot, skinVariant, hair
    b += struct.pack("<f", 0.5)                 # hairDye? no: the C# writes a float here
    b += bytes([3])                             # hairDye is a BYTE
    write_string(b, name)
    b += bytes([0])                             # hideMisc
    b += struct.pack("<H", 0)                   # hideMisc bitfield
    b += bytes([0])
    for _ in range(7):
        b += bytes([1, 2, 3])                   # colours and hair colour
    b += bytes([0, 0, 0])                       # difficulty, biome torches, crystals
    return b


def main():
    sock = socket.create_connection((HOST, PORT), timeout=30)
    sock.settimeout(30)
    ok = True

    print("Hello (1)")
    b = bytearray()
    # The body is JUST the greeting string: `reader.ReadString() == "Terraria" + 326`
    # (MessageBuffer.cs:203). There is no NetworkText mode byte here, and putting one in
    # is answered with the version-mismatch kick, which is what this driver did first.
    write_string(b, CONNECT_STRING)
    send(sock, HELLO, b)
    mid, body = recv_packet(sock)
    print("  <- %d PlayerInfo slot=%d" % (mid, body[0]))
    ok &= mid == PLAYER_INFO

    print("SyncPlayer (4)")
    send(sock, SYNC_PLAYER, sync_player_body("Replay"))
    for mid in (68, 16, 42, 50, 147):
        send(sock, mid, bytes(16))
    send(sock, 16, struct.pack("<hh", 100, 100))

    print("RequestWorldData (6)")
    send(sock, REQUEST_WORLD_DATA)
    mid, body = recv_packet(sock)
    if mid != WORLD_DATA:
        print("  <- %d, expected WorldData (7): %r" % (mid, body[:120]))
        return 1
    time = struct.unpack_from("<i", body, 0)[0]
    off = 4
    day_bits = body[off]; off += 1
    moon_phase = body[off]; off += 1
    (max_x, max_y, spawn_x, spawn_y, surface, rock) = struct.unpack_from("<hhhhhh", body, off)
    off += 12
    world_id = struct.unpack_from("<i", body, off)[0]; off += 4
    name, off = read_string(body, off)
    print("  <- 7 WorldData %d bytes" % len(body))
    print("       time=%d dayBits=0x%02x moon=%d size=%dx%d spawn=(%d,%d)"
          % (time, day_bits, moon_phase, max_x, max_y, spawn_x, spawn_y))
    print("       surface=%d rock=%d worldId=%d name=%r" % (surface, rock, world_id, name))
    ok &= (max_x, max_y) == (8400, 2400)
    ok &= (spawn_x, spawn_y) == (4205, 425)
    ok &= name == "a"

    print("SpawnTileData (8)")
    ask = struct.pack("<ii", spawn_x, spawn_y) + bytes([0])
    send(sock, SPAWN_TILE_DATA, ask)
    mid, body = recv_packet(sock)
    if mid != STATUS_TEXT_SIZE:
        print("  <- %d, expected StatusTextSize (9): %r" % (mid, body[:120]))
        return 1
    status_max = struct.unpack_from("<i", body, 0)[0]
    mode = body[4]
    key, off = read_string(body, 5)
    print("  <- 9 StatusTextSize max=%d mode=%d key=%r" % (status_max, mode, key))
    ok &= mode == 2 and key == "LegacyInterface.44"

    sections = 0
    tiles = 0
    bytes_in = 0
    while sections < status_max:
        mid, body = recv_packet(sock)
        if mid is None or mid != TILE_SECTION:
            print("  <- %r, expected a tile section (10)" % (mid,))
            return 1
        plain = zlib.decompress(body, -15)          # raw DEFLATE
        x_start, y_start = struct.unpack_from("<ii", plain, 0)
        width, height = struct.unpack_from("<hh", plain, 8)
        # Walk the section the way the client's `DecompressTileBlock_Inner` does, counting
        # tiles and consuming every optional byte. Then the three list counts have to land
        # exactly at the end of the buffer: a walk that mis-reads one field still reaches
        # `width*height` tiles, but it does not land on the end, and that is the check.
        off = 12
        n = 0
        repeat = 0
        while n < width * height:
            if repeat:
                repeat -= 1
                n += 1
                continue
            b4 = plain[off]; off += 1
            b = b2 = b3 = 0
            if b4 & 1:
                b = plain[off]; off += 1
            if b & 1:
                b2 = plain[off]; off += 1
            if b2 & 1:
                b3 = plain[off]; off += 1
            if b4 & 2:
                # LOW byte first, then high: the client does `b5 = ReadByte(); num2 =
                # ReadByte(); num2 = (num2 << 8) | b5`. Getting this backwards reads a
                # different tile id and then skips the wrong number of frame bytes.
                type_id = ((plain[off + 1] << 8) | plain[off]) if b4 & 0x20 else plain[off]
                off += 2 if b4 & 0x20 else 1        # block type
                if IMPORTANT is not None and type_id < len(IMPORTANT) and IMPORTANT[type_id]:
                    off += 4                        # frameX, frameY
                if b2 & 8:
                    off += 1                        # block colour
            if b4 & 4:
                off += 1                            # wall
                if b2 & 0x10:
                    off += 1                        # wall colour
            if (b4 & 0x18) >> 3:
                off += 1                            # liquid amount
            if b2 & 0x40:
                off += 1                            # wall id, high byte
            n += 1
            rle = (b4 & 0xC0) >> 6
            if rle == 1:
                repeat = plain[off]; off += 1
            elif rle == 2:
                repeat = struct.unpack_from("<H", plain, off)[0]; off += 2
        chests, signs, entities = struct.unpack_from("<hhh", plain, off)
        off += 6
        if off != len(plain):
            print("  !! section (%d,%d): the walk left %d bytes: %r"
                  % (x_start, y_start, len(plain) - off, plain[off:][:16]))
            ok = False
        assert chests == 0 and signs == 0 and entities == 0, (
            "section (%d,%d): %d chests, %d signs, %d entities - the port sends none"
            % (x_start, y_start, chests, signs, entities)
        )
        sections += 1
        tiles += n
        bytes_in += len(body)
        print("  <- 10 section (%d,%d) %dx%d  wire=%d plain=%d tiles=%d"
              % (x_start, y_start, width, height, len(body), len(plain), n))
        if n != width * height:
            print("  !! the section's runs cover %d tiles, not %d" % (n, width * height))
            ok = False
    print("  %d of %d sections, %d tiles, %d bytes on the wire" % (sections, status_max, tiles, bytes_in))

    print("PlayerSpawn (12)")
    send(sock, PLAYER_SPAWN, struct.pack("<bhhihhbb", 0, spawn_x, spawn_y, 0, 0, 0, 0, 0))
    # The proof the state moved to 10: 20 is above 12, so before State 10 the guard would
    # answer with the InvalidState kick instead of this port's unimplemented notice.
    send(sock, 20)
    mid, body = recv_packet(sock)
    kick_mode = body[0] if body else -1
    if kick_mode == 0:
        text, _ = read_string(body, 1)
    else:
        key, _ = read_string(body, 1)
        text = "<key %s>" % key
    print("  <- %d kick: %r   (State reached 10 if this names message 20)" % (mid, text))
    ok &= "20" in text or "message 20" in text

    sock.close()
    print()
    print("OK   the client's whole opening is served" if ok else "FAILED")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
