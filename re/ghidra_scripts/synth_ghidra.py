#!/usr/bin/env python3
"""synth_ghidra.py: ghidra-cli JSON exports -> canonical evidence sheets.

THE SPREADSHEET METHOD, Mode B (MDD 8.4, 8.6). Producer output is EVIDENCE.

Inputs (re/exports/ghidra/):
  functions.json   ghidra function list --json --fields name,address,size
  symbols.json     ghidra symbol list --json
  types.json       ghidra type list --json
  interesting.json ghidra find interesting --json
  crypto.json      ghidra find crypto --json
  comments.json    ghidra comment list --json

Outputs (sheets/re/):
  functions.tsv    one row per native function
  strings.tsv      one row per recovered string (from the CLI metadata strings)
  triage.tsv       MDD 8.5 score, the work queue

Every row carries ref_addr + ref_conf + evidence (MDD 8.9 / O16).
"""
from __future__ import annotations

import hashlib
import io
import json
import math
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
EXPORTS = os.path.join(ROOT, "re", "exports", "ghidra")

# Ghidra analysed Terraria.exe only, so these sheets describe the CLIENT binary and
# live beside the other client evidence. The server has no Ghidra evidence yet:
# adding it means importing TerrariaServer.exe into its own project and re-running
# this script with a second output directory.
CLIENT_OUT = os.path.join(ROOT, "sheets", "re", "client")


# ---------------------------------------------------------------------------
# content root
# ---------------------------------------------------------------------------

# The game's Content directory is an INPUT, not a checked-in artifact: it is
# large (495 MB, 15994 .xnb) and it belongs to the user's own installation.
# Override with CONTENT_DIR; the default matches the Steam layout.
CONTENT_DIR = os.environ.get(
    "CONTENT_DIR",
    r"C:\Steam\steamapps\common\Terraria\Content",
)


def load(name: str):
    p = os.path.join(EXPORTS, name)
    if not os.path.exists(p):
        return None
    raw = io.open(p, encoding="utf-8", errors="replace").read().strip()
    if not raw:
        return None
    # ghidra-cli prints bridge chatter ("Starting Ghidra bridge...\nBridge
    # ready.") to the same stream before the payload when it has to start the
    # bridge, so skip everything before the first JSON delimiter. Silently
    # returning None here would have hidden a real data file.
    starts = [i for i in (raw.find("["), raw.find("{")) if i >= 0]
    if not starts:
        return None
    raw = raw[min(starts):]
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return None


def sanitize(cell: str) -> str:
    return str(cell).replace("\t", " ").replace("\r", " ").replace("\n", " ").strip()


def manifest(sheet: str, generator: str, requires: str, note: str, evidence: str = "required",
             extra: str = "") -> str:
    return (
        f"# sheet: {sheet}\n# version: 1\n# generator: {generator}\n# target: sheets/re\n"
        f"# index: by_id\n# requires: {requires}\n# owned_by: extractor\n# doctrine: D1\n"
        f"# evidence: {evidence}\n{extra}\n# {note}\n"
    )


def write(path: str, header: str, rows: list[list[str]]) -> int:
    # last writer wins per id, then sort. Duplicate ids can arise when two
    # distinct producer names fold to the same sanitized id; keeping one row per
    # id is what makes preflight check 6 (no duplicate keys) pass by design
    # rather than by accident.
    by_id: dict[str, list[str]] = {}
    for r in rows:
        by_id[r[0]] = r
    ordered = [by_id[k] for k in sorted(by_id)]
    body = "\n".join("\t".join(sanitize(c) for c in r) for r in ordered)
    with io.open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(header)
        if body:
            f.write(body + "\n")
    return len(ordered)


# ---------------------------------------------------------------------------
# functions -> functions.tsv, triage.tsv
# ---------------------------------------------------------------------------

