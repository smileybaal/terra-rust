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

import json
import math
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
EXPORTS = os.path.join(ROOT, "re", "exports")

# The two managed subjects. They get completely separate sheet sets rather than a
# shared set plus a delta: the client and the server are different binaries, so
# they are different relations, and a relation per binary is what keeps a query
# like "which types does the server have and the client not" a plain overlap
# instead of a hand-maintained list that can rot.
TARGETS = [
    {
        "label": "client",
        "prefix": "ilspy",            # re/exports/ilspy_entities_*.txt
        "tree": "ilspy",              # re/exports/ilspy/**/*.cs
        "base": "re/client",          # sheet names: re/client/types, ...
        "out": os.path.join(ROOT, "sheets", "re", "client"),
    },
    {
        "label": "server",
        "prefix": "ilspy_server",
        "tree": "ilspy_server",
        "base": "re/server",
        "out": os.path.join(ROOT, "sheets", "re", "server"),
    },
]

# Set per target by main() before scanning.
PREFIX = TARGETS[0]["prefix"]
TREE = TARGETS[0]["tree"]

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
        p = os.path.join(EXPORTS, f"{PREFIX}_entities_{key}.txt")
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
    base = os.path.join(EXPORTS, TREE)
    for dirpath, _, names in os.walk(base):
        for n in names:
            if n.endswith(".cs"):
                out.append(os.path.join(dirpath, n))
    return sorted(out)


def rel_artifact(path: str) -> str:
    return f"{TREE}/" + os.path.relpath(path, os.path.join(EXPORTS, TREE)).replace("\\", "/")


def ghidra_edge_count():
    """Read the Ghidra call-graph measurement so the sheet states a measured
    number rather than a remembered one (MDD instrumentation rule: never
    hand-maintain a number that can be derived)."""
    p = os.path.join(EXPORTS, "ghidra", "callgraph.json")
    if not os.path.exists(p):
        return None, None
    raw = open(p, encoding="utf-8", errors="replace").read()
    starts = [i for i in (raw.find("["), raw.find("{")) if i >= 0]
    if not starts:
        return None, None
    try:
        d = json.loads(raw[min(starts):])
    except json.JSONDecodeError:
        return None, None
    if isinstance(d, list) and d and isinstance(d[0], dict):
        return d[0].get("edge_count"), d[0].get("node_count")
    return None, None


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
        t = {"kind": decl_kind, "name": decl_name, "fields": 0, "props": 0,
             "methods": [], "bases": [], "sigtext": []}

        # base types and interfaces sit on the declaration line: `: Bar, IBaz`
        decl_line = self.lines[start]
        idx = decl_line.find(decl_name)
        rest = decl_line[idx + len(decl_name):] if idx >= 0 else ""
        if ":" in rest:
            basepart = re.sub(r"<[^>]*>", "", rest.split(":", 1)[1].split("{")[0])
            for b in basepart.split(","):
                b = b.strip().split(".")[-1].strip()
                if b and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", b):
                    t["bases"].append(b)
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
                                sig = stripped.split("{")[0].strip()
                                t["methods"].append((name, sig, kind))
                                t["sigtext"].append(sig)
            i += 1
        return t, i


