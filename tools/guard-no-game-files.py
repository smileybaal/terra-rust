"""Block a commit that would add a file from the Terraria install.

The scans prove the repository is clean today. This is what keeps it clean: a
pre-commit guard that refuses a commit the moment a game file is staged, rather
than relying on someone remembering to run the scans before every push.

It is deliberately cheap. Hashing the whole 804 MB install on every commit would be
unusable, so the install's blob names are cached once in .git/ (untracked, never
committed) and the guard only hashes the files being committed, which are few.

  python tools/guard-no-game-files.py --staged        # the pre-commit hook path
  python tools/guard-no-game-files.py PATH [PATH...]  # check specific files
  python tools/guard-no-game-files.py --rebuild       # refresh the cache

Exits 0 when clear, 1 with a named reason when not.
"""
import hashlib
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
INSTALL = r"C:\Steam\steamapps\common\Terraria"
CACHE = os.path.join(ROOT, ".git", "terraria-install-blobs.txt")

SUSPECT_EXT = {
    ".exe", ".dll", ".xnb", ".xwb", ".xsb", ".wav", ".ogg", ".mp3", ".pdb",
    ".png", ".jpg", ".jpeg", ".bmp", ".gif", ".fxc", ".so", ".dylib",
}


def git(*args):
    return subprocess.run(
        ["git", "-C", ROOT, *args], capture_output=True, text=True, errors="replace"
    ).stdout


def blob_name_of_bytes(data):
    h = hashlib.sha1()
    h.update(b"blob %d\0" % len(data))
    h.update(data)
    return h.hexdigest()


def blob_name_of_file(path):
    try:
        return blob_name_of_bytes(open(path, "rb").read())
    except OSError:
        return None


def load_cache(rebuild):
    if not rebuild and os.path.exists(CACHE):
        with open(CACHE, encoding="utf-8") as f:
            return set(f.read().split())
    if not os.path.isdir(INSTALL):
        return None  # cannot build; the guard degrades to the name check
    blobs = set()
    for dirpath, _, names in os.walk(INSTALL):
        for nm in names:
            b = blob_name_of_file(os.path.join(dirpath, nm))
            if b:
                blobs.add(b)
    os.makedirs(os.path.dirname(CACHE), exist_ok=True)
    with open(CACHE, "w", encoding="utf-8") as f:
        f.write("\n".join(sorted(blobs)))
    return blobs


def install_names():
    names = set()
    if os.path.isdir(INSTALL):
        for dirpath, _, files in os.walk(INSTALL):
            for nm in files:
                names.add(nm.lower())
    return names


def main():
    args = [a for a in sys.argv[1:] if a != "--staged"]
    rebuild = "--rebuild" in args
    if rebuild:
        load_cache(True)
        print(f"cache rebuilt at {os.path.relpath(CACHE, ROOT)}")
        return 0

    if "--staged" in sys.argv:
        out = git("diff", "--cached", "--name-only", "--diff-filter=ACM")
        targets = [p for p in out.splitlines() if p.strip()]
    else:
        targets = [a for a in args if not a.startswith("--")]
        if not targets:
            print("nothing to check: pass --staged or some paths", file=sys.stderr)
            return 2

    blobs = load_cache(False)
    names = install_names()
    problems = []

    for rel in targets:
        rel = rel.strip().replace("\\", "/")
        if not rel:
            continue
        base = os.path.basename(rel).lower()
        ext = os.path.splitext(rel)[1].lower()

        if ext in SUSPECT_EXT:
            problems.append(f"{rel}: a game-file extension ({ext}) is never committed")
            continue
        if names and base in names:
            problems.append(f"{rel}: the name {base!r} exists in the install")
            continue
        if blobs:
            b = blob_name_of_file(os.path.join(ROOT, rel))
            if b and b in blobs:
                problems.append(f"{rel}: the CONTENT is identical to a file in the install")

    if problems:
        print("REFUSED: this commit would add a file from the Terraria install.\n")
        for p in problems:
            print(f"  {p}")
        print()
        print("A copy of a game file is never committed (see README, Legal section).")
        print("Keep it out of the tree, or add it to .gitignore.")
        return 1

    note = "" if blobs else " (content cache unavailable: name/extension check only)"
    print(f"clear: {len(targets)} path(s) checked{note}")
    return 0


sys.exit(main())
