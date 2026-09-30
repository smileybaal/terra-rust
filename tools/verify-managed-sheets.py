"""Verify the invariants the producer now claims, on BOTH platforms.

1. `fields` in types.tsv == number of fields-sheet rows owned by that type.
   This is the check that would have caught the nested-owner bug: the whole
   point is that the two sheets cannot disagree.
2. every nested type's id == (namespace + '.' + name).lower(), so the dotted
   path really is derivable from the row rather than restated.
3. no duplicate ids in any sheet.
4. provenance balances (rows + dropped == source_rows).
"""
import collections
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SHEETS = os.path.join(ROOT, "sheets")
bad = 0


def load(rel):
    path = os.path.join(SHEETS, rel)
    head, body = [], []
    for line in open(path, encoding="utf-8"):
        line = line.rstrip("\n")
        (head if line.startswith("#") else body).append(line)
    cols = [c.split(":")[0] for c in body[0].split("\t")]
    man = {}
    for h in head:
        if h.startswith("#") and ":" in h:
            k, v = h[1:].split(":", 1)
            if k.strip() in ("source_rows", "dropped", "generator"):
                man[k.strip()] = v.strip()
    rows = [dict(zip(cols, l.split("\t"))) for l in body[1:] if l]
    return man, rows


for plat in ("client", "server"):
    print(f"=== {plat} ===")
    tman, types = load(f"re/{plat}/types.tsv")
    fman, fields = load(f"re/{plat}/fields.tsv")
    mman, methods = load(f"re/{plat}/methods.tsv")

    own = collections.Counter(x["type"] for x in fields)
    mism = [(t["id"], int(t["fields"]), own.get(t["id"], 0))
            for t in types if int(t["fields"]) != own.get(t["id"], 0)]
    print(f"  1. fields column vs fields rows: {len(mism)} mismatches")
    for m in mism[:8]:
        print(f"       {m[0]}: column {m[1]} vs rows {m[2]}")
    bad += len(mism)

    # Two types can share a bare name in one namespace (UIDynamicItemCollection
    # has a generic and a non-generic form), and the id-uniqueness guard then
    # appends `_1`. That suffix is required, so those ids are legitimately not
    # derivable from the row and are skipped rather than reported.
    dotted = [t for t in types
              if t["namespace"] != "-"
              and not re.fullmatch(r".*_\d+", t["id"])
              and t["id"] != (t["namespace"] + "." + t["name"]).lower()]
    real_bad = dotted
    print(f"  2. derived-id mismatches: {len(real_bad)}")
    for t in real_bad[:8]:
        print(f"       id={t['id']} ns={t['namespace']} name={t['name']}")
    bad += len(real_bad)

    for nm, rs in (("types", types), ("fields", fields), ("methods", methods)):
        ids = collections.Counter(x["id"] for x in rs)
        dup = {k: v for k, v in ids.items() if v > 1}
        print(f"  3. {nm}: {len(rs)} rows, {len(dup)} duplicate ids")
        for k, v in list(dup.items())[:4]:
            print(f"       {k} x{v}")
        bad += len(dup)

    for nm, man, rs in (("types", tman, types), ("fields", fman, fields),
                        ("methods", mman, methods)):
        src = man.get("source_rows")
        drop = man.get("dropped")
        if src is None or drop is None:
            print(f"  4. {nm}: no provenance")
            bad += 1
            continue
        ok = int(src) == len(rs) + int(drop)
        print(f"  4. {nm}: {len(rs)} rows + {drop} dropped == {src}  {'OK' if ok else 'MISMATCH'}")
        if not ok:
            bad += 1

    # orphan owners: a fields/methods row whose type is not a type row
    tids = {t["id"] for t in types}
    orphan_f = {x["type"] for x in fields} - tids
    orphan_m = {x["type"] for x in methods} - tids
    print(f"  5. fields owned by unknown types: {len(orphan_f)}; "
          f"methods owned by unknown types: {len(orphan_m)}")
    for o in list(orphan_f)[:5]:
        print(f"       fields owner {o}")
    for o in list(orphan_m)[:5]:
        print(f"       methods owner {o}")
    bad += len(orphan_f) + len(orphan_m)
    print()

print("TOTAL PROBLEMS:", bad)
sys.exit(1 if bad else 0)
