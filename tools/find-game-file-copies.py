"""Find any file in terraria-port that matches any file in the Terraria install.

The rule is: a copy of a game file is never committed, and ideally never present.
Three kinds of match are reported, weakest first, because "matches" should not be
left to one interpretation:

  PATH    the same lowercased relative path, e.g. Content/Images/Foo.xnb
  NAME    the same lowercased basename anywhere in the tree
  HASH    the same sha256 with any name at all

HASH is the one that actually matters legally - a renamed copy is still a copy -
so it is computed on both sides rather than guessed at from extensions.

  python tools/find-game-file-copies.py [--repo .] [--install DIR]
"""
import argparse
import hashlib
import os
import sys

DEFAULT_INSTALL = r"C:\Steam\steamapps\common\Terraria"
SKIP_DIRS = {".git", "target", "node_modules", "__pycache__"}


def sha256(path, limit=64 * 1024 * 1024):
    """Hash a file, skipping anything implausibly large for a copy."""
    try:
        if os.path.getsize(path) > limit:
            return None
        h = hashlib.sha256()
        with open(path, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
        return h.hexdigest()
    except OSError:
        return None


def walk(root, skip):
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in skip]
        for n in filenames:
            yield os.path.join(dirpath, n)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=".")
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--no-hash", action="store_true")
    a = ap.parse_args()

    repo = os.path.abspath(a.repo)
    install = os.path.abspath(a.install)
    if not os.path.isdir(install):
        print(f"install directory not found: {install}", file=sys.stderr)
        return 2

    # --- the install ------------------------------------------------------
    game_paths = {}
    game_names = {}
    game_hashes = {}
    n = 0
    for p in walk(install, set()):
        n += 1
        rel = os.path.relpath(p, install).replace("\\", "/").lower()
        game_paths.setdefault(rel, p)
        game_names.setdefault(os.path.basename(rel), p)
        if not a.no_hash:
            h = sha256(p)
            if h:
                game_hashes.setdefault(h, p)
    print(f"install: {n} files, {len(game_hashes)} hashed")

    # --- the repo ---------------------------------------------------------
    hits = {"PATH": [], "NAME": [], "HASH": []}
    n_repo = 0
    for p in walk(repo, SKIP_DIRS):
        n_repo += 1
        rel = os.path.relpath(p, repo).replace("\\", "/").lower()
        base = os.path.basename(rel)
        if rel in game_paths:
            hits["PATH"].append((os.path.relpath(p, repo), game_paths[rel]))
        elif base in game_names:
            hits["NAME"].append((os.path.relpath(p, repo), game_names[base]))
        if not a.no_hash:
            h = sha256(p)
            if h and h in game_hashes:
                src = game_hashes[h]
                if not any(os.path.relpath(p, repo) == x[0] for x in hits["PATH"]):
                    hits["HASH"].append((os.path.relpath(p, repo), src))
    print(f"repo:    {n_repo} files\n")

    # --- is each match actually neutralised? -------------------------------
    # Finding a copy is not the point; the point is that it cannot be committed.
    # A copy that is TRACKED has to be removed from the index, and a copy that is
    # untracked but NOT ignored is one `git add -A` away from being committed, so
    # both are failures. Only an ignored copy is safe to leave on disk.
    def git(*args):
        import subprocess

        r = subprocess.run(
            ["git", "-C", repo, *args], capture_output=True, text=True
        )
        return r.returncode

    total = 0
    unsafe = 0
    for kind in ("PATH", "NAME", "HASH"):
        rows = hits[kind]
        total += len(rows)
        print(f"== {kind} matches: {len(rows)} ==")
        for got, src in rows[:80]:
            tracked = git("ls-files", "--error-unmatch", "--", got) == 0
            ignored = git("check-ignore", "-q", "--", got) == 0
            if tracked:
                state, bad = "TRACKED - must be removed from the index", True
            elif ignored:
                state, bad = "ignored - cannot be committed", False
            else:
                state, bad = "NOT IGNORED - one git add away", True
            if bad:
                unsafe += 1
            print(f"   [{state}]")
            print(f"   {got}")
            print(f"      <- {os.path.relpath(src, install)}")
        if len(rows) > 80:
            print(f"   ... +{len(rows) - 80} more")
        print()

    print("TOTAL MATCHES:", total)
    print("MATCHES THAT COULD STILL BE COMMITTED:", unsafe)
    return 1 if unsafe else 0


sys.exit(main())
