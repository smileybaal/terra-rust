"""Prove that no original game file is in this repository's HISTORY.

Checking the working tree is not enough. A file that was committed once and
deleted later is still in the pack and still ships on push, so the only question
that matters for "what will GitHub receive" is: does any git OBJECT equal any file
from the Terraria install?

The comparison is exact and does not need a checkout. A git blob's name is
sha1("blob <length>\\0" + content), so hashing every install file that way and
intersecting with the set of blob names the repository actually contains is a
byte-exact answer for all of history, not a guess from filenames or extensions.

Three independent passes, because each catches what the others miss:

  BLOB      every object in the object database vs every install file, by content
  PATH      every path ever added in any commit, vs the install's paths and names
  SUSPECT   the largest blobs in history, listed for eyeballing: a game file that
            somehow got in would be far bigger than anything this book produces

  python tools/check-history-for-game-files.py
"""
import hashlib
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_INSTALL = r"C:\Steam\steamapps\common\Terraria"

# Shapes that must never appear as a committed path, whatever the content.
SUSPECT_EXT = {
    ".exe", ".dll", ".xnb", ".xwb", ".xsb", ".wav", ".ogg", ".mp3", ".pdb",
    ".png", ".jpg", ".jpeg", ".bmp", ".gif", ".fxc", ".so", ".dylib",
}


def git(*args):
    return subprocess.run(
        ["git", "-C", ROOT, *args], capture_output=True, text=True, errors="replace"
    ).stdout


def blob_name(path):
    """The git object name a file WOULD have: sha1('blob <len>\\0' + bytes)."""
    try:
        data = open(path, "rb").read()
    except OSError:
        return None
    h = hashlib.sha1()
    h.update(b"blob %d\0" % len(data))
    h.update(data)
    return h.hexdigest()


def main():
    install = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_INSTALL
    if not os.path.isdir(install):
        print(f"install not found: {install}", file=sys.stderr)
        return 2

    # ---- every object the repository actually contains --------------------
    print("reading the object database ...")
    objects = {}
    for line in git("cat-file", "--batch-all-objects", "--batch-check=%(objecttype) %(objectname) %(objectsize)").splitlines():
        parts = line.split()
        if len(parts) == 3:
            objects[parts[1]] = (parts[0], int(parts[2]))
    blobs = {k for k, v in objects.items() if v[0] == "blob"}
    print(f"  {len(objects)} objects, {len(blobs)} of them blobs")

    # ---- every file in the install, by the name it WOULD have ------------
    print("hashing the install ...")
    game_by_blob = {}
    n = 0
    for dirpath, _, names in os.walk(install):
        for nm in names:
            p = os.path.join(dirpath, nm)
            b = blob_name(p)
            n += 1
            if b:
                game_by_blob.setdefault(b, p)
    print(f"  {n} files hashed")

    # ---- pass 1: content ------------------------------------------------
    # Reachability matters and is not the same question as existence. `git add`
    # writes a blob into the object database IMMEDIATELY, so a commit that the
    # pre-commit guard then refuses still leaves the blob behind as an unreachable
    # loose object. That object is not in any commit and is not sent by a push, but
    # it is on disk until it is pruned, and the two cases deserve different answers
    # rather than one alarming yes/no.
    reachable = set()
    for line in git("rev-list", "--objects", "--all").splitlines():
        parts = line.split(" ", 1)
        if parts and len(parts[0]) == 40:
            reachable.add(parts[0])

    shared = sorted(set(game_by_blob) & blobs)
    reach = [b for b in shared if b in reachable]
    unreach = [b for b in shared if b not in reachable]
    print(f"\n== BLOB: install files present as git objects: {len(shared)} ==")
    print(f"   reachable from a ref (these WOULD be pushed): {len(reach)}")
    for b in reach:
        print(f"      {b}  {os.path.relpath(game_by_blob[b], install)}")
    print(f"   unreachable leftovers in .git/objects (prune these): {len(unreach)}")
    for b in unreach:
        print(f"      {b}  {os.path.relpath(game_by_blob[b], install)}")

    # ---- pass 2: paths ever added ---------------------------------------
    print("\n== PATH: every path ever added in any commit ==")
    paths = set()
    for line in git("log", "--all", "--pretty=format:", "--name-only", "--diff-filter=A").splitlines():
        line = line.strip()
        if line:
            paths.add(line)
    print(f"  {len(paths)} distinct paths added across all of history")

    install_paths = set()
    install_names = set()
    for dirpath, _, names in os.walk(install):
        for nm in names:
            rel = os.path.relpath(os.path.join(dirpath, nm), install).replace("\\", "/").lower()
            install_paths.add(rel)
            install_names.add(nm.lower())

    path_hits = sorted(p for p in paths if p.lower() in install_paths or os.path.basename(p).lower() in install_names)
    ext_hits = sorted(p for p in paths if os.path.splitext(p)[1].lower() in SUSPECT_EXT)
    print(f"  paths matching an install path or name: {len(path_hits)}")
    for p in path_hits[:40]:
        print(f"     {p}")
    print(f"  paths with a game-file extension: {len(ext_hits)}")
    for p in ext_hits[:40]:
        print(f"     {p}")

    # ---- pass 3: the biggest blobs --------------------------------------
    print("\n== SUSPECT: the 15 largest blobs in history ==")
    biggest = sorted(blobs, key=lambda b: objects[b][1], reverse=True)[:15]
    # map blob -> a path it appears at, for readability
    where = {}
    for line in git("rev-list", "--objects", "--all").splitlines():
        parts = line.split(" ", 1)
        if len(parts) == 2:
            where.setdefault(parts[0], parts[1])
    for b in biggest:
        print(f"   {objects[b][1]:>10} bytes  {b[:12]}  {where.get(b, '(no path)')}")

    bad = len(reach) + len(path_hits) + len(ext_hits)
    print("\nPUSHABLE GAME FILES (reachable from a ref):", len(reach))
    if unreach:
        print(f"unreachable leftovers, not pushable but present on disk: {len(unreach)}")
        print("  clear them with: git gc --prune=now")
    print("HISTORY CONTAINS GAME FILES:", "YES" if bad else "NO")
    print("problems:", bad)
    return 1 if bad else 0


sys.exit(main())
