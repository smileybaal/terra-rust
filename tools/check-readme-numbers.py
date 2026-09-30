#!/usr/bin/env python3
"""Re-derive the README's figures and refuse a README that has drifted from the tree.

Why this exists. README.md claims its numbers are re-derived, and for a while seven
of them were not: the client/server sheet restructure invalidated the generated-line
count, the sheet-row count, the kernel test count, the shared-id count, the fixture
error count and the object-database blob count, and nothing noticed. A hand-maintained
number is a number that lies (MDD, and the note above `rules_summary`), so the
derivable ones are checked here instead of trusted.

Scope, stated honestly:

  * Only figures that can be re-derived from the working tree are checked. Prose,
    dates, hashes and the "0 decoded instructions" claim are not. Each check names
    the figures it owns, so an incidental number inside a word (the 256 in "sha256")
    is not mistaken for a claim.
  * "2 divergent" is not checked: it needs the demo crate's comparison logic, and
    crates/terraria-demo already asserts it.
  * The object-database blob count is deliberately NOT checked. It changes with every
    commit, so a check would fail for a reason that is not a defect; the README hedges
    it as "as of this commit" instead.
  * The generated-line and placeholder figures need build output, so this refuses to
    run without it rather than skipping them silently. A check that quietly skips is
    the failure mode this repository's doctrine is built around.

Usage:
    cargo build -p terraria-kernel      # for the generated figures
    python tools/check-readme-numbers.py

Exit 0 when every checked figure agrees, 1 when one has drifted (with the README
value, the derived value and the line to fix), 2 when the tool cannot check.
"""
import glob
import io
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
README = os.path.join(ROOT, "README.md")
SHEETS = os.path.join(ROOT, "sheets")


def read(path):
    with io.open(path, encoding="utf-8") as f:
        return f.read()


def sheet_rows(rel):
    """The data rows of one sheet: manifest (#) lines and the header are dropped."""
    lines = [
        l for l in read(os.path.join(SHEETS, rel)).split("\n")
        if l.strip() and not l.startswith("#")
    ]
    return [l.split("\t") for l in lines[1:]]


def sheet_ids(rel):
    return {r[0] for r in sheet_rows(rel) if r}


def readme_line(prefix):
    for l in read(README).split("\n"):
        if l.startswith(prefix):
            return l
    fail("no README line starts with %r" % prefix)


def figures(line):
    """Every number on a line, as ints, in order."""
    return [int(x.replace(",", "")) for x in re.findall(r"\d[\d,]*", line)]


def generated_dir():
    """The newest sheetty output tree, or None when the kernel has not been built."""
    cands = glob.glob(os.path.join(ROOT, "target", "*", "build", "terraria-kernel-*", "out"))
    cands = [c for c in cands if glob.glob(os.path.join(c, "**", "*.rs"), recursive=True)]
    return max(cands, key=os.path.getmtime) if cands else None


PROBLEMS = []


def fail(msg):
    print("cannot check: %s" % msg, file=sys.stderr)
    sys.exit(2)


def check(label, prefix, derive, want_indices=None):
    """Compare the README figures this check owns against freshly derived ones.

    `want_indices` selects which numbers on the line are claims to compare. It is
    needed because a line can carry a number that is not a figure at all: the
    assets row says "with a real sha256 each", and 256 is a word, not a claim.
    """
    line = readme_line(prefix)
    on_the_line = figures(line)
    idx = list(range(len(on_the_line))) if want_indices is None else list(want_indices)
    want = [on_the_line[i] for i in idx]
    got = derive()
    if len(want) != len(got):
        PROBLEMS.append(
            "%s: README line carries %d checked figure(s), the tree yields %d: %s vs %s"
            % (label, len(want), len(got), want, got)
        )
        return
    for w, g in zip(want, got):
        if w != g:
            PROBLEMS.append(
                "%s: README says %s, the tree says %s (fix the line starting %r)"
                % (label, "{:,}".format(w), "{:,}".format(g), prefix)
            )
    if not any(p.startswith(label + ":") for p in PROBLEMS):
        print("OK   %-22s %s" % (label, ", ".join("{:,}".format(g) for g in got)))