def scan() -> tuple[list[list[str]], list[list[str]], dict]:
    entities = read_entities()
    ent_kind = {full: kind for kind, full in entities}
    ent_ns = {full: split_ns(full)[0] for _, full in entities}

    types_rows: list[list[str]] = []
    methods_rows: list[list[str]] = []
    records: list[dict] = []
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
                tid, kind, ns_final or "-", decl_name, str(t["fields"]),
                ",".join(t["bases"]) if t["bases"] else "-", "todo",
                artifact, f"ilspy:{artifact}", "certain", f"ilspycmd {artifact}",
            ])
            records.append({
                "tid": tid, "full": full, "fields": t["fields"],
                "methods": t["methods"], "bases": t["bases"],
                "sigtext": t["sigtext"], "artifact": artifact,
            })

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

    # -----------------------------------------------------------------------
    # managed type-dependency edges -> re/callgraph.tsv
    #
    # dec013: Ghidra decoded 0 instructions, so it yields no edges at all
    # (`ghidra graph calls` reports edge_count=0). The managed dependency graph
    # is therefore derived from the decompiled C# declarations instead: a base
    # type or interface is `inherits`, a type named in a method signature is
    # `uses`. Only unambiguous short names resolve; a name shared by several
    # types is skipped rather than guessed at.
    # -----------------------------------------------------------------------
    short_map: dict[str, str] = {}
    ambiguous: set[str] = set()
    for _k, full in entities:
        short = split_ns(full)[1]
        if short in short_map and short_map[short] != full:
            ambiguous.add(short)
        else:
            short_map[short] = full
    for a in ambiguous:
        short_map.pop(a, None)
    stats["ambiguous_short_names"] = len(ambiguous)

    id_by_full = {r["full"]: r["tid"] for r in records}
    edges: dict[str, list[str]] = {}
    edge_candidates = 0
    for rec in records:
        toks = set(rec["bases"])
        for s in rec["sigtext"]:
            toks.update(re.findall(r"[A-Za-z_][A-Za-z0-9_]*", s))
        for tok in toks:
            tgt_full = short_map.get(tok)
            if not tgt_full or tgt_full == rec["full"]:
                continue
            tgt = id_by_full.get(tgt_full)
            if not tgt:
                continue
            edge_candidates += 1
            kind = "inherits" if tok in rec["bases"] else "uses"
            eid = re.sub(r"[^a-z0-9_.]+", "_", f"{rec['tid']}__{tgt}__{kind}").strip("_")
            edges[eid] = [
                eid, rec["tid"], tgt, kind, "identified",
                f"ilspy:{rec['artifact']}", "probable",
                f"ilspycmd C# declaration references {tok}",
            ]
    callgraph_rows = list(edges.values())
    stats["edges"] = len(callgraph_rows)
    stats["edge_candidates"] = edge_candidates
    stats["entity_count"] = len(entities)
    stats["records"] = len(records)
    # the writers fold duplicate ids, so "rows written" is the unique-id count,
    # not the length of the row list. Using the list length would understate
    # `dropped` and make the provenance balance check fail for the wrong reason.
    stats["uniq_types"] = len({r[0] for r in types_rows})
    stats["uniq_methods"] = len({r[0] for r in methods_rows})
    stats["uniq_edges"] = len({r[0] for r in callgraph_rows})

    # -----------------------------------------------------------------------
    # triage from measured managed evidence -> re/triage.tsv
    #
    # MDD 8.5 scores a function by log(size) and xrefs. Neither is available
    # here: Ghidra decoded no code (dec013), so its `size` is a metadata record
    # length and its xrefs are all zero. Scoring those would be scoring nothing.
    # Fields and methods per type ARE measured exactly from the decompiled
    # source, so they are the honest size proxy.
    # -----------------------------------------------------------------------
    triage_rows: list[list[str]] = []
    for rec in records:
        f = rec["fields"]
        m = len(rec["methods"])
        score = 2.0 * math.log2(1 + f) + 3.0 * math.log2(1 + m)
        prio = 1 if score >= 12 else (2 if score >= 8 else 3)
        triage_rows.append([
            rec["tid"], f"fields={f};methods={m}", f"{score:.2f}", str(prio), "todo",
            f"ilspy:{rec['artifact']}", "probable",
            f"ILSpy measured fields={f} methods={m}; size proxy, because Ghidra size is unusable (dec013)",
        ])
    stats["triage"] = len(triage_rows)
    stats["uniq_triage"] = len({r[0] for r in triage_rows})

    return types_rows, methods_rows, triage_rows, callgraph_rows, stats


