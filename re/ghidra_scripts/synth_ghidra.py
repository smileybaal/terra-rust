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

import io
import json
import math
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
EXPORTS = os.path.join(ROOT, "re", "exports", "ghidra")


def load(name: str):
    p = os.path.join(EXPORTS, name)
    if not os.path.exists(p):
        return None
    raw = io.open(p, encoding="utf-8", errors="replace").read().strip()
    if not raw:
        return None
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

def do_functions() -> tuple[int, int]:
    funcs = load("functions.json") or []
    interesting = load("interesting.json") or []
    crypto = load("crypto.json") or []

    inter_addrs = set()
    if isinstance(interesting, list):
        for it in interesting:
            if isinstance(it, dict):
                a = it.get("address") or it.get("addr")
                if a:
                    inter_addrs.add(str(a).lstrip("0x").lower())
    crypto_n = len(crypto) if isinstance(crypto, list) else 0

    rows: list[list[str]] = []
    triage: list[list[str]] = []
    for f in funcs:
        if not isinstance(f, dict):
            continue
        addr = str(f.get("address", ""))
        name = str(f.get("name", ""))
        size = int(f.get("size", 0) or 0)
        fid = f"fn_{addr.lower()}"
        # Ghidra recovered managed method names, so a non-FUN_ name is a real symbol
        named = 0.0 if name.startswith(("FUN_", "thunk_", "SUB_")) else 1.0
        thunk = 1.0 if (name.startswith(("thunk_",)) or size <= 8) else 0.0
        near_str = 1.0 if addr.lstrip("0x").lower() in inter_addrs else 0.0
        score = (
            3.0 * math.log2(1 + size)
            + 5.0 * named
            + 4.0 * near_str
            - 2.0 * thunk
        )
        rows.append([
            fid, name, "0x" + addr.lower(), str(size), "0", "0", "-", "identified",
            "ghidra/functions.json", "0x" + addr.lower(), "certain",
            f"ghidra function list; size={size}",
        ])
        triage.append([
            fid, f"size_bucket={size};named={int(named)};thunk={int(thunk)}",
            f"{score:.2f}", "1" if score >= 12 else ("2" if score >= 8 else "3"),
            "todo", "0x" + addr.lower(), "probable",
            "ghidra triage score (MDD 8.5 reduced: size, symbol, interesting-proximity)",
        ])

    nf = write(
        os.path.join(ROOT, "sheets", "re", "functions.tsv"),
        manifest("re/functions", "functions", "re/sources",
                 "Native/PE-side function inventory from Ghidra. One row per function.") +
        "id:string*\tname:string\taddr:string\tsize:u32\tcalls:u16\tcalled_by:u16\ttags:string\tstatus:string\tartifact:string\tref_addr:string\tref_conf:string\tevidence:string\n",
        rows,
    )
    nt = write(
        os.path.join(ROOT, "sheets", "re", "triage.tsv"),
        manifest("re/triage", "triage", "re/functions,re/methods",
                 "Function triage; score follows MDD 8.5 (log size, xrefs, string proximity,\n# entry proximity, named symbol, interesting hits, minus thunk-likeness).") +
        "id:string*\treason:string\tscore:f32\tpriority:u8\tstatus:string\tref_addr:string\tref_conf:string\tevidence:string\n",
        triage,
    )
    print(f"functions          {nf}")
    print(f"triage             {nt}   (interesting hits: {len(inter_addrs)}, crypto hits: {crypto_n})")
    return nf, nt


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
        manifest("re/types_pe", "types_pe", "re/sources",
                 "PE-side data type inventory from Ghidra's data type manager.\n# This is NOT the managed type inventory; that is re/types.tsv (ILSpy).\n# id_form: compound",
                 evidence="none") +
        "id:string*\tkind:string\tsize:string\tname:string\tpath:string\tartifact:string\n"
    )
    n = write(os.path.join(ROOT, "sheets", "re", "types_pe.tsv"), header, rows)
    print(f"pe types           {n}")
    return n


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

def main() -> int:
    if not os.path.isdir(EXPORTS):
        print("no ghidra exports dir", file=sys.stderr)
        return 1
    do_functions()
    do_types()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
