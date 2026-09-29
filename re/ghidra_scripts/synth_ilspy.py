#!/usr/bin/env python3
"""synth_ilspy.py: ILSpy exports -> canonical evidence sheets.

THE SPREADSHEET METHOD, Mode B (MDD 8.6). Decompiled output is EVIDENCE, never
source. This script is the [normalize] stage: it reads the raw producer output
and writes canonical TSV into sheets/re/.

Inputs (all under re/exports/):
  ilspy_entities_{c,s,e,i,d}.txt   entity inventories ("Class Terraria.Foo")
  ilspy/**/*.cs                    the decompiled project tree (one file per type)

Outputs:
  sheets/re/types.tsv
  sheets/re/methods.tsv

Canonical form (MDD 5.2): UTF-8 no BOM, LF, TAB delimited. Rows are sorted by
id. Every row carries ref_addr + ref_conf + evidence (MDD 8.9 / O16).
"""
from __future__ import annotations

import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
EXPORTS = os.path.join(ROOT, "re", "exports")

# ---------------------------------------------------------------------------
# canonical writers
# ---------------------------------------------------------------------------

def manifest(sheet: str, generator: str, target: str, requires: str, doctrine: str,
             evidence: str = "required", extra: str = "") -> str:
    lines = [
        f"# sheet: {sheet}",
        "# version: 1",
        f"# generator: {generator}",
        f"# target: {target}",
        "# index: by_id",
        f"# requires: {requires}",
        "# owned_by: extractor",
        f"# doctrine: {doctrine}",
        f"# evidence: {evidence}",
    ]
    if extra:
        lines.append(extra)
    return "\n".join(lines) + "\n"


def write_tsv(path: str, header: str, rows: list[list[str]]) -> int:
    by_id: dict[str, list[str]] = {}
    for r in rows:
        by_id[r[0]] = r
    ordered = [by_id[k] for k in sorted(by_id)]
    out = []
    for r in ordered:
        cells = [sanitize(c) for c in r]
        while len(cells) > 1 and cells[-1] == "":
            cells.pop()
        out.append("\t".join(cells))
    body = "\n".join(out)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(header)
        if body:
            f.write(body + "\n")
    return len(ordered)


def sanitize(cell: str) -> str:
    """Cells must not contain TAB or newline (MDD 5.2)."""
    return cell.replace("\t", " ").replace("\r", " ").replace("\n", " ")


# ---------------------------------------------------------------------------
# entity inventory
# ---------------------------------------------------------------------------

ENTITY_KIND = {
    "c": "class",
    "s": "struct",
    "e": "enum",
    "i": "interface",
    "d": "delegate",
}


def read_entities() -> list[tuple[str, str]]:
    out: list[tuple[str, str]] = []
    for key, kind in ENTITY_KIND.items():
        p = os.path.join(EXPORTS, f"ilspy_entities_{key}.txt")
        if not os.path.exists(p):
            continue
        with open(p, encoding="utf-8", errors="replace") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                m = re.match(r"^(Class|Struct|Enum|Interface|Delegate)\s+(.*)$", line)
                if not m:
                    continue
                out.append((kind, m.group(2).strip()))
    return out


def split_ns(full: str) -> tuple[str, str]:
    """Terraria.GameContent.Foo.Bar -> ('Terraria.GameContent.Foo', 'Bar')."""
    if "." not in full:
        return ("", full)
    ns, _, name = full.rpartition(".")
    return (ns, name)


# ---------------------------------------------------------------------------
# decompiled tree
# ---------------------------------------------------------------------------