def do_functions() -> int:
    """re/client/functions.tsv from ghidra's CLI symbol table.

    IMPORTANT (dec013): `ghidra stats` reports instructions=0 for this binary.
    Ghidra's CLI analyzer created 18300 function objects carrying names and
    addresses read from the .NET metadata, but decoded NO machine code, so a
disassembly request fails with "No instruction at address". Consequently:
    - `size` is NOT a code size and must not be read as complexity,
    - `calls`/`called_by` are unknown (they are recorded as 0 and flagged),
    - no call-graph edges exist (see re/client/callgraph.tsv),
    - MDD 8.5 triage scoring is meaningless here and is NOT emitted.

    The rows are still worth keeping: this is a real symbol inventory, and it is
    the provenance that ties 18300 addresses to managed method names.
    """
    funcs = load("functions.json") or []
    interesting = load("interesting.json") or []

    inter_addrs = set()
    if isinstance(interesting, list):
        for it in interesting:
            if isinstance(it, dict):
                a = it.get("address") or it.get("addr")
                if a:
                    inter_addrs.add(str(a).lstrip("0x").lower())

    rows: list[list[str]] = []
    for f in funcs:
        if not isinstance(f, dict):
            continue
        addr = str(f.get("address", ""))
        name = str(f.get("name", ""))
        size = int(f.get("size", 0) or 0)
        fid = f"fn_{addr.lower()}"
        named = "-" if name.startswith(("FUN_", "thunk_", "SUB_")) else "managed_name"
        rows.append([
            fid, name, "0x" + addr.lower(), str(size), "0", "0", named, "symbol_only",
            "ghidra/functions.json", "0x" + addr.lower(), "certain",
            f"ghidra CLI symbol record; NO instructions decoded (dec013), so size={size} is a metadata record length, not code size",
        ])

    header = (
        manifest("re/client/functions", "functions", "re/sources",
                 "CLI-managed symbol inventory from Ghidra for the CLIENT binary.\n# One row per symbol record.\n# dec013: instructions=0, so size/calls/called_by are NOT code measurements.",
                 extra="\n".join([
                     f"# source_rows: {len(funcs)}",
                     f"# dropped: {len(rows) - len({r[0] for r in rows})}",
                     "# dropped_reason: -" if len(rows) == len({r[0] for r in rows})
                     else "# dropped_reason: duplicate ids folded by the writer",
                 ])) +
        "id:string*\tname:string\taddr:string\tsize:u32\tcalls:u16\tcalled_by:u16\ttags:string\tstatus:string\tartifact:string\tref_addr:string\tref_conf:string\tevidence:string\n"
    )
    n = write(os.path.join(CLIENT_OUT, "functions.tsv"), header, rows)
    print(f"functions          {n}   (symbol records, status=symbol_only)")
    print(f"interesting hits   {len(inter_addrs)}   (note: no code, so proximity is not meaningful)")
    return n


# ---------------------------------------------------------------------------
# types -> domain/structs_pe.tsv  (PE-side type inventory)
# ---------------------------------------------------------------------------

def do_types() -> int:
    types = load("types.json")
    if types is None:
        return 0
    items = types if isinstance(types, list) else types.get("types", [])
    rows = []
    seen: dict[str, int] = {}
    for t in items:
        if isinstance(t, str):
            name, kind, size, path = t, "named", "-", "-"
        elif isinstance(t, dict):
            name = str(t.get("name", ""))
            kind = str(t.get("kind", "unknown"))
            size = str(t.get("size", "-"))
            path = str(t.get("path", t.get("category", "-")))
        else:
            continue
        if not name:
            continue
        # Ghidra's type names are read from a spacing, not from a C source, so
        # they carry non-id characters (`#blob`, `#~`, `undefined4 *`). Fold to
        # [a-z0-9_.] and disambiguate; the name column keeps the real spelling.
        tid = re.sub(r"[^a-z0-9_.]+", "_", name.lower()).strip("_")
        if not tid:
            tid = "unnamed"
        n = seen.get(tid, 0)
        seen[tid] = n + 1
        if n:
            tid = f"{tid}_{n}"
        rows.append([f"pe.{tid}", kind, size, name, path, "typemgr"])
    header = (
        manifest("re/client/types_pe", "types_pe", "re/sources",
                 "PE-side data type inventory from Ghidra's data type manager, for the CLIENT binary.\n# This is NOT the managed type inventory; that is re/client/types.tsv (ILSpy).\n# id_form: compound",
                 evidence="none",
                 extra="\n".join([
                     f"# source_rows: {len(items)}",
                     f"# dropped: {len(items) - len({r[0] for r in rows})}",
                     "# dropped_reason: -" if len(items) == len({r[0] for r in rows})
                     else "# dropped_reason: sanitized names folded together (see the name column)",
                 ])) +
        "id:string*\tkind:string\tsize:string\tname:string\tpath:string\tartifact:string\n"
    )
    n = write(os.path.join(CLIENT_OUT, "types_pe.tsv"), header, rows)
    print(f"pe types           {n}")
    return n


