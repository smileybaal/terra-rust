#!/usr/bin/env python3
"""Generate sheets/re/server/localization.tsv from the game's localization JSON.

The sheets carry no strings (dec015: the server has no strings sheet), so every
protocol string in the port was hand-cited from the C# and from the game's
localization table. That table ships with the export and is machine-readable, so it
can be a relation like any other, and then the port reads its strings from the book
instead of repeating them.

This is the CLI section first (the server's interactive surface: its prompts, its
console commands and its runtime messages). Later sections (Net.*, the
LegacyMultiplayer.* table that Lang.mp[n] indexes) join it the same way; the
provenance block records how many leaves are not yet carried, so the omission is
visible in the sheet rather than silent.

Contract, from the sibling managed sheets:
  - UTF-8 without BOM, LF line endings, TAB separated (D7)
  - a `# key: value` manifest, a header row, then data
  - `id_form: compound`, because the ids are dotted key paths (dec009)
  - `source_rows` == rows + `dropped`, so provenance balances (dec008 drops text
    containing a TAB or a newline, which the canonical form forbids)

Usage:
    python tools/gen-localization.py            # write the sheet
    python tools/gen-localization.py --check    # report, write nothing
"""
from __future__ import annotations

import argparse
import io
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "sheets" / "re" / "server" / "localization.tsv"

# The sections this producer carries, per artifact. This is the protocol surface the
# port needs: the CLI console and its prompts, the `Net.*` status and kick texts, and
# the `LegacyMultiplayer` table that `Lang.mp[n]` indexes (`mp[1]` is "Incorrect
# password", `mp[4]` is the version mismatch). Everything else in these files is
# counted in `dropped`, so the sheet cannot look complete while it is not.
SOURCES = [
    ("ilspy_server/Terraria.Localization.Content.en-US.json", ("CLI", "Net")),
    ("ilspy_server/Terraria.Localization.Content.en-US.Legacy.json", ("LegacyMultiplayer",)),
]

COLUMNS = [
    ("id", "string*"),
    ("key", "string"),
    ("section", "string"),
    ("text", "string"),
    ("length", "u16"),
    ("status", "string"),
    ("artifact", "string"),
    ("ref_addr", "string"),
    ("ref_conf", "string"),
    ("evidence", "string"),
]


def load_json(path: Path) -> dict:
    """Parse the bundled localization file.

    The files are JSON.NET output and carry trailing commas, which strict JSON
    forbids, so they are stripped before parsing rather than pulling in a second
    parser.
    """
    text = path.read_text(encoding="utf-8-sig")
    text = re.sub(r",(\s*[}\]])", r"\1", text)
    return json.loads(text)


def walk(node, path: str = ""):
    """Yield (dotted_key, json_pointer, text) for every leaf string."""
    if isinstance(node, dict):
        for k, v in node.items():
            yield from walk(v, f"{path}/{k}")
    elif isinstance(node, str):
        yield path.strip("/").replace("/", "."), path, node
    elif isinstance(node, list):
        for i, v in enumerate(node):
            yield from walk(v, f"{path}/{i}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="report only, write nothing")
    args = ap.parse_args()

    leaves = []
    for rel, _sections in SOURCES:
        path = ROOT / "re" / "exports" / rel
        if not path.exists():
            print(f"missing artifact: {path}", file=sys.stderr)
            return 2
        for key, pointer, text in walk(load_json(path)):
            leaves.append((rel, key, pointer, text))
    source_rows = len(leaves)

    wanted = {rel: set(sections) for rel, sections in SOURCES}
    kept, dropped = [], 0
    for rel, key, pointer, text in leaves:
        section = key.split(".")[0]
        if section not in wanted[rel]:
            continue
        # dec008: the canonical TSV form forbids unescaped tabs and newlines.
        if "\t" in text or "\n" in text or "\r" in text:
            dropped += 1
            continue
        kept.append((key.lower(), key, section, text, pointer, rel))

    kept.sort()
    ids = [r[0] for r in kept]
    dupes = {i for i in ids if ids.count(i) > 1}
    if dupes:
        print(f"refusing to write: duplicate ids {sorted(dupes)}", file=sys.stderr)
        return 2

    # Everything not carried by this run, so the omission is counted rather than
    # implied.
    not_yet = source_rows - len(kept) - dropped

    manifest = [
        "# sheet: re/server/localization",
        "# version: 1",
        "# generator: localization",
        "# target: sheets/re/server",
        "# index: by_id",
        "# requires: re/sources",
        "# owned_by: extractor",
        "# doctrine: D1",
        "# evidence: required",
        "# emit: rust",
        "# Localization keys for the server binary, from the game's own table.",
        "# The sheets carry no strings (dec015), so this relation is what lets the",
        "# port READ its protocol and console text instead of repeating it. `key` is",
        "# the case-sensitive key exactly as the C# passes it to",
        "# Language.GetTextValue, and `text` is the en-US value.",
        "# `section` is the first dotted segment of the key, so a later run can carry",
        "# another section without restructuring the sheet.",
        "# `length` is the UTF-8 byte length of `text`.",
        "# `ref_addr` is the JSON pointer of the leaf inside the artifact.",
        f"# Only {', '.join(sorted({s for _, secs in SOURCES for s in secs}))} are carried so far; the rest are counted in",
        "# `dropped` below, so the sheet cannot look complete while it is not.",
        "# Keys are dotted paths, so the compound-key exception is declared (dec009).",
        "# id_form: compound",
        f"# source_rows: {source_rows}",
        f"# dropped: {source_rows - len(kept)}",
        f"# dropped_reason: {dropped} leaf/leaves contain a TAB or newline and cannot be represented (dec008); {not_yet} belong to sections this run does not carry",
    ]

    header = "\t".join(f"{n}:{t}" for n, t in COLUMNS)
    lines = [*manifest, header]
    for ident, key, section, text, pointer, rel in kept:
        cells = [
            ident,
            key,
            section,
            text,
            str(len(text.encode("utf-8"))),
            "identified",
            rel,
            pointer,
            "certain",
            "localization table leaf",
        ]
        if len(cells) != len(COLUMNS):
            print(f"internal error: {len(cells)} cells for {len(COLUMNS)} columns", file=sys.stderr)
            return 2
        lines.append("\t".join(cells))

    body = "\n".join(lines) + "\n"
    if args.check:
        print(f"would write {len(kept)} rows to {OUT.relative_to(ROOT)}")
        print(f"source leaves {source_rows}, dropped {source_rows - len(kept)} "
              f"({dropped} unrepresentable, {not_yet} other sections)")
        return 0

    # D7: UTF-8, no BOM, LF.
    with io.open(OUT, "w", encoding="utf-8", newline="") as f:
        f.write(body)
    print(f"wrote {OUT.relative_to(ROOT)}: {len(kept)} rows, "
          f"dropped {source_rows - len(kept)} of {source_rows} leaves")
    return 0


if __name__ == "__main__":
    sys.exit(main())