TYPE_START = re.compile(
    r"^\s*(?:\[[^\]]*\]\s*)*"                       # attributes
    r"(?:public|internal|private|protected|file)\s+"  # accessibility
    r"(?:static\s+|sealed\s+|abstract\s+|partial\s+|readonly\s+|ref\s+|unsafe\s+|new\s+)*"
    r"(class|struct|enum|interface)\s+([A-Za-z_][A-Za-z0-9_]*|@[A-Za-z_][A-Za-z0-9_]*)"
)
# a delegate's return type precedes its name: `public delegate void Foo(...)`
DELEGATE_START = re.compile(
    r"^\s*(?:\[[^\]]*\]\s*)*"
    r"(?:public|internal|private|protected)\s+"
    r"delegate\s+"
    r"(?:ref\s+|unsafe\s+|readonly\s+)*"
    r"[\w<>\[\],\.\?@]+\s+"
    r"([A-Za-z_][A-Za-z0-9_]*|@[A-Za-z_][A-Za-z0-9_]*)"
)
METHOD_START = re.compile(
    r"^\s*(?:\[[^\]]*\]\s*)*"
    r"(?:public|internal|private|protected)\s+"
    r"(?:static\s+|virtual\s+|override\s+|sealed\s+|abstract\s+|extern\s+|unsafe\s+|new\s+|async\s+|partial\s+|readonly\s+)*"
    r"(?:[\w<>\[\],\.\?@\s]+?)\s+"
    r"([A-Za-z_][A-Za-z0-9_]*|@[A-Za-z_][A-Za-z0-9_]*|operator\s*[^\s(]+|this)\s*\("
)
CTOR_START = re.compile(r"^\s*(?:public|internal|private|protected)\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(")
FIELD_START = re.compile(
    r"^\s*(?:\[[^\]]*\]\s*)*(?:public|internal|private|protected)\s+"
    r"(?:static\s+|readonly\s+|volatile\s+|const\s+|new\s+|unsafe\s+|fixed\s+)*"
    r"[\w<>\[\],\.\?@]+(?:<[^>]*>)?\s+"
    r"([A-Za-z_][A-Za-z0-9_]*)\s*(?:=|;)"
)
PROP_START = re.compile(
    r"^\s*(?:\[[^\]]*\]\s*)*(?:public|internal|private|protected)\s+"
    r"(?:static\s+|virtual\s+|override\s+|sealed\s+|abstract\s+|new\s+|unsafe\s+|readonly\s+)*"
    r"[\w<>\[\],\.\?@]+(?:<[^>]*>)?\s+"
    r"([A-Za-z_][A-Za-z0-9_]*)\s*\{\s*(?:get|set)"
)


OP_SYMBOL = {
    "+": "plus", "-": "minus", "!": "bang", "~": "tilde", "++": "plusplus",
    "--": "minusminus", "*": "star", "/": "slash", "%": "percent", "&": "amp",
    "|": "pipe", "^": "caret", "<<": "lshift", ">>": "rshift", "==": "eq",
    "!=": "ne", "<": "lt", ">": "gt", "<=": "le", ">=": "ge", "true": "true",
    "false": "false",
}


def op_id(text: str) -> str:
    """Map `Type::operator !=` to `Type::op_ne`, and conversions to op_<type>.

    Keeps operator identity inside the id instead of discarding it, so distinct
    operators do not fold into one name.
    """
    if "::operator " not in text:
        return text
    head, _, rest = text.partition("::operator ")
    rest = rest.strip()
    if rest in OP_SYMBOL:
        return f"{head}::op_{OP_SYMBOL[rest]}"
    ty = rest.split("(")[0].strip()
    return f"{head}::op_{ty}"


def type_decl(line: str):
    """Return (kind, name) if the line declares a type, else None."""
    m = TYPE_START.match(line)
    if m:
        return (m.group(1), m.group(2))
    d = DELEGATE_START.match(line)
    if d:
        return ("delegate", d.group(1))
    return None


RESERVED_WORDS = {
    "if", "for", "while", "switch", "foreach", "return", "new", "using", "lock",
    "catch", "else", "do", "try", "throw", "fixed", "unchecked", "checked", "base",
    "this", "await", "yield", "goto", "case", "default", "sizeof", "typeof",
}


def cs_files() -> list[str]:
    out = []
    base = os.path.join(EXPORTS, "ilspy")
    for dirpath, _, names in os.walk(base):
        for n in names:
            if n.endswith(".cs"):
                out.append(os.path.join(dirpath, n))
    return sorted(out)


def rel_artifact(path: str) -> str:
    return "ilspy/" + os.path.relpath(path, os.path.join(EXPORTS, "ilspy")).replace("\\", "/")


