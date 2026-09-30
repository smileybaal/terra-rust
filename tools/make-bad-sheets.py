#!/usr/bin/env python3
"""make-bad-sheets.py: generate malformed fixtures that exercise every preflight rule.

"Every firing rule proven by a committed test" was an overstatement: only a
handful of diagnostics were asserted, and E-SCHEMA-DRIFT had never fired at all.
MDD: "a rule that never fires may be checking nothing."

This writes a self-contained sweep fixture under tests/sweep/. It is self-contained
on purpose: check 22 scans the sheets directory's *parent* and check 21 looks for
<parent>/kernel, so both need files outside sheets/ but inside the fixture root.

Run: python tools/make-bad-sheets.py
Then: powershell -File tools/test-rules-sweep.ps1
"""
from __future__ import annotations

import os
import shutil

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SWEEP = os.path.join(ROOT, "tests", "sweep")
SHEETS = os.path.join(SWEEP, "sheets")

MANIFEST = (
    "# sheet: {name}\n# version: {ver}\n# generator: sweep\n# target: scratch\n"
    "# index: by_id\n# requires: -\n# owned_by: verifier\n# doctrine: D1\n{extra}"
)


def w(rel: str, text: str, *, bom: bool = False, crlf: bool = False, raw: bytes | None = None) -> None:
    p = os.path.join(SHEETS, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    if raw is not None:
        with open(p, "wb") as f:
            f.write(raw)
        return
    if crlf:
        text = text.replace("\n", "\r\n")
    data = text.encode("utf-8")
    if bom:
        data = b"\xef\xbb\xbf" + data
    with open(p, "wb") as f:
        f.write(data)


def main() -> int:
    if os.path.isdir(SWEEP):
        shutil.rmtree(SWEEP)
    os.makedirs(SHEETS)

    # --- doctrine: needed so the emitter-version check has something to compare
    w("00-doctrine.tsv", MANIFEST.format(name="00-doctrine", ver=1, extra="") + "id:string*\tkey:string\tvalue:string\n"
      "d1\temitter_version\t1\n")

    # --- 01-schema: deliberately disagrees with drift.tsv on one column type,
    # declares a column the header does not have, and omits one the header does.
    w("01-schema.tsv",
      MANIFEST.format(name="01-schema", ver=1, extra="# id_form: compound\n")
      + "id:string*\tsheet:string\tcolumn:string\ttype:string\tunit:string\toptional:string\tref:string\tdefault:string\tview:string\tnotes:string?\n"
      "drift.n\tdrift\tn\tf32\t-\tno\t-\t-\tv_full\t-\n"
      "drift.ghost\tdrift\tghost\tstring\t-\tyes\t-\t-\tv_full\tdeclared but absent from the header\n")

    # --- schema drift / extra / missing
    w("drift.tsv", MANIFEST.format(name="drift", ver=1, extra="") +
      "id:string*\tn:u16\textra_col:string\nx\t5\there\n")

    # --- structural (L0)
    w("bom.tsv", MANIFEST.format(name="bom", ver=1, extra="") + "id:string*\nz\n", bom=True)
    w("crlf.tsv", MANIFEST.format(name="crlf", ver=1, extra="") + "id:string*\nz\n", crlf=True)
    w("enc.tsv", "", raw=b"# sheet: enc\n# version: 1\nid:string*\n\xff\xfe\x00z\n")
    w("notab.tsv", MANIFEST.format(name="notab", ver=1, extra="") + "id:string*\nz\n".replace("\t", " "))
    w("badheader.tsv", MANIFEST.format(name="badheader", ver=1, extra="") + "idstring\tok:string\nz\ty\n")
    w("noheader.tsv", MANIFEST.format(name="noheader", ver=1, extra="") + "# only a comment\n")
    w("nomanifest.tsv", "# version: 1\nid:string*\nz\n")
    w("emitterver.tsv", MANIFEST.format(name="emitterver", ver=99, extra="") + "id:string*\nz\n")
    # E-L0-KEY needs an empty primary key in a row that is not otherwise blank.
    # A line consisting only of a tab is treated as blank (MDD 5.2: "fully blank
    # lines ignored"), so the row needs a non-empty second cell to be parsed
    # at all - which is a real property of the reader worth knowing.
    w("badkey.tsv", MANIFEST.format(name="badkey", ver=1, extra="") + "id:string*\tnote:string\n\thas no key\n")
    w("badkey2.tsv", MANIFEST.format(name="badkey2", ver=1, extra="") + "id:string*\nBADKEY\n")
    w("dupkey.tsv", MANIFEST.format(name="dupkey", ver=1, extra="") + "id:string*\nsame\nsame\n")
    w("wide.tsv", MANIFEST.format(name="wide", ver=1, extra="") + "id:string*\ta:string\nz\t1\t2\t3\n")
    # A short row: fewer cells than the header declares. dec004 allows omitting
    # trailing empties, but the reader says so (W-L0-SHORT) because a truncated
    # row is also what a broken writer produces.
    w("short.tsv", MANIFEST.format(name="short", ver=1, extra="") + "id:string*\ta:string\tb:string\nz\t1\n")

    # --- types (L1)
    w("badtype.tsv", MANIFEST.format(name="badtype", ver=1, extra="") + "id:string*\tx:notatype\nz\ty\n")
    w("badvalue.tsv", MANIFEST.format(name="badvalue", ver=1, extra="") + "id:string*\tn:u16\nz\tabc\n")
    w("quoted.tsv", MANIFEST.format(name="quoted", ver=1, extra="") + 'id:string*\tnote:string\nz\t"quoted cell"\n')

    # --- references (L2)
    w("tgt.tsv", MANIFEST.format(name="tgt", ver=1, extra="") + "id:string*\nknown\n")
    w("badref.tsv", MANIFEST.format(name="badref", ver=1, extra="") + "id:string*\tref:string -> tgt.id\tlost:string -> ghost.id\nz\tnope\tnope\n")
    w("noev.tsv", MANIFEST.format(name="noev", ver=1, extra="# evidence: required\n") +
      "id:string*\tstatus:string\tref_addr:string\tref_conf:string\tevidence:string\nz\ttodo\t-\t-\t-\n")
    # a cycle: two sheets requiring each other
    w("cyc_a.tsv", MANIFEST.format(name="cyc_a", ver=1, extra="").replace("# requires: -", "# requires: cyc_b") + "id:string*\na\n")
    w("cyc_b.tsv", MANIFEST.format(name="cyc_b", ver=1, extra="").replace("# requires: -", "# requires: cyc_a") + "id:string*\nb\n")

    # --- L1 rules that need a second sheet for the unit clash
    w("unit_a.tsv", MANIFEST.format(name="unit_a", ver=1, extra="") + "id:string*\tspeed:f32 m/s\nz\t1.5\n")
    w("unit_b.tsv", MANIFEST.format(name="unit_b", ver=1, extra="") + "id:string*\tspeed:f32 kph\nz\t1.5\n")
    w("defbad.tsv", MANIFEST.format(name="defbad", ver=1, extra="") + "id:string*\treload:f32 s =abc\nz\t\n")
    w("empty.tsv", MANIFEST.format(name="empty", ver=1, extra="") + "id:string*\tneed:string\nz\t\n")

    # --- provenance (L5) and budgets (L6)
    w("unbalanced.tsv", MANIFEST.format(name="unbalanced", ver=1, extra="# source_rows: 99\n# dropped: 0\n# dropped_reason: -\n")
      + "id:string*\nr1\nr2\n")
    w("small.tsv", MANIFEST.format(name="small", ver=1, extra="") + "id:string*\nr1\nr2\nr3\n")
    # v_over must name only real columns, or build_pack rejects it as a bad view
    # (W-L6-VIEW) instead of measuring it and going over budget (E-L6-BUDGET).
    # Rows are joined from lists so a mistyped separator cannot silently split a
    # row across two lines - which is what happened the first time this was
    # written, and produced five phantom views named 'tester'.
    def row(*cells: str) -> str:
        return "\t".join(cells) + "\n"

    w("views.tsv", MANIFEST.format(name="views", ver=1, extra="")
      + row("id:string*", "sheet:string", "columns:string", "row_window:string",
            "token_budget:u32", "used_by:string", "status:string")
      + row("v_over", "small", "id", "*", "1", "tester", "done")
      + row("v_ghostsheet", "ghost", "id", "*", "1000", "tester", "done")
      + row("v_ghostcol", "small", "id,nosuchcol", "*", "1000", "tester", "done"))

    # --- kernel allowlist (L0 check 21): a module on disk that is not listed
    kdir = os.path.join(SWEEP, "kernel")
    os.makedirs(kdir, exist_ok=True)
    with open(os.path.join(kdir, "probe.rs"), "w", encoding="utf-8", newline="\n") as f:
        f.write("// an unlisted hand-written kernel module; E-L0-KERNEL must fire\n")
    w("kernel.tsv", MANIFEST.format(name="kernel", ver=1, extra="") + "id:string*\tkind:string\trust_module:string\tstatus:string\ttests:string\tnotes:string?\n")

    # --- generated file in the tree (L0 check 22) is NOT written here on purpose.
    # The rule scans outward from the sheets directory, so a persisted banner file
    # would make the real book fail preflight - correctly, since a generated file
    # in the working tree is exactly what D6 forbids. The sweep test creates it,
    # asserts the finding, and removes it.

    n = sum(len(files) for _, _, files in os.walk(SHEETS))
    print(f"wrote {n} fixture sheet(s) under {os.path.relpath(SHEETS, ROOT)}")
    print(f"plus tests/sweep/kernel/probe.rs (E-L0-KERNEL)")
    print(f"the E-L0-GENERATED probe is created and removed by tools/test-rules-sweep.ps1")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