def do_strings() -> int:
    """re/client/strings.tsv from ghidra's recovered string table.

    dec008: rows whose text contains TAB or newline are excluded. The canonical
    TSV form forbids unescaped tabs, and a string table is exactly where they
    occur (several entries here are whole embedded files).
    """
    strings = load("strings_all.json")
    if not isinstance(strings, list):
        return 0
    rows = []
    seen: dict[str, int] = {}
    excluded = 0
    for s in strings:
        if not isinstance(s, dict):
            continue
        addr = str(s.get("address", ""))
        val = str(s.get("value", ""))
        length = s.get("length", 0)
        if not addr:
            continue
        if "\t" in val or "\n" in val or "\r" in val:
            excluded += 1
            continue
        # An empty string is a real value in a string table, not a missing one;
        # NULL is the canonical explicit marker (MDD 5.2), so use it rather than
        # an empty cell (which would mean "not specified").
        text = val if val else "NULL"
        sid = f"str_{addr.lower()}"
        n = seen.get(sid, 0)
        seen[sid] = n + 1
        if n:
            sid = f"{sid}_{n}"
        rows.append([
            sid, "0x" + addr.lower(), text, str(length), "0", "unknown", "identified",
            "ghidra/strings_all.json", "0x" + addr.lower(), "certain",
            f"ghidra find string; length={length}",
        ])
    header = (
        manifest("re/client/strings", "strings", "re/sources",
                 "String table from Ghidra for the CLIENT binary. Rows whose text contains TAB\n# or newline are excluded (dec008).",
                 extra="\n".join([
                     f"# source_rows: {len(strings)}",
                     f"# dropped: {excluded + (len(rows) - len({r[0] for r in rows}))}",
                     "# dropped_reason: text contains TAB or newline, which the canonical form forbids (dec008)",
                 ])) +
        "id:string*\taddr:string\ttext:string\tlength:u16\tx_refs:u16\tuse:string\tstatus:string\tartifact:string\tref_addr:string\tref_conf:string\tevidence:string\n"
    )
    n = write(os.path.join(CLIENT_OUT, "strings.tsv"), header, rows)
    print(f"strings            {n}   (excluded for TAB/newline: {excluded})")
    return n