# ---------------------------------------------------------------------------
# main scan
# ---------------------------------------------------------------------------

class TypeReader:
    """Extract type declarations with their fields, properties and methods from a
    single ILSpy file. A file can hold more than one type (nested or sibling);
    each concrete declaration is scanned independently by restarting at it."""

    def __init__(self, text: str):
        self.lines = text.split("\n")
        self.ns = ""
        self.types: list[dict] = []
        self._scan()

    def _scan(self) -> None:
        for ln in self.lines:
            m = re.match(r"^\s*namespace\s+([\w\.]+)", ln)
            if m:
                self.ns = m.group(1)
                break

        i = 0
        n = len(self.lines)
        while i < n:
            decl = type_decl(self.lines[i])
            if decl:
                t, end = self._read_type(i, decl[0], decl[1])
                self.types.append(t)
                i = i + 1 if end <= i else end
                continue
            i += 1

    def _read_type(self, start: int, decl_kind: str, decl_name: str) -> tuple[dict, int]:
        """Return (type record, index just past this type's closing brace)."""
        decl_name = decl_name.lstrip("@")
        # constructors are named after the type; the accurate name is the one in
        # the declaration, so reset it even if a previous match grabbed something
        t = {"kind": decl_kind, "name": decl_name, "fields": 0, "props": 0, "methods": []}
        depth = 0
        began = False
        i = start
        n = len(self.lines)
        while i < n:
            ln = self.lines[i]
            if not began:
                if "{" in ln:
                    began = True
                    depth = ln.count("{") - ln.count("}")
                    if depth <= 0:
                        return t, i + 1
                i += 1
                continue
            depth += ln.count("{") - ln.count("}")
            if depth <= 0:
                return t, i + 1
            stripped = ln.strip()
            if stripped and not stripped.startswith("//") and not stripped.startswith("/*"):
                if type_decl(ln):
                    i += 1
                    continue
                if FIELD_START.match(ln):
                    t["fields"] += 1
                elif PROP_START.match(ln):
                    t["props"] += 1
                elif not stripped.startswith("["):
                    mc = CTOR_START.match(ln)
                    if mc and mc.group(1) == decl_name:
                        t["methods"].append((".ctor", f"{decl_name}()", "ctor"))
                    else:
                        mm = METHOD_START.match(ln)
                        if mm:
                            name = mm.group(1).lstrip("@")
                            if name not in RESERVED_WORDS:
                                if name == "this":
                                    name = "this[]"
                                kind = "accessor" if name.startswith(("get_", "set_")) else "method"
                                t["methods"].append((name, stripped.split("{")[0].strip(), kind))
            i += 1
        return t, i


