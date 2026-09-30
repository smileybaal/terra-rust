# tools/engine.ps1 - resolve the sheetty engine the tests must run.
#
# Dot-source this from a test script:
#     . (Join-Path $PSScriptRoot "engine.ps1")
#     $exe = Get-SheettyExe
#
# The tests assert that preflight RULES fire. A prebuilt target/debug/sheetty.exe
# that is older than the sources makes such a test prove nothing: it passes while
# exercising an engine that no longer exists in the tree. That happened for real -
# test-truncation.ps1 passed with target/debug/sheetty.exe back-dated to 2020.
#
# So build first (up to date, cargo is ~0.2s), and if there is no cargo to build
# with, refuse to run at all rather than silently testing a stale binary.

function Get-SheettyExe {
    $exe = "target/debug/sheetty.exe"

    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if ($cargo) {
        & cargo build -q -p sheetty-cli
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build -p sheetty-cli failed; the engine does not build, so no rule can be trusted"
        }
    }

    if (-not (Test-Path $exe)) {
        throw "no $exe. Build it first: cargo build -p sheetty-cli"
    }

    if (-not $cargo) {
        # No toolchain to rebuild with, so the best available check is freshness.
        $sources = @()
        foreach ($p in @("crates/sheetty/src", "crates/sheetty-cli/src",
                         "crates/sheetty/Cargo.toml", "crates/sheetty-cli/Cargo.toml",
                         "Cargo.toml", "Cargo.lock")) {
            if (Test-Path $p) {
                $sources += Get-ChildItem $p -Recurse -File -ErrorAction SilentlyContinue
            }
        }
        if ($sources.Count -gt 0) {
            $newest = ($sources | Measure-Object -Property LastWriteTime -Maximum).Maximum
            if ((Get-Item $exe).LastWriteTime -lt $newest.AddSeconds(-2)) {
                throw ("$exe is older than its sources (newest: {0}); " -f $newest) +
                      "rebuild with cargo build -p sheetty-cli, or this test proves nothing"
            }
        }
    }

    return $exe
}
