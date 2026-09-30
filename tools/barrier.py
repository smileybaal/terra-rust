#!/usr/bin/env python3
"""barrier.py: name, from the sheet book, where a vanilla client will get stuck.

The port's problem is not that it is unfinished. Every server is unfinished. The
problem is that "unfinished" is silent: a client stalls, or shows an empty world,
and nothing says which message caused it or why. This tool answers that before a
client connects, and it does it from the same rows the Rust port is projected from,
so the prediction and the server cannot disagree.

How it works, and why it is a query over the book rather than a list of numbers:

  1. The client's opening is a SEQUENCE of message ids, in the order the client
     performs it. That sequence is in `capture.py` (and `replay-client.py`), cited
     per step to `MessageBuffer.cs`.

  2. For each id in that sequence, the answer is looked up in the BOOK: the port
     handles an id when a row in `sheets/kernel.tsv` claims it. The set of handled
     ids is declared in the sheet as a column of values rather than repeated here,
     because a hand-typed copy of a derivable set is exactly the bug this repository
     exists to avoid (D1: hand-typed copies of derivable values are bugs).

  3. The first id the port does NOT handle is the barrier. Everything after it is
     unreachable, whatever its status, which is why the output says "unreachable"
     rather than listing them as merely unfinished.

So the prediction is a JOIN of two relations: the client's sequence, and the
kernel's handled set. Adding a handler is an edit to `sheets/kernel.tsv`; the
prediction moves with it, with no second place to update.

    python tools\\barrier.py            # the prediction, for the default sequence
    python tools\\barrier.py --json     # the same, machine-readable
    python tools\\barrier.py --check    # exit 1 if the tool and the book disagree

`--check` is what makes this a check rather than a document: it asserts that the
handled set is non-empty, that every id it names is a real MessageID row, and that
the barrier it computes is the FIRST unhandled id (not merely an unhandled one).
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import wire  # noqa: E402
from wire import MessageNames, WireError  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
KERNEL_SHEET = ROOT / "sheets" / "kernel.tsv"

# The client's opening, in order. Each entry is (id, what the client is doing, cite).
# This sequence is the SAME one `capture.py` performs and `replay-client.py` performs:
# three tools claiming to speak the client's opening must agree, so it is written
# once here and the others say so.
CLIENT_OPENING: list[tuple[int, str, str]] = [
    (1, "Hello", "MessageBuffer.cs:181-222 (State 0 -> 1)"),
    (4, "SyncPlayer", "MessageBuffer.cs:248 (the first thing after PlayerInfo)"),
    (68, "Unknown68", "MessageBuffer.cs:248-253 (the early burst)"),
    (16, "PlayerLifeMana", "MessageBuffer.cs:1095"),
    (42, "Unknown42", "MessageBuffer.cs:248-253 (the early burst)"),
    (50, "PlayerBuffs", "MessageBuffer.cs:248-253 (the early burst)"),
    (147, "SyncLoadout", "MessageBuffer.cs:248-253 (the early burst)"),
    (6, "RequestWorldData", "MessageBuffer.cs:462-472 (State 1 -> 2)"),
    (8, "SpawnTileData", "MessageBuffer.cs:664-875 (State 2 -> 3, sections stream)"),
    (12, "PlayerSpawn", "MessageBuffer.cs:901-950 (State 3 -> 10)"),
]

# What the C# sends after the tile sections, in order, before the client is playable.
# These are things the SERVER must send, so "handled" means something different: it
# means the port has a sender for it. `sheets/kernel.tsv` records them by their name
# in the C#, so the same query applies.
SERVER_AFTER_SECTIONS: list[tuple[str, str]] = [
    ("ItemDrop", "MessageBuffer.cs:843-874 (the world's items)"),
    ("NPCBush", "MessageBuffer.cs:843-874 (the world's NPCs)"),
    ("Projectile", "MessageBuffer.cs:843-874 (live projectiles)"),
    ("Banner", "MessageBuffer.cs:843-874 (kill counts)"),
    ("CreativePowers", "MessageBuffer.cs:843-874 (the creative-power state)"),
    ("Pylons", "MessageBuffer.cs:843-874 (the pylon network)"),
]


@dataclass
class Handled:
    """What the kernel sheet says the port handles.

    Read from the sheet, never typed. Two shapes are recognised because the sheet
    records two kinds of claim:

      * a message id named in a row's `id` (`msg7`, `msg12_start`): the port sends or
        reads that id. The id is extracted from the row's TEXT, which is the sheet's
        own spelling, so a row that names a different id names a different handler.
      * a message NAME in a row's `notes` (`WorldData (7)`): the same claim written
        the other way round, which is how the sheet records the block of ids the
        world transfer uses.
    """

    ids: set[int]
    names: set[str]
    by_id: dict[int, str]
    by_name: dict[str, str]

    def has_id(self, mid: int) -> bool:
        return mid in self.ids

    def has_name(self, name: str) -> bool:
        return name in self.names

    def why_id(self, mid: int) -> str:
        return self.by_id.get(mid, "")

    def why_name(self, name: str) -> str:
        return self.by_name.get(name, "")


def read_handled(path: Path = KERNEL_SHEET, names: MessageNames | None = None) -> Handled:
    """The set of messages the kernel sheet claims, by id and by name.

    Three spellings are recognised, because the sheet uses three and all three are
    honest claims:

      * `SyncPlayer(4)` / `WorldData(7)` - a message NAME with its id in brackets.
        Both the name and the id are taken.
      * `msg12` / `msg_12` / `message 10` - the id spelled bare.
      * `the Hello handshake` - a message NAME with NO id, which is how the sheet
        describes the entry path. The id is resolved by joining the name against the
        MessageID rows, which is a join over two relations rather than a guess: if
        the id table has a member with that name, the claim is about that id, and if
        it does not, the name is recorded but claims no id.

    That third spelling is why this function takes the `MessageNames`: without it,
    `net`'s claim to the handshake would be invisible and the tool would report a
    barrier at id 1 against a server that answers id 1, which is the failure mode
    this whole tool exists to prevent (it reported exactly that on first run).
    """
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as e:
        raise WireError(f"cannot read {path}: {e}") from e

    ids: set[int] = set()
    msg_names: set[str] = set()
    by_id: dict[int, str] = {}
    by_name: dict[str, str] = {}

    # name (lowercased) -> id, for the third spelling. Built from the id table, so a
    # name the table does not carry resolves to nothing rather than to a guess.
    name_to_id: dict[str, int] = {}
    if names is not None:
        for mid, nm in names.by_id.items():
            name_to_id.setdefault(nm.lower(), mid)

    for line in text.splitlines():
        if not line or line.startswith("#"):
            continue
        cells = line.split("\t")
        if len(cells) < 3:
            continue
        row_id, _kind, _module = cells[0], cells[1], cells[2]
        notes = cells[5] if len(cells) > 5 else ""
        haystack = f"{row_id}\t{notes}"

        # `msg12`, `msg_12`, `message 10`: the id spelled bare.
        for m in re.finditer(r"\bmsg[_ ]?(\d+)\b", haystack, re.IGNORECASE):
            mid = int(m.group(1))
            ids.add(mid)
            by_id.setdefault(mid, f"{row_id} ({_module})")
        for m in re.finditer(r"\bmessage\s+(\d+)\b", haystack, re.IGNORECASE):
            mid = int(m.group(1))
            ids.add(mid)
            by_id.setdefault(mid, f"{row_id} ({_module})")

        # `SyncPlayer(4)`, `WorldData(7)`: a message NAME with its id in brackets.
        for m in re.finditer(r"\b([A-Z][A-Za-z0-9_]{2,})\s*\((\d+)\)", haystack):
            name, mid_s = m.group(1), m.group(2)
            msg_names.add(name)
            by_name.setdefault(name, f"{row_id} ({_module})")
            ids.add(int(mid_s))
            by_id.setdefault(int(mid_s), f"{row_id} ({_module}) via {name}")

        # A message NAME with no id. Resolved against the id table.
        for word in re.findall(r"\b[A-Z][A-Za-z0-9_]{2,}\b", haystack):
            mid = name_to_id.get(word.lower())
            if mid is None:
                continue
            # Only accept a word the id table knows as a message name; `Registry` and
            # `TileID` are not message names and the table has no member called that.
            msg_names.add(word)
            by_name.setdefault(word, f"{row_id} ({_module}) by name")
            ids.add(mid)
            by_id.setdefault(mid, f"{row_id} ({_module}) via the name {word}")

    return Handled(ids=ids, names=msg_names, by_id=by_id, by_name=by_name)


@dataclass
class Step:
    label: str
    mid: int | None
    cite: str
    handled: bool
    why: str
    state: str  # "reached" | "barrier" | "unreachable"

    def as_row(self) -> dict[str, object]:
        return {
            "label": self.label, "id": self.mid, "cite": self.cite,
            "handled": self.handled, "why": self.why, "state": self.state,
        }


def predict(names: MessageNames, handled: Handled) -> tuple[list[Step], Step | None]:
    """Walk the client's opening and mark the first step the port cannot answer.

    Once one step is unhandled, every later step is `unreachable`: that is the whole
    point of a barrier, and calling the later steps merely "unfinished" would hide the
    fact that their status does not matter yet.
    """
    steps: list[Step] = []
    barrier: Step | None = None
    for mid, label, cite in CLIENT_OPENING:
        ok = handled.has_id(mid)
        if barrier is None and not ok:
            state = "barrier"
        elif barrier is not None:
            state = "unreachable"
        else:
            state = "reached"
        name = names.name(mid)
        step = Step(
            label=f"{label} [{name}]" if name != label else label,
            mid=mid, cite=cite, handled=ok,
            why=handled.why_id(mid) if ok else "no kernel row claims this id",
            state=state,
        )
        steps.append(step)
        if state == "barrier" and barrier is None:
            barrier = step
    return steps, barrier


def predict_server_sends(handled: Handled) -> list[Step]:
    """What the server must SEND after the sections, and whether the port sends it.

    A different question from the client's opening: here an unhandled name is not a
    barrier that stops the load, it is a gap the client will notice as an empty world.
    Reported separately for that reason instead of being mixed into the walk.
    """
    out: list[Step] = []
    for name, cite in SERVER_AFTER_SECTIONS:
        ok = handled.has_name(name)
        out.append(Step(
            label=name, mid=None, cite=cite, handled=ok,
            why=handled.why_name(name) if ok else "no kernel row claims this message",
            state="sent" if ok else "missing",
        ))
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    ap.add_argument("--check", action="store_true",
                    help="assert the prediction is self-consistent; exit 1 if not")
    args = ap.parse_args()

    try:
        names = MessageNames.from_book()
        handled = read_handled(names=names)
    except WireError as e:
        print(f"barrier: {e}", file=sys.stderr)
        return 2

    steps, barrier = predict(names, handled)
    sends = predict_server_sends(handled)

    if args.check:
        problems: list[str] = []
        if not handled.ids:
            problems.append("the kernel sheet names no message id at all; the check "
                            "would pass on an empty set, which is not a check")
        for mid in sorted(handled.ids):
            if not names.is_known(mid):
                problems.append(f"the kernel sheet claims msg{mid}, which is not a "
                                f"MessageID row")
        seen_barrier = False
        for s in steps:
            if s.state == "barrier":
                seen_barrier = True
            if s.state == "reached" and not s.handled:
                problems.append(f"{s.label} is 'reached' but unhandled, which is a "
                                f"contradiction")
            if seen_barrier and s.handled and s.state == "unreachable":
                # Legal: a later step can be handled and still be unreachable, because
                # the barrier stops the stream before it. Not a problem, just worth
                # not flagging.
                pass
        if barrier is None:
            first_unhandled = next((s for s in steps if not s.handled), None)
            if first_unhandled is not None:
                problems.append("no barrier was found, but "
                                f"{first_unhandled.label} is unhandled")
        else:
            earlier = [s for s in steps if s.mid is not None and not s.handled
                       and s.state == "reached"]
            if earlier:
                problems.append("the barrier is not the FIRST unhandled step: "
                                + ", ".join(s.label for s in earlier))
        if problems:
            for p in problems:
                print(f"barrier: FAIL  {p}", file=sys.stderr)
            return 1
        print(f"barrier: ok  {len(handled.ids)} id(s) claimed by the kernel sheet, "
              f"barrier = {barrier.label if barrier else 'none in the client opening'}")
        return 0

    if args.json:
        print(json.dumps({
            "handled_ids": sorted(handled.ids),
            "handled_names": sorted(handled.names),
            "client_opening": [s.as_row() for s in steps],
            "barrier": barrier.as_row() if barrier else None,
            "server_after_sections": [s.as_row() for s in sends],
        }, indent=2))
        return 0

    print("CLIENT OPENING - where a vanilla client gets stopped")
    print()
    for s in steps:
        mark = {"reached": "  ok ", "barrier": "STOP ", "unreachable": "  -- "}[s.state]
        print(f"  {mark} id={s.mid:<4} {s.label}")
        print(f"        {s.cite}")
        if s.state == "reached":
            print(f"        handled by {s.why}")
        elif s.state == "barrier":
            print(f"        BARRIER: {s.why}")
        else:
            print(f"        unreachable until the barrier is passed")
    print()
    if barrier:
        print(f"VERDICT: a vanilla client reaches every message up to id {barrier.mid} "
              f"({barrier.label})")
        print(f"         and stops there, because the port has no handler for it.")
        print(f"         A client that cannot send id {barrier.mid} never loads the world.")
    else:
        print("VERDICT: every message in the client's opening is handled")

    print()
    print("SERVER SEND AFTER THE SECTIONS - what the client will notice missing")
    print()
    for s in sends:
        mark = "sent" if s.handled else "MISS"
        print(f"  {mark}  {s.label:18} {s.cite}")
    missing = [s.label for s in sends if not s.handled]
    if missing:
        print()
        print(f"  {len(missing)} of {len(sends)} are not sent: {', '.join(missing)}")
        print("  A client that gets terrain without these looks at an empty world.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