def do_assets() -> int:
    """re/assets.tsv from the asset manifest embedded in the binary.

    One recovered string is the game's own content table: a TSV with
    Path/Width/Height columns. Reusing it is far better evidence than guessing
    asset names, and it ties directly to the on-disk Content directory.
    """
    strings = load("strings_all.json")
    if not isinstance(strings, list):
        return 0
    manifest_txt = None
    addr = ""
    for s in strings:
        if isinstance(s, dict) and str(s.get("value", "")).startswith("Path\tWidth\tHeight"):
            manifest_txt = str(s["value"])
            addr = str(s.get("address", ""))
            break
    if manifest_txt is None:
        print("assets             skipped (no embedded content manifest found)")
        return 0

    if not os.path.isdir(CONTENT_DIR):
        print(f"assets             skipped (CONTENT_DIR not found: {CONTENT_DIR})")
        return 0

    rows = []
    seen: dict[str, int] = {}
    parsed = 0
    for line in manifest_txt.split("\n")[1:]:
        parts = line.split("\t")
        if len(parts) < 3:
            continue
        parsed += 1
        path, w, h = parts[0].strip(), parts[1].strip(), parts[2].strip()
        if not path:
            continue
        aid = re.sub(r"[^a-z0-9_.]+", "_", path.lower()).strip("_") or "unnamed"
        n = seen.get(aid, 0)
        seen[aid] = n + 1
        if n:
            aid = f"{aid}_{n}"
        on_disk = os.path.join(CONTENT_DIR, path.replace("/", os.sep) + ".xnb")
        if os.path.isfile(on_disk):
            status = "found"
            size = str(os.path.getsize(on_disk))
            # A content hash is what makes an asset claim checkable later. Size
            # alone cannot detect a swapped file of the same length.
            # NB: do not name this `h` -- `h` already holds the manifest height.
            hasher = hashlib.sha256()
            with open(on_disk, "rb") as fh:
                for chunk in iter(lambda: fh.read(1 << 20), b""):
                    hasher.update(chunk)
            digest = hasher.hexdigest()
        else:
            status = "missing"
            # NULL is the explicit "do not default" marker (MDD 5.2). The asset
            # is not on disk, so its size and hash are genuinely unknown -- an
            # empty cell would incorrectly imply "inherit the column default".
            size = "NULL"
            digest = "NULL"
        rows.append([
            aid, path, size, digest, "xnb", status,
            "0x%s" % addr.lower(), "certain",
            "embedded Path/Width/Height manifest; dims %sx%s" % (w, h),
        ])
    header = (
        manifest("re/assets", "assets", "re/sources",
                 "Content inventory, SHARED by both binaries: the client and the server load the\n# same Content directory. Source is the Path/Width/Height manifest embedded in\n# Terraria.exe, cross-checked against re/content.\n# id_form: compound",
                 extra="\n".join([
                     f"# source_rows: {parsed}",
                     f"# dropped: {parsed - len({r[0] for r in rows})}",
                     "# dropped_reason: -" if parsed == len({r[0] for r in rows})
                     else "# dropped_reason: slash-normalised paths folded to the same id",
                 ])) +
        "id:string*\tpath:string\tsize:u64\tsha256:string\tloader:string\tstatus:string\tref_addr:string\tref_conf:string\tevidence:string\n"
    )
    n = write(os.path.join(ROOT, "sheets", "re", "assets.tsv"), header, rows)
    print(f"assets             {n}")
    found = sum(1 for r in rows if r[5] == "found")
    print(f"assets on disk     {found} / {len(rows)}")
    return n


def do_callgraph() -> int:
    """Check the Ghidra call-graph measurement. Writes NO sheet.

    dec013: Ghidra decoded 0 instructions for this binary, so there are no CALL
    references to walk and `ghidra graph calls` returns edge_count=0.

    This producer must not write re/client/callgraph.tsv: the managed producer owns
    that sheet, because it is the only one that can see edges on this target.
    Writing an empty sheet here would clobber real edges whenever the scripts
    run in a different order. Instead we record and assert the measurement.
    """
    g = load("callgraph.json")
    edge_count = None
    node_count = None
    if isinstance(g, list) and g and isinstance(g[0], dict):
        edge_count = g[0].get("edge_count")
        node_count = g[0].get("node_count")

    print(f"ghidra callgraph   edge_count={edge_count} over node_count={node_count} (expected 0: no decoded code)")
    if edge_count not in (0, None):
        print("    WARNING: ghidra reported edges, which contradicts dec013; investigate before trusting re/client/callgraph.tsv")
    return 0 if edge_count in (0, None) else 1


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

def main() -> int:
    if not os.path.isdir(EXPORTS):
        print("no ghidra exports dir", file=sys.stderr)
        return 1
    # the client evidence directory may not exist yet if this runs before synth_ilspy
    os.makedirs(CLIENT_OUT, exist_ok=True)
    do_functions()
    do_types()
    do_strings()
    do_assets()
    do_callgraph()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
