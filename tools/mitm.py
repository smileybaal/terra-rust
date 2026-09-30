#!/usr/bin/env python3
"""mitm.py: a logging TCP proxy for the Terraria wire protocol.

Point the vanilla client at this proxy and it relays every byte to the real server
while printing each framed message in both directions.

    python tools\\mitm.py --listen 127.0.0.1:1739 --target 127.0.0.1:1738

This tool is now a THIN WRAPPER over `tools/capture.py --mode proxy`. It used to
carry its own copy of the framing, its own `Reader`, its own body decoders and its
own reader for the message-name table, which meant four things that could disagree
with the Rust port and with each other. The framing and the names are properties of
the PROTOCOL, not of this tool, so they live in one place now (`tools/wire.py`), and
`capture.py` is the implementation.

Why keep the name and the CLI at all: it is the tool the parity roadmap tells a
reader to run (Stage 0), it is referenced by `dec013`'s notes, and the invocation
above is in the roadmap's own text. A rename would break a documented entry point to
save nine lines of shim, which is a bad trade.

What this wrapper adds over calling `capture.py` directly: nothing, deliberately. It
forwards its arguments and exits with the same code. The one thing it changes is the
default, kept from the original tool: `--listen` defaults to 127.0.0.1:1739 and
`--target` to 127.0.0.1:1738, the ports the roadmap names.

Note on the server browser: Terraria finds servers by a UDP broadcast to
255.255.255.255:8888 once a second (`Netplay.cs:826-874`), which this proxy does not
relay. That is fine for a client you add by IP, which is the usual case when you
already know the port. Relaying the beacon too would need the client to trust a port
number the proxy then has to lie about.
"""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
CAPTURE = HERE / "capture.py"


def main(argv: list[str]) -> int:
    """Forward to `capture.py --mode proxy`, keeping this tool's own defaults.

    A `--listen`/`--target` the user did not pass is supplied here rather than in
    capture.py, because those two defaults are this tool's documented behaviour and
    capture.py has none (it is driven by `host`/`port` positionally).
    """
    args = list(argv)
    if "--listen" not in args:
        args += ["--listen", "127.0.0.1:1739"]
    if "--target" not in args:
        args += ["--target", "127.0.0.1:1738"]
    cmd = [sys.executable, str(CAPTURE), "--mode", "proxy"] + args
    return subprocess.call(cmd)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