def emit_target(t: dict) -> int:
    """Produce the four managed sheets for one binary into its own directory."""
    global PREFIX, TREE
    PREFIX = t["prefix"]
    TREE = t["tree"]
    base = t["base"]
    out = t["out"]
    label = t["label"]

    tree_dir = os.path.join(EXPORTS, TREE)
    if not os.path.isdir(tree_dir):
        print(f"{label:7} SKIPPED: no decompiled tree at re/exports/{TREE}")
        return 0

    other = next((x["base"] for x in TARGETS if x["label"] != label), base)

    os.makedirs(out, exist_ok=True)
    types_rows, methods_rows, triage_rows, callgraph_rows, stats = scan()

    types_name = f"{base}/types"
    methods_name = f"{base}/methods"
    graph_name = f"{base}/callgraph"
    triage_name = f"{base}/triage"
    target_dir = f"sheets/re/{label}"

    theader = manifest(
        types_name, "types", target_dir, "re/sources", "D1",
        extra=(
            f"# Managed type inventory for the {label} binary, from ILSpy.\n"
            "# kind is class|struct|enum|interface|delegate.\n"
            "# Type ids are dotted namespace paths, so the compound-key exception is declared.\n"
            "# emit: rust\n"
            "# id_form: compound\n"
            f"# key_alias: {other}/types,{other}/triage\n"
            "# The two platform inventories are DIFFERENT RELATIONS OVER THE SAME TYPE IDS.\n"
            "# That is the point: overlapping them is how client-only and server-only types\n"
            "# are found, so check 13 is exempted rather than the ids being namespaced apart.\n"
            f"# source_rows: {stats['entity_count']}\n"
            f"# dropped: {stats['entity_count'] - stats['uniq_types']}\n"
            "# dropped_reason: compiler-generated types have no emitted .cs file (dec010); resource "
            "shims were also absent from the inventory comparison"
        ),
    ) + "id:string*\tkind:string\tnamespace:string\tname:string\tfields:u16\tbase:string\tstatus:string\tartifact:string\tref_addr:string\tref_conf:string\tevidence:string\n"

    mheader = manifest(
        methods_name, "methods", target_dir, f"{types_name},re/sources", "D1",
        extra=(
            f"# Managed method inventory for the {label} binary, from ILSpy.\n"
            "# ids are lowercased dotted paths; the compound key joins type and\n"
            "# method with '.', so id_form is declared.\n"
            "# id_form: compound\n"
            f"# key_alias: {other}/methods\n"
            "# Shares its key space with the other platform's method inventory by design.\n"
            f"# source_rows: {stats['methods']}\n"
            f"# dropped: {stats['methods'] - stats['uniq_methods']}\n"
            "# dropped_reason: -"
        ),
    ) + "id:string*\ttype:string\tname:string\tsignature:string\tkind:string\til_offset:string\tstatus:string\tartifact:string\tref_addr:string\tref_conf:string\tevidence:string\n"

    gec, gnc = ghidra_edge_count()
    gheader = manifest(
        graph_name, "callgraph_managed", target_dir, f"{types_name}", "D1",
        extra=(
            "# Edges. Cycles here are informative, not errors: SCCs are usually subsystems.\n"
            "# PROVENANCE: Ghidra contributed none of these. `ghidra graph calls` returned\n"
            f"# edge_count={gec} over node_count={gnc} because Ghidra decoded 0 instructions\n"
            "# for this managed binary (dec013). The edges are derived instead from the\n"
            "# decompiled C# declarations: base/interface -> inherits, a type named in a\n"
            "# method signature -> uses. Ambiguous short names are skipped, not guessed.\n"
            "# id_form: compound\n"
            f"# key_alias: {other}/callgraph\n"
            "# Shares its key space with the other platform's edge set by design.\n"
            f"# source_rows: {stats['edge_candidates']}\n"
            f"# dropped: {stats['edge_candidates'] - stats['uniq_edges']}\n"
            "# dropped_reason: the same (from,to,kind) triple seen more than once folds to one edge"
        ),
    ) + "id:string*\tfrom:string\tto:string\tkind:string\tstatus:string\tref_addr:string\tref_conf:string\tevidence:string\n"

    trheader = manifest(
        triage_name, "triage_managed", target_dir, f"{types_name}", "D1",
        extra=(
            f"# Porting-order work queue for the {label} binary, scored from MEASURED evidence.\n"
            "# MDD 8.5 scores log(size) and xrefs; both are unavailable because Ghidra\n"
            "# decoded no code (dec013), so this scores fields and methods per type,\n"
            "# which ILSpy measured exactly. ref_conf is 'probable': the counts are\n"
            "# certain, the choice of them as a size proxy is a judgement.\n"
            f"# This sheet is keyed by {types_name}.id on purpose: one triage annotation\n"
            "# per type, so preflight check 13 is exempted via key_alias.\n"
            f"# key_alias: {types_name},{other}/types,{other}/triage\n"
            "# id_form: compound\n"
            f"# source_rows: {stats['records']}\n"
            f"# dropped: {stats['records'] - stats['uniq_triage']}\n"
            "# dropped_reason: -"
        ),
    ) + "id:string*\treason:string\tscore:f32\tpriority:u8\tstatus:string\tref_addr:string\tref_conf:string\tevidence:string\n"

    nt = write_tsv(os.path.join(out, "types.tsv"), theader, types_rows)
    nm = write_tsv(os.path.join(out, "methods.tsv"), mheader, methods_rows)
    ng = write_tsv(os.path.join(out, "callgraph.tsv"), gheader, callgraph_rows)
    ntr = write_tsv(os.path.join(out, "triage.tsv"), trheader, triage_rows)

    print(f"{label:7} files {stats['files']:5}  types {nt:5}  methods {nm:6}  "
          f"edges {ng:5}  triage {ntr:5}  fields {stats['fields']:6}  "
          f"(ambiguous shorts skipped {stats['ambiguous_short_names']})")
    print(f"{label:7} unmatched inventory entries: {stats['unmatched_types']}")
    return 0


def main() -> int:
    if not os.path.isdir(EXPORTS):
        print(f"no exports dir at {EXPORTS}", file=sys.stderr)
        return 1
    rc = 0
    for t in TARGETS:
        rc |= emit_target(t)
    return rc


if __name__ == "__main__":
    raise SystemExit(main())
