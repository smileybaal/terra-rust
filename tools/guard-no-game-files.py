"""Block a commit that would add a file from the Terraria install.

The scans prove the repository is clean today. This is what keeps it clean: a
pre-commit guard that refuses a commit the moment a game file is staged, rather
than relying on someone remembering to run the scans before every push.

It is deliberately cheap. Hashing the whole 804 MB install on every commit would be
unusable, so the install's blob names are cached once in .git/ (untracked, never
committed) and the guard only hashes the files being committed, which are few.

The install directory is found in this order, so a non-Steam install (docs/PIPELINE.md
tells the user to set CONTENT_DIR) is not silently checked by extension alone:

  TERRARIA_INSTALL_DIR   the install root, explicitly
  CONTENT_DIR            the install root, or <install>/Content (its parent is used)
  C:\\Steam\\steamapps\\common\\Terraria

Whenever the install names or the blob cache are unavailable the guard says so on
stderr and names which checks did NOT run, because a check that silently skips is
the failure this whole doctrine is about.

Under --staged the content check hashes both the bytes staged in the index (what
the commit will actually store) and the bytes on disk, because git normalises text
on the way into the index and either copy alone misses a renaming trick.

  python tools/guard-no-game-files.py --staged        # the pre-commit hook path
  python tools/guard-no-game-files.py PATH [PATH...]  # check specific files
  python tools/guard-no-game-files.py --rebuild       # refresh the cache

Exits 0 when clear, 1 with a named reason when not (or when --rebuild cannot build
the cache), and 2 when there is nothing to check (no --staged and no paths).
"""
import hashlib
import os
import subprocess
import sys


def _install_dir():
    """The install root, honouring the environment before the Steam default."""
    explicit = os.environ.get("TERRARIA_INSTALL_DIR")
    if explicit:
        return explicit
    content = os.environ.get("CONTENT_DIR")
    if content:
        norm = os.path.normpath(content)
        # docs/PIPELINE.md documents CONTENT_DIR as <install>/Content. Only strip
        # it when the parent is a real directory name, so CONTENT_DIR=C:\Content
        # cannot turn the install into C:\ and hash a whole drive.
        if os.path.basename(norm).lower() == "content":
            parent = os.path.dirname(norm)
            if parent and os.path.basename(parent):
                return parent
        return norm
    return r"C:\Steam\steamapps\common\Terraria"


ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
INSTALL = _install_dir()
CACHE = os.path.join(ROOT, ".git", "terraria-install-blobs.txt")

SUSPECT_EXT = {
    ".exe", ".dll", ".xnb", ".xwb", ".xsb", ".wav", ".ogg", ".mp3", ".pdb",
    ".png", ".jpg", ".jpeg", ".bmp", ".gif", ".fxc", ".so", ".dylib",
}


def blob_name_of_git_blob(rel):
    """The git name of the content STAGED at rel.

    A commit stores the index, so the bytes that will actually be committed have
    to be checked, not only the file on disk: `git add copy.tsv; del copy.tsv`
    used to pass the content check while the commit still added the game blob.
    """
    out = subprocess.run(
        ["git", "-C", ROOT, "show", f":{rel}"], capture_output=True
    )
    if out.returncode != 0:
        return None
    return blob_name_of_bytes(out.stdout)


