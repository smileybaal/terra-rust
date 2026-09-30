#!/usr/bin/env python3
"""Run a tools/*.ps1 script through powershell without depending on, or altering,
the machine's execution policy.

Why this exists. On a default Windows box the execution policy is Restricted, so
the documented

    powershell -File tools/test-preflight.ps1

dies with a raw SecurityError before a single line of the script is read:

    File ...\\tools\\test-preflight.ps1 cannot be loaded because running scripts
    is disabled on this system.

No script-side change can fix that, because the policy is consulted before the
file loads. Passing -ExecutionPolicy Bypass for the one process is the supported
way to run a trusted local script without touching machine or user policy, and it
is exactly what the gates in docs/PIPELINE.md already do.

    python tools/ps.py tools/test-preflight.ps1
    python tools/ps.py tools/test-preflight.ps1 -SomeArg

The child's exit code is this process's exit code, so a failing check still fails
whatever runs it. Docs: replace `powershell -File tools/<x>.ps1` with
`python tools/ps.py tools/<x>.ps1`.
"""
import os
import shutil
import subprocess
import sys


def main(argv):
    if not argv:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    script = argv[0]
    if not os.path.exists(script):
        print(f"no such script: {script}", file=sys.stderr)
        return 2
    if not script.lower().endswith(".ps1"):
        print(f"refusing to run {script!r}: expected a .ps1", file=sys.stderr)
        return 2

    pwsh = shutil.which("powershell") or shutil.which("pwsh")
    if pwsh is None:
        print("powershell not found on PATH", file=sys.stderr)
        return 2

    # cwd is left alone: every script sets its own working directory, and a caller
    # that wants a different one can cd first.
    r = subprocess.run(
        [pwsh, "-NoProfile", "-ExecutionPolicy", "Bypass",
         "-File", os.path.abspath(script), *argv[1:]]
    )
    return r.returncode


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
