# tools/test-preflight.ps1 - prove that preflight rules actually fire.
#
# A rule that has never been seen to fire is not a verified rule, and the MDD
# says as much: "A rule that never fires may be checking nothing." This script
# runs the engine against a fixture that deliberately breaks eight rules and
# asserts each expected diagnostic code appears.
#
# Exit 0 = every expected rule fired. Exit 1 = at least one did not.
#
# The engine is built (or refused) by tools/engine.ps1 before anything is asserted,
# so this cannot pass by exercising a stale target/debug/sheetty.exe.
#
# Running it: a default Windows box has an execution policy of Restricted, and
# `powershell -File tools/test-preflight.ps1` then dies with a raw policy error
# before a single line of this file is read. Nothing inside the script can change
# that - the policy is consulted before the file loads - so run it through the
# launcher, which does not depend on the machine's policy:
#
#     python tools/ps.py tools/test-preflight.ps1

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
. (Join-Path $PSScriptRoot "engine.ps1")

$fixture = "tests/preflight-rules"
$exe = Get-SheettyExe

Write-Host "== preflight against $fixture =="
$out = & $exe preflight --sheets $fixture 2>&1 | Out-String
Write-Host $out

# Each entry: code we expect, and why the fixture should trigger it.
$expected = @(
    @{ code = "E-L1-DEFAULT"; why = "column 'reload' declares default 'abc' which is not an f32" },
    @{ code = "E-L1-UNIT";    why = "'speed' is m/s in a.tsv and kph in b.tsv" },
    @{ code = "E-L2-CYCLE";   why = "a requires b and b requires a" },
    @{ code = "E-L2-TOPO";    why = "a cycle means no topological order exists" },
    @{ code = "E-L1-EMPTY";   why = "empty required cell handling" },
    @{ code = "E-L0-NOSCHEMA";why = "the fixture has no 01-schema.tsv" },
    @{ code = "W-L0-SHORT";   why = "a row omits trailing empty cells (dec004)" },
    @{ code = "E-L5-DIVERGE"; why = "c.tsv claims source_rows=100 with 0 dropped but has 2 rows" }
)

$missing = @()
foreach ($e in $expected) {
    if ($out -match [regex]::Escape($e.code)) {
        Write-Host ("  ok   {0,-14} {1}" -f $e.code, $e.why)
    } else {
        Write-Host ("  FAIL {0,-14} {1}" -f $e.code, $e.why) -ForegroundColor Red
        $missing += $e.code
    }
}

# A clean fixture must pass, otherwise the rules are noise.
Write-Host ""
Write-Host "== control: the real book must pass =="
& $exe preflight | Out-String | Write-Host
if ($LASTEXITCODE -ne 0) {
    Write-Host "  FAIL the real book does not pass preflight" -ForegroundColor Red
    exit 1
}
Write-Host "  ok   the real book passes preflight"

Write-Host ""
if ($missing.Count -gt 0) {
    Write-Host ("FAILED: {0} expected rule(s) did not fire: {1}" -f $missing.Count, ($missing -join ", ")) -ForegroundColor Red
    exit 1
}
Write-Host "PASSED: all $($expected.Count) expected diagnostics fired, and the real book is clean."
exit 0