def scan() -> tuple[list[list[str]], list[list[str]], dict]:
    entities = read_entities()
    ent_kind = {full: kind for kind, full in entities}
    ent_ns = {full: split_ns(full)[0] for _, full in entities}

    types_rows: list[list[str]] = []
    methods_rows: list[list[str]] = []
    seen_types: set[str] = set()
    seen_method_ids: dict[str, int] = {}
    seen_type_ids: dict[str, int] = {}
    stats = {"files": 0, "skipped": 0, "methods": 0, "fields": 0, "ctors": 0, "props": 0, "multi": 0}

    for path in cs_files():
        try:
            with open(path, encoding="utf-8", errors="replace") as f:
                text = f.read()
        except OSError:
            stats["skipped"] += 1
            continue
        stats["files"] += 1
        artifact = rel_artifact(path)
        reader = TypeReader(text)
        if not reader.types:
            stats["skipped"] += 1
            continue
        if len(reader.types) > 1:
            stats["multi"] += 1

        for t in reader.types:
            decl_name = t["name"]
            ns = reader.ns
            full = f"{ns}.{decl_name}" if ns else decl_name
            kind = ent_kind.get(full, t["kind"])
            seen_types.add(full)
            stats["fields"] += t["fields"]
            stats["props"] += t["props"]
            stats["methods"] += len(t["methods"])

            # the inventory is authoritative for the namespace: file layout can
            # put a global-namespace type in a nested directory (e.g. the
            # nativefiledialog.cs shim lands under Terraria/), so prefer it
            ns_final = ent_ns.get(full, ns) or ns or "-"
            # D7 requires lowercase ids; the original casing is preserved in the
            # name and namespace columns, so nothing is lost. A monotonic
            # suffix guarantees uniqueness after folding (preflight check 6).
            tid = full.lower()
            uniq_t = seen_type_ids.get(tid, 0)
            seen_type_ids[tid] = uniq_t + 1
            if uniq_t:
                tid = f"{tid}_{uniq_t}"
            types_rows.append([
                tid, kind, ns_final or "-", decl_name, str(t["fields"]), "-", "todo",
                artifact, f"ilspy:{artifact}", "certain", f"ilspycmd {artifact}",
            ])

            counts: dict[str, int] = {}
            for name, sig, mkind in t["methods"]:
                if mkind == "ctor":
                    stats["ctors"] += 1
                key = f"{full}::{name}"
                n = counts.get(key, 0)
                counts[key] = n + 1
                mid_raw = key if n == 0 else f"{key}#{n}"
                # id charset must satisfy D7. C# operator names carry spaces and
                # symbols, so map each operator token to a stable word rather
                # than dropping it, then fold anything outside [a-z0-9_.] to '_'.
                # A monotonic per-id suffix guarantees uniqueness after folding
                # (preflight check 6: no duplicate keys).
                mid = op_id(mid_raw).lower().replace("::", ".")
                mid = re.sub(r"[^a-z0-9_.]+", "_", mid).strip("_")
                uniq = seen_method_ids.get(mid, 0)
                seen_method_ids[mid] = uniq + 1
                if uniq:
                    mid = re.sub(r"[^a-z0-9_.]+", "_", f"{mid}_{uniq}").strip("_")
                methods_rows.append([
                    mid, full, name, sig, mkind, "-", "todo", artifact,
                    f"ilspy:{artifact}", "certain", f"ilspycmd {artifact}",
                ])

    stats["types"] = len(types_rows)
    stats["unmatched_types"] = len(set(ent_kind) - seen_types)
    return types_rows, methods_rows, stats


def main() -> int:
    if not os.path.isdir(EXPORTS):
        print(f"no exports dir at {EXPORTS}", file=sys.stderr)
        return 1
    types_rows, methods_rows, stats = scan()

    tpath = os.path.join(ROOT, "sheets", "re", "types.tsv")
    mpath = os.path.join(ROOT, "sheets", "re", "methods.tsv")

    theader = manifest(
        "re/types", "types", "sheets/re", "re/sources", "D1",
        extra="# Managed type inventory from ILSpy. kind is class|struct|enum|interface|delegate.\n# Type ids are dotted namespace paths, so the compound-key exception is declared.\n# id_form: compound",
    ) + "id:string*\tkind:string\tnamespace:string\tname:string\tfields:u16\tbase:string\tstatus:string\tartifact:string\tref_addr:string\tref_conf:string\tevidence:string\n"

    mheader = manifest(
        "re/methods", "methods", "sheets/re", "re/types,re/sources", "D1",
        extra="# Managed method inventory from ILSpy. ids are lowercased dotted paths;\n# the compound key joins type and method with '.', so id_form is declared.\n# id_form: compound",
    ) + "id:string*\ttype:string\tname:string\tsignature:string\tkind:string\til_offset:string\tstatus:string\tartifact:string\tref_addr:string\tref_conf:string\tevidence:string\n"

    nt = write_tsv(tpath, theader, types_rows)
    nm = write_tsv(mpath, mheader, methods_rows)

    print(f"files scanned      {stats['files']}")
    print(f"types              {nt}")
    print(f"methods            {nm}")
    print(f"fields (counted)   {stats['fields']}")
    print(f"ctors              {stats['ctors']}")
    print(f"properties         {stats['props']}")
    print(f"multi-type files   {stats['multi']}")
    print(f"unmatched entities {stats['unmatched_types']} (in inventory, no file matched)")
    print(f"skipped files      {stats['skipped']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