def content_is_install_file(rel, blobs, staged):
    """True when the staged bytes OR the bytes on disk are a file from the install.

    Both are tested because they are different bytes and each catches what the
    other misses. The staged blob is what the commit adds; the working file is
    what it looked like before git normalised it, and `core.autocrlf=input` (the
    setting this repository pins, docs/PIPELINE.md) rewrites CRLF on the way into
    the index - so a CRLF text file copied from the install is byte-identical on
    disk and only a near-match once staged.
    """
    candidates = [blob_name_of_file(os.path.join(ROOT, rel))]
    if staged:
        candidates.append(blob_name_of_git_blob(rel))
    return any(c and c in blobs for c in candidates)


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
            try:
                blobs = set(f.read().split())
            except OSError:
                return None
        # An empty cache is not a cache. A concurrent --rebuild that truncated the
        # file would otherwise leave every later commit checking extensions only.
        return blobs or None
    if not os.path.isdir(INSTALL):
        return None  # cannot build; the guard degrades to the name check
    blobs = set()
    for dirpath, _, names in os.walk(INSTALL):
        for nm in names:
            b = blob_name_of_file(os.path.join(dirpath, nm))
            if b:
                blobs.add(b)
    os.makedirs(os.path.dirname(CACHE), exist_ok=True)
    # Write to a temporary file and rename, so two concurrent runs (or a commit
    # racing a manual --rebuild) can never see a half-written cache. Rename is
    # atomic on both Windows and POSIX.
    tmp = CACHE + ".tmp.%d" % os.getpid()
    with open(tmp, "w", encoding="utf-8") as f:
        f.write("\n".join(sorted(blobs)))
    try:
        os.replace(tmp, CACHE)
    except OSError:
        # Windows can refuse the rename while a reader holds the file open. A
        # direct write is worse (it truncates) but it still beats failing a commit.
        with open(CACHE, "w", encoding="utf-8") as f:
            f.write("\n".join(sorted(blobs)))
        os.remove(tmp)
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
        blobs = load_cache(True)
        if blobs is None:
            print(
                f"cannot rebuild the cache: no install directory at {INSTALL}\n"
                "set TERRARIA_INSTALL_DIR (or CONTENT_DIR) to the install root, or the\n"
                "guard will keep running without it",
                file=sys.stderr,
            )
            return 1
        print(f"cache rebuilt at {os.path.relpath(CACHE, ROOT)}: {len(blobs)} blob name(s)")
        return 0

    staged = "--staged" in sys.argv
    if staged:
        # -z: no path quoting and no newline splitting, so a name with a space,
        # a quote, a non-ASCII byte or a newline cannot silently break the list.
        diff = subprocess.run(
            ["git", "-C", ROOT, "diff", "--cached", "--name-only",
             "--diff-filter=ACM", "-z"],
            capture_output=True,
        )
        if diff.returncode != 0:
            # An unreadable index yields no paths, and no paths reads as "clear".
            print(
                "cannot read the staged paths (git diff failed): refusing to report clear\n"
                + diff.stderr.decode("utf-8", "replace").strip(),
                file=sys.stderr,
            )
            return 1
        targets = [os.fsdecode(p) for p in diff.stdout.split(b"\0") if p.strip()]
    else:
        targets = [a for a in args if not a.startswith("--")]
        if not targets:
            print("nothing to check: pass --staged or some paths", file=sys.stderr)
            return 2

    blobs = load_cache(False)
    names = install_names()

    # Say exactly which checks will not run. A guard that reports "clear" while
    # silently skipping the name and content checks is worse than no guard.
    skipped = [c for c, have in (("name", names), ("content", blobs)) if not have]
    if skipped:
        why = f"no install directory at {INSTALL}" if not names else \
              f"no blob cache at {os.path.relpath(CACHE, ROOT)}"
        print(
            f"warning: degraded guard, not checking: {', '.join(skipped)} ({why}). "
            f"Run --rebuild, or point TERRARIA_INSTALL_DIR (or CONTENT_DIR) at the install.",
            file=sys.stderr,
        )
    problems = []

    for rel in targets:
        rel = rel.strip().replace("\\", "/")
        if not rel:
            continue
        base = os.path.basename(rel).lower()
        ext = os.path.splitext(rel)[1].lower()

        # A path that is not there was not checked, and saying "clear" about it is
        # the silent skip this guard exists to avoid. Under --staged the content
        # comes from the index, so this only applies to explicit paths.
        if not staged and not (os.path.exists(rel) or os.path.exists(os.path.join(ROOT, rel))):
            problems.append(f"{rel}: no such file, so nothing about it was checked")
            continue

        if ext in SUSPECT_EXT:
            problems.append(f"{rel}: a game-file extension ({ext}) is never committed")
            continue
        if names and base in names:
            problems.append(f"{rel}: the name {base!r} exists in the install")
            continue
        if blobs and content_is_install_file(rel, blobs, staged):
            problems.append(f"{rel}: the CONTENT is identical to a file in the install")

    if problems:
        print("REFUSED: this commit would add a file from the Terraria install.\n")
        for p in problems:
            print(f"  {p}")
        print()
        print("A copy of a game file is never committed (see README, Legal section).")
        print("Keep it out of the tree, or add it to .gitignore.")
        return 1

    have = ["extension"] + (["name"] if names else []) + (["content"] if blobs else [])
    note = "" if not skipped else f"; {'/'.join(skipped)} check(s) unavailable"
    print(f"clear: {len(targets)} path(s) checked ({'+'.join(have)}{note})")
    return 0


sys.exit(main())
