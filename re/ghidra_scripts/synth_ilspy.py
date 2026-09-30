#!/usr/bin/env python3
"""synth_ilspy.py: ILSpy exports -> canonical evidence sheets.

THE SPREADSHEET METHOD, Mode B (MDD 8.6). Decompiled output is EVIDENCE, never
source. This script is the [normalize] stage: it reads the raw producer output
and writes canonical TSV into sheets/re/.

Inputs (all under re/exports/):
  ilspy_entities_{c,s,e,i,d}.txt   entity inventories ("Class Terraria.Foo")
  ilspy/**/*.cs                    the decompiled project tree (one file per type)

Outputs, one set per managed subject (see TARGETS below):
  sheets/re/<platform>/types.tsv
  sheets/re/<platform>/methods.tsv
  sheets/re/<platform>/callgraph.tsv
  sheets/re/<platform>/triage.tsv

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

# ---------------------------------------------------------------------------
# member extraction
#
# The first version of this producer counted FIELD_START matches while tracking
# brace depth, but only skipped a nested type's DECLARATION line, never its
# members. So a nested type's members were counted against its parent:
# Terraria.Player reported 1316 "fields" when 1241 belong to Player itself and
# 75 belong to its 21 nested types. That is the wrong owner, and a Rust
# projection built on it puts a nested type's fields inside the parent struct.
#
# So nested types are now parsed as types in their own right (they are in the
# ILSpy entity inventory too: `Class Terraria.Player.Settings`), members are
# attributed to the type that DECLARES them, and `fields` counts own members.
# ---------------------------------------------------------------------------

MODIFIERS = {
    "public", "private", "protected", "internal", "static", "readonly", "const",
    "volatile", "new", "override", "virtual", "abstract", "sealed", "unsafe",
    "extern", "partial", "event", "fixed", "required", "async", "file",
    "ref", "in", "out", "params", "this",
}

ACCESS = {"public", "private", "protected", "internal"}

# `get`/`set` and friends are the property body, not members of it
ACCESSOR_WORDS = {"get", "set", "init", "add", "remove", "get;", "set;", "init;"}


def strip_literals(ln: str) -> str:
    """Blank out string and char literals, so a brace inside one is not counted
    as code. `Console.WriteLine("{")` must not look like an open block, and with
    1551 files of decompiled output that is not a hypothetical.)"""
    out = []
    i = 0
    n = len(ln)
    while i < n:
        c = ln[i]
        if c == '"':
            i += 1
            while i < n:
                if ln[i] == "\\":
                    i += 2
                    continue
                if ln[i] == '"':
                    i += 1
                    break
                i += 1
            out.append('""')
            continue
        if c == "'":
            i += 1
            while i < n:
                if ln[i] == "\\":
                    i += 2
                    continue
                if ln[i] == "'":
                    i += 1
                    break
                i += 1
            out.append("''")
            continue
        out.append(c)
        i += 1
    code = "".join(out)
    cut = code.find("//")
    return code[:cut] if cut >= 0 else code


def brace_delta(ln: str) -> int:
    c = strip_literals(ln)
    return c.count("{") - c.count("}")


def _depths(s: str, i: int, depth: int) -> tuple[int, str]:
    """Advance one character, maintaining nesting depth for the delimiters that
    can hide a comma or an equals sign."""
    c = s[i]
    if c in "<([":
        depth += 1
    elif c in ">)]":
        depth -= 1
    return depth, c


def split_top_level(s: str, sep: str = ",") -> list[str]:
    """Split on `sep` at nesting depth 0, so `Dictionary<string, int> x` stays
    in one piece."""
    parts = []
    depth = 0
    cur = []
    for i, c in enumerate(s):
        if c in "<([{":
            depth += 1
        elif c in ">)]}":
            depth -= 1
        if c == sep and depth == 0:
            parts.append("".join(cur))
            cur = []
            continue
        cur.append(c)
    parts.append("".join(cur))
    return parts


def find_top_level_eq(s: str) -> int:
    """Index of the first assignment `=` that is not part of `==`, `!=`, `<=`,
    `>=` or `=>`, at nesting depth 0. -1 when there is none."""
    depth = 0
    for i, c in enumerate(s):
        if c in "<([{":
            depth += 1
        elif c in ">)]}":
            depth -= 1
        elif c == "=" and depth == 0:
            nxt = s[i + 1] if i + 1 < len(s) else ""
            prv = s[i - 1] if i > 0 else ""
            if nxt == "=" or prv in "=!<>" or nxt == ">":
                continue
            return i
    return -1


def split_decl(head: str) -> tuple[str, str, str]:
    """`public static List<int> Names` -> ('public static', 'List<int>', 'Names').
    Modifiers are peeled from the front; the name is the last token."""
    toks = head.split()
    if not toks:
        return ("", "", "")
    name = toks[-1].lstrip("@")
    rest = toks[:-1]
    mods = []
    while rest and rest[0] in MODIFIERS:
        mods.append(rest.pop(0))
    return (" ".join(mods), " ".join(rest), name)


def member_kind_and_parts(line: str, type_kind: str) -> list[tuple[str, str, str, str]]:
    """Classify one line that sits directly inside a type body.

    Returns a list of (kind, modifiers, declared_type, name), empty when the line
    is not a member declaration (a brace, an accessor, a stray statement). It is
    a list because C# allows several declarators on one line: `int a, b;`.

    The order of the tests matters and was got wrong once: a field whose
    initializer spans lines (`static readonly char[] X = new char[64]` then a
    brace block) ends with neither `;` nor `{`, and classifying it by its ending
    turned 1,248 real fields into "properties". A top-level `=` is what makes a
    field a field, whatever the line ends with.
    """
    s = strip_literals(line).strip()
    if not s:
        return []
    if s in ("{", "}") or s in ACCESSOR_WORDS:
        return []
    if s.startswith("#") or s.startswith("["):
        return []

    # enum members: `None = -1,` or `Slayer,`
    if type_kind == "enum":
        m = re.match(r"^(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*(?:=\s*(?P<val>[^,]+?))?\s*,?$", s)
        if m:
            return [("enum_member", "", m.group("val") or "", m.group("name"))]
        return []

    eq = find_top_level_eq(s)
    arrow = s.find("=>")

    # Everything after a top-level `=` or `=>` is an initializer or a body, and a
    # multi-line one must not leak into the name. Taking the name from the whole
    # line produced names like `"");` and, for `operator float`, the name `float`.
    cut = len(s)
    if eq >= 0:
        cut = eq
    if arrow >= 0:
        cut = min(cut, arrow)
    head = s[:cut]
    brace_at = head.find("{")
    if brace_at >= 0:
        head = head[:brace_at]
    paren_at = head.find("(")
    pre = head[:paren_at].strip() if paren_at >= 0 else head.strip()
    pre_toks = pre.split()

    # Separating members from STATEMENTS is the hard part, and brace depth cannot
    # do it: a property whose expression body spans lines leaves its continuation
    # lines at member depth. Two rules do separate them:
    #   - a member declaration starts with an accessibility modifier, OR
    #   - it is an explicit interface implementation, which carries none
    #     (`bool IEnumerator.MoveNext()`), and is recognised by having a return
    #     type before a dotted member name.
    # Requiring only the first rule silently dropped 438 methods; requiring only
    # the second let `Language.GetText(...)` through as a member.
    first = pre_toks[0] if pre_toks else ""
    if first not in ACCESS:
        explicit_impl = len(pre_toks) >= 2 and "." in pre_toks[-1]
        if not explicit_impl:
            return []

    # The method test comes FIRST, and it is safe to do so because `cut` already
    # truncated at `=>`: for `public string ToString() => "x"` the paren is before
    # the cut, while for `public int X => Foo()` it is after and is gone. Testing
    # the arrow first turned every expression-bodied METHOD into a property.
    if paren_at >= 0:
        # C# writes a conversion as `implicit operator BitsByte(byte b)`. The
        # operator token IS the method's identity, so keep it: the id encoder
        # turns `operator float` into `op_float`, and dropping it would name the
        # method `float` and collide with the field named `float`.
        if " operator " in f" {pre} ":
            left, _, opname = pre.partition(" operator ")
            toks = left.split()
            mods = []
            while toks and toks[0] in MODIFIERS:
                mods.append(toks.pop(0))
            name = f"operator {opname.strip()}"
            if name and name != "operator":
                return [("method", " ".join(mods), " ".join(toks), name)]
            return []
        mods, ty, name = split_decl(pre)
        return [("method", mods, ty, name)] if name else []

    if arrow >= 0 and (eq < 0 or arrow < eq):
        mods, ty, name = split_decl(pre)
        return [("property", mods, ty, name)] if name and ty else []

    # fields, with or without an initializer, and possibly several declarators
    body = s[:eq] if eq >= 0 else s.rstrip().rstrip(";")
    parts = split_top_level(body)
    out = []
    first_mods, first_ty, _ = split_decl(parts[0].strip())
    for i, part in enumerate(parts):
        part = part.strip()
        if not part:
            continue
        if i == 0:
            mods, ty, name = first_mods, first_ty, split_decl(part)[2]
        else:
            # a later declarator carries only a name: `int a, b;`
            mods, ty, name = first_mods, first_ty, split_decl(part)[2]
        if not name or not ty:
            continue
        modwords = mods.split()
        if "const" in modwords:
            kind = "const"
        elif "event" in modwords:
            kind = "event"
        else:
            kind = "field"
        out.append((kind, mods, ty, name))

    if out:
        return out

    # a block-bodied property: `public static IPAddress ServerIP` then `{`
    mods, ty, name = split_decl(pre)
    if name and ty:
        return [("property", mods, ty, name)]
    return []


def parse_params(sig: str) -> tuple[str, str]:
    """Split a C# signature into (return type, `name:type;name:type`).

    Parsed once here and stored, rather than re-parsed by every consumer: the
    emitter would otherwise have to rebuild a C# parser in Rust, and a signature
    is evidence whereas a re-derived parse is not.
    """
    s = strip_literals(sig).strip().rstrip(";").strip()
    open_at = s.find("(")
    if open_at < 0:
        return ("void", "")
    depth = 0
    close_at = -1
    for i in range(open_at, len(s)):
        if s[i] == "(":
            depth += 1
        elif s[i] == ")":
            depth -= 1
            if depth == 0:
                close_at = i
                break
    if close_at < 0:
        return ("void", "")
    head = s[:open_at].strip()
    body = s[open_at + 1:close_at].strip()

    ret = "void"
    if " operator " in f" {head} ":
        ret = head.split(" operator ", 1)[0].strip()
    else:
        toks = head.split()
        keep = []
        while toks and toks[0] in MODIFIERS:
            toks.pop(0)
        keep = toks
        if len(keep) >= 2:
            ret = " ".join(keep[:-1])
        elif len(keep) == 1:
            ret = "void"  # a constructor: no return type is written
    params = []
    if body:
        for p in split_top_level(body):
            p = p.strip()
            if not p:
                continue
            # drop a default value: `string s = "x"` is one parameter, and
            # keeping the `= ...` produced a parameter literally named `""`
            # (the literal was blanked) and put a quoted cell in the sheet.
            p = split_top_level(p, "=")[0].strip()
            if not p:
                continue
            toks = p.split()
            while toks and toks[0] in ("ref", "out", "in", "params", "this", "scoped"):
                toks.pop(0)
            if not toks:
                continue
            if len(toks) == 1:
                params.append(f"arg{len(params)}:{toks[0]}")
                continue
            pname = toks[-1].lstrip("@")
            ptype = " ".join(toks[:-1])
            # a name that is not an identifier is not a name
            if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", pname):
                pname = f"arg{len(params)}"
            params.append(f"{pname}:{ptype}")
    return (ret, ";".join(params))


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
        self.member_lines = 0
        self.skipped_lines = 0
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
                end = self._read_type(i, decl[0], decl[1], self.ns, None)
                i = i + 1 if end <= i else end
                continue
            i += 1

    def _body_extent(self, start: int) -> int:
        """Index one past the line that closes the block opened at or after
        `start`. Braces inside string literals do not count.

        A declaration that ends in `;` before any `{` has NO body - that is a
        delegate - and must not be allowed to swallow the next type's braces.
        """
        i = start
        n = len(self.lines)
        # Walk to the body. A declaration terminated by `;` before any `{` has no
        # body at all: that is a delegate, and treating it as if it had one made
        # it swallow the braces of whatever type came next in the file.
        while i < n:
            code = strip_literals(self.lines[i])
            if "{" in code:
                break
            if ";" in code:
                return start + 1
            i += 1
        else:
            return start + 1

        depth = 0
        began = False
        while i < n:
            if not began:
                if "{" in strip_literals(self.lines[i]):
                    began = True
                    depth = brace_delta(self.lines[i])
                    if depth <= 0:
                        return i + 1
                i += 1
                continue
            depth += brace_delta(self.lines[i])
            if depth <= 0:
                return i + 1
            i += 1
        return n

    def _read_type(self, start: int, decl_kind: str, decl_name: str,
                   ns: str, parent_full: str | None) -> int:
        """Parse one type and, recursively, every type nested in it.

        Returns the index just past this type's closing brace. Nested types are
        appended to `self.types` as types in their own right, which is what makes
        a member's owner correct: `Terraria.Player.Settings` owns its own fields
        instead of lending them to `Terraria.Player`.
        """
        decl_name = decl_name.lstrip("@")
        if parent_full:
            full = f"{parent_full}.{decl_name}"
            own_ns = parent_full
        else:
            full = f"{ns}.{decl_name}" if ns else decl_name
            own_ns = ns or "-"

        t = {"kind": decl_kind, "name": decl_name, "full": full, "ns": own_ns,
             "nested": parent_full is not None,
             "members": [], "methods": [], "bases": [], "sigtext": [],
             "children": []}

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

        end = self._body_extent(start)
        self.types.append(t)

        # Walk the body tracking brace depth, so only DIRECT members are
        # considered. Statements inside a method body are at depth >= 2 and are
        # not members, and counting them would inflate the provenance numbers
        # with lines that were never candidates.
        depth = 0
        i = start
        while i < end:
            ln = self.lines[i]
            code = strip_literals(ln)
            if depth == 0:
                if "{" not in code:
                    i += 1
                    continue
                depth = brace_delta(ln)
                i += 1
                continue
            if depth == 1:
                nested = type_decl(ln)
                if nested:
                    t["children"].append(nested[1].lstrip("@"))
                    # A nested type's extent is balanced, so depth is still 1
                    # when this returns; no depth update is needed.
                    i = self._read_type(i, nested[0], nested[1], ns, full)
                    continue
                s = ln.strip()
                if s and not s.startswith("//") and not s.startswith("/*") and not s.startswith("*"):
                    self.member_lines += 1
                    found = member_kind_and_parts(ln, decl_kind)
                    if not found:
                        self.skipped_lines += 1
                    for got in found:
                        mkind, mods, ty, name = got
                        if mkind == "method":
                            sig = code.strip().split("{")[0].strip()
                            # a constructor is named after its type and carries no
                            # return type, which is how it is told apart here
                            real = "ctor" if (name == decl_name and not ty) else "method"
                            t["methods"].append((name, sig, real, mods))
                            t["sigtext"].append(sig)
                        else:
                            t["members"].append((mkind, mods, ty, name))
            depth += brace_delta(ln)
            i += 1
        return end


def scan() -> tuple[list[list[str]], list[list[str]], list[list[str]], dict]:
    entities = read_entities()
    ent_kind = {full: kind for kind, full in entities}
    ent_ns = {full: split_ns(full)[0] for _, full in entities}

    types_rows: list[list[str]] = []
    methods_rows: list[list[str]] = []
    fields_rows: list[list[str]] = []
    records: list[dict] = []
    seen_types: set[str] = set()
    seen_method_ids: dict[str, int] = {}
    seen_field_ids: dict[str, int] = {}
    seen_type_ids: dict[str, int] = {}
    stats = {"files": 0, "skipped": 0, "methods": 0, "fields": 0, "ctors": 0,
             "props": 0, "multi": 0, "nested": 0, "events": 0, "consts": 0,
             "enum_members": 0, "member_lines": 0, "skipped_lines": 0}

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
        stats["member_lines"] += reader.member_lines
        stats["skipped_lines"] += reader.skipped_lines
        if not reader.types:
            stats["skipped"] += 1
            continue
        if len(reader.types) > 1:
            stats["multi"] += 1

        for t in reader.types:
            decl_name = t["name"]
            ns = reader.ns
            # nested types carry their parent's full path in `ns`, so the id
            # stays the dotted path the inventory uses (Terraria.Player.Settings)
            full = t["full"]
            kind = ent_kind.get(full, t["kind"])
            seen_types.add(full)
            own_members = len(t["members"])
            stats["fields"] += own_members
            stats["methods"] += len(t["methods"])
            if t["nested"]:
                stats["nested"] += 1

            # A top-level type takes its namespace from the inventory, because file
            # layout can put a global-namespace type in a nested directory (the
            # nativefiledialog.cs shim lands under Terraria/). A NESTED type's
            # namespace is its parent's full path, which the inventory would
            # otherwise flatten to the parent's namespace and lose the nesting.
            if t["nested"]:
                ns_final = t["ns"]
            else:
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
                tid, kind, ns_final or "-", decl_name, str(own_members),
                ",".join(t["bases"]) if t["bases"] else "-", "todo",
                artifact, f"ilspy:{artifact}", "certain", f"ilspycmd {artifact}",
            ])
            records.append({
                "tid": tid, "full": full, "fields": own_members,
                "methods": t["methods"], "bases": t["bases"],
                "sigtext": t["sigtext"], "artifact": artifact,
            })

            # ---------------------------------------------------------------
            # members -> re/<platform>/fields.tsv
            #
            # The owner is the type that DECLARES the member. `fields` in the
            # types sheet is this count, so the two sheets agree by
            # construction rather than by coincidence (it was the disagreement
            # between them that exposed the nested-owner bug).
            # ---------------------------------------------------------------
            for mkind, mods, ty, mname in t["members"]:
                if mkind == "property":
                    stats["props"] += 1
                elif mkind == "event":
                    stats["events"] += 1
                elif mkind == "const":
                    stats["consts"] += 1
                elif mkind == "enum_member":
                    stats["enum_members"] += 1
                fid = re.sub(r"[^a-z0-9_.]+", "_", f"{tid}.{mname.lower()}").strip("_")
                uniq_f = seen_field_ids.get(fid, 0)
                seen_field_ids[fid] = uniq_f + 1
                if uniq_f:
                    fid = f"{fid}_{uniq_f}"
                fields_rows.append([
                    fid, tid, mname, ty or "-", mkind, mods or "-", "todo",
                    artifact, f"ilspy:{artifact}", "certain", f"ilspycmd {artifact}",
                ])

            counts: dict[str, int] = {}
            for name, sig, mkind, mods in t["methods"]:
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
                ret, params = parse_params(sig)
                methods_rows.append([
                    # `type` is the OWNER's type id, not the C# spelling: the same
                    # value as types.id, so the column is a real foreign key and
                    # joins without a case fold. The C# spelling is preserved in
                    # types.namespace and types.name. (It was `full` here and
                    # `tid` in the fields sheet, so the two sheets disagreed about
                    # how to name the same type.)
                    mid, tid, name, sig, ret, params, mods or "-", mkind, "-",
                    "todo", artifact, f"ilspy:{artifact}", "certain",
                    f"ilspycmd {artifact}",
                ])

    stats["types"] = len(types_rows)
    stats["unmatched_types"] = len(set(ent_kind) - seen_types)
    stats["uniq_fields"] = len({r[0] for r in fields_rows})

    # -----------------------------------------------------------------------
    # managed type-dependency edges -> re/<platform>/callgraph.tsv
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
    # triage from measured managed evidence -> re/<platform>/triage.tsv
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

    return types_rows, methods_rows, fields_rows, triage_rows, callgraph_rows, stats


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
    types_rows, methods_rows, fields_rows, triage_rows, callgraph_rows, stats = scan()

    types_name = f"{base}/types"
    methods_name = f"{base}/methods"
    fields_name = f"{base}/fields"
    graph_name = f"{base}/callgraph"
    triage_name = f"{base}/triage"
    target_dir = f"sheets/re/{label}"

    theader = manifest(
        types_name, "types", target_dir, "re/sources", "D1",
        extra=(
            f"# Managed type inventory for the {label} binary, from ILSpy.\n"
            "# kind is class|struct|enum|interface|delegate.\n"
            "# NESTED types are rows too, under their parent's dotted path\n"
            "# (terraria.player.settings), because the ILSpy inventory lists them\n"
            "# and because a nested type owns its own members.\n"
            "# `fields` is the number of rows in the sibling fields sheet whose\n"
            "# `type` is this id, so the two agree by construction (dec018).\n"
            "# Type ids are dotted namespace paths, so the compound-key exception is declared.\n"
            "# emit: rust\n"
            "# id_form: compound\n"
            f"# key_alias: {other}/types,{other}/triage,{fields_name},{other}/fields\n"
            "# A TYPE and a FIELD of its parent can fold to the same id, because ids\n"
            "# are lowercase (D7) while C# is case-sensitive: the field\n"
            "# `Player.settings` and the nested type `Player.Settings`. Check 13 is\n"
            "# exempted because both rows are correct (dec019).\n"
            f"# source_rows: {stats['entity_count']}\n"
            f"# dropped: {stats['entity_count'] - stats['uniq_types']}\n"
            "# dropped_reason: compiler-generated types have no emitted .cs file (dec010); "
            "resource shims were also absent from the inventory comparison"
        ),
    ) + "id:string*\tkind:string\tnamespace:string\tname:string\tfields:u16\tbase:string\tstatus:string\tartifact:string\tref_addr:string\tref_conf:string\tevidence:string\n"

    fheader = manifest(
        fields_name, "fields_managed", target_dir, f"{types_name},re/sources", "D1",
        extra=(
            f"# Member inventory for the {label} binary: fields, consts, events,\n"
            "# properties and enum members, each owned by the type that DECLARES\n"
            "# it. Members of a nested type belong to the nested type, not to its\n"
            "# parent; that distinction is what makes a struct projection correct\n"
            "# (dec018), and it is why the counts here reconcile exactly with the\n"
            "# `fields` column of the types sheet.\n"
            "# kind is field|const|event|property|enum_member.\n"
            "# field_type is the declared C# type, verbatim, including generics and\n"
            "# array suffixes; the projection maps it, it is not rewritten here.\n"
            "# id_form: compound\n"
            f"# key_alias: {other}/fields,{types_name},{methods_name},{other}/methods\n"
            "# A field and a nested type can fold to the same id (see the types sheet,\n"
            "# dec019); `kind` distinguishes them.\n"
            f"# source_rows: {stats['member_lines']}\n"
            f"# dropped: {stats['member_lines'] - stats['uniq_fields']}\n"
            "# dropped_reason: lines inside a type body that are not member "
            "declarations (braces, get/set accessors, and statements that belong to "
            "an initializer)"
        ),
    ) + "id:string*\ttype:string\tname:string\tfield_type:string\tkind:string\tmodifiers:string\tstatus:string\tartifact:string\tref_addr:string\tref_conf:string\tevidence:string\n"

    mheader = manifest(
        methods_name, "methods", target_dir, f"{types_name},re/sources", "D1",
        extra=(
            f"# Managed method inventory for the {label} binary, from ILSpy.\n"
            "# ids are lowercased dotted paths; the compound key joins type and\n"
            "# method with '.', so id_form is declared.\n"
            "# `ret` and `params` are the signature parsed ONCE, here, into a form a\n"
            "# projection can consume mechanically. params is `name:type` joined by\n"
            "# ';' so a comma inside a generic type cannot be confused with the\n"
            "# separator. The raw `signature` is kept as the evidence for both.\n"
            "# id_form: compound\n"
            f"# key_alias: {other}/methods,{fields_name},{other}/fields\n"
            "# Shares its key space with the other platform's method inventory by design,\n"
            "# and with the fields sheet because a CASE-DISTINCT member folds to the\n"
            "# same id: the field `Main.autoJoin` and the method `Main.AutoJoin(string)`\n"
            "# are two real members and one lowercase id (dec019).\n"
            f"# source_rows: {stats['methods']}\n"
            f"# dropped: {stats['methods'] - stats['uniq_methods']}\n"
            "# dropped_reason: -"
        ),
    ) + "id:string*\ttype:string\tname:string\tsignature:string\tret:string\tparams:string?\tmodifiers:string\tkind:string\til_offset:string\tstatus:string\tartifact:string\tref_addr:string\tref_conf:string\tevidence:string\n"

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
            f"# key_alias: {types_name},{other}/types,{other}/triage,{fields_name},{other}/fields\n"
            "# id_form: compound\n"
            f"# source_rows: {stats['records']}\n"
            f"# dropped: {stats['records'] - stats['uniq_triage']}\n"
            "# dropped_reason: -"
        ),
    ) + "id:string*\treason:string\tscore:f32\tpriority:u8\tstatus:string\tref_addr:string\tref_conf:string\tevidence:string\n"

    nt = write_tsv(os.path.join(out, "types.tsv"), theader, types_rows)
    nm = write_tsv(os.path.join(out, "methods.tsv"), mheader, methods_rows)
    nf = write_tsv(os.path.join(out, "fields.tsv"), fheader, fields_rows)
    ng = write_tsv(os.path.join(out, "callgraph.tsv"), gheader, callgraph_rows)
    ntr = write_tsv(os.path.join(out, "triage.tsv"), trheader, triage_rows)

    print(f"{label:7} files {stats['files']:5}  types {nt:5}  methods {nm:6}  "
          f"fields {nf:6}  edges {ng:5}  triage {ntr:5}  "
          f"(nested types {stats['nested']:4}, props {stats['props']:5}, "
          f"consts {stats['consts']:5}, events {stats['events']:4}, "
          f"enum members {stats['enum_members']:4})")
    print(f"{label:7} unmatched inventory entries: {stats['unmatched_types']}")
    print(f"{label:7} member lines {stats['member_lines']}, of which "
          f"{stats['skipped_lines']} were not declarations "
          f"({stats['skipped_lines'] * 100 // max(1, stats['member_lines'])}%)")
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