def main():
    gen = generated_dir()
    if gen is None:
        fail(
            "no generated output under target/*/build/terraria-kernel-*/out.\n"
            "          Build it first: cargo build -p terraria-kernel"
        )
    gen_files = sorted(glob.glob(os.path.join(gen, "**", "*.rs"), recursive=True))
    port = [f for f in gen_files if f.endswith("port.rs")]
    if not port:
        fail("no port.rs in %s" % gen)
    port_text = read(port[0])

    all_sheets = sorted(glob.glob(os.path.join(SHEETS, "**", "*.tsv"), recursive=True))

    def every_sheet_rows():
        return sum(len(sheet_rows(os.path.relpath(p, SHEETS).replace("\\", "/"))) for p in all_sheets)

    def every_sheet_columns():
        total = 0
        for p in all_sheets:
            lines = [
                l for l in read(p).split("\n")
                if l.strip() and not l.startswith("#")
            ]
            total += len(lines[0].split("\t"))
        return total

    def kernel_tests():
        n = 0
        for f in sorted(glob.glob(os.path.join(ROOT, "kernel", "*.rs"))):
            n += read(f).count("#[test]")
        return n

    def shared_ids():
        c, s = sheet_ids("re/client/types.tsv"), sheet_ids("re/server/types.tsv")
        return [len(c & s), len(c - s), len(s - c)]

    check("sheet count", "| sheets |", lambda: [len(all_sheets)])
    check("row count", "| rows |", lambda: [every_sheet_rows()])
    check("column count", "| columns |", lambda: [every_sheet_columns()])
    check(
        "client evidence",
        "| client evidence (ILSpy) |",
        lambda: [
            len(sheet_rows("re/client/types.tsv")),
            len(sheet_rows("re/client/fields.tsv")),
            len(sheet_rows("re/client/methods.tsv")),
            len(sheet_rows("re/client/callgraph.tsv")),
        ],
    )
    check(
        "server evidence",
        "| server evidence (ILSpy) |",
        lambda: [
            len(sheet_rows("re/server/types.tsv")),
            len(sheet_rows("re/server/fields.tsv")),
            len(sheet_rows("re/server/methods.tsv")),
            len(sheet_rows("re/server/callgraph.tsv")),
        ],
    )
    # index 3 is "2 divergent", which needs the demo crate's logic to derive.
    check("client vs server", "| client vs server |", shared_ids, want_indices=(0, 1, 2))
    check(
        "generated port",
        "| Rust port (generated) |",
        lambda: [
            len(sheet_rows("re/server/types.tsv")),
            len(sheet_rows("re/server/fields.tsv")),
            len(sheet_rows("re/server/methods.tsv")),
            port_text.count("/// Referenced by the server but not declared in it."),
            sum(read(f).count("\n") for f in gen_files),
            len(gen_files),
        ],
    )
    check("kernel tests", "| Rust kernel (hand-written) |", lambda: [kernel_tests()])
    # figures 0-2 only: the line also says "a real sha256 each", and 256 is part of
    # a word there, not a claim.
    check(
        "strings / assets",
        "| strings / assets |",
        lambda: [
            len(sheet_rows("re/client/strings.tsv")),
            len(sheet_rows("re/assets.tsv")),
            sum(1 for r in sheet_rows("re/assets.tsv") if len(r) > 5 and r[5] == "found"),
        ],
        want_indices=(0, 1, 2),
    )
    # figures 0-1 only: index 2 is "0 decoded instructions", a Ghidra finding.
    check(
        "ghidra",
        "| Ghidra |",
        lambda: [len(sheet_rows("re/client/functions.tsv")), len(sheet_rows("re/client/types_pe.tsv"))],
        want_indices=(0, 1),
    )
    # the same line-count claim, stated in the intro prose
    check("intro lines", "port **compiles** -", lambda: [sum(read(f).count("\n") for f in gen_files), len(gen_files)])

    if PROBLEMS:
        print()
        print("README DRIFTED from the tree in %d place(s):" % len(PROBLEMS))
        for p in PROBLEMS:
            print("  %s" % p)
        print()
        print("Re-derive the figure, or fix the code. Do not delete the claim: a number")
        print("that is not re-derived is a number that lies (MDD).")
        return 1
    print()
    print("OK   every checked README figure agrees with the tree")
    return 0


if __name__ == "__main__":
    sys.exit(main())
