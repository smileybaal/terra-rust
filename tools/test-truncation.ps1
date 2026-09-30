# tools/test-truncation.ps1 - prove the truncation guard fires.
#
# MDD risk R6 is "truncated extraction: a lost export silently halves the sheet",
# which is the most damaging operational failure in Mode B because nothing else
# notices. Two rules guard it from different angles:
#   W-L7-ROWCOUNT  the count moved badly against the last commit (check 28)
#   E-L5-DIVERGE   the sheet no longer balances against its producer (check 24)
#
# The client's triage sheet is used because it is large and committed (2464 rows
# when this was last touched). Any sheet with a committed baseline works; if it
# moves, update $target.
#
# Two things this script must NOT do, both of which it used to:
#
#  1. Run a stale engine. It asserted diagnostics from whatever
#     target/debug/sheetty.exe happened to be on disk, so back-dating that binary
#     to 2020 still produced PASSED. Get-SheettyExe (tools/engine.ps1) builds the
#     engine and refuses to continue without it.
#  2. Write to the committed sheet. It truncated sheets/re/client/triage.tsv in
#     place and restored it in a finally, which a hard kill (Ctrl-C, timeout, a
#     killed agent) skipped - leaving the committed sheet truncated in the working
#     tree, recoverable only with a git write. The truncation now happens on a
#     copy under re/exports/ (gitignored), so there is nothing to restore and no
#     state a kill can corrupt. The last check proves the committed sheet is
#     byte-identical to HEAD afterwards.
#
# The truncation is written as LF without a BOM, because writing with a BOM or CRLF
# would corrupt the canonical form and the test would fail for the wrong reason -
# which is exactly what happened the first time this was written.
#
# Exit 0 = the guard fired and the committed book is untouched.

$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)
. (Join-Path $PSScriptRoot "engine.ps1")

$target = "sheets/re/client/triage.tsv"
# Per-run scratch directory, under the gitignored re/exports/: two concurrent runs
# (or a run racing a kill) must not share a tree, and whatever is left behind there
# is invisible to git.
$scratch = "re/exports/_trunc-$PID"
$sheets = "$scratch/sheets"
$rel = $target -replace '\\', '/'

$exe = Get-SheettyExe

if (-not (Test-Path $target)) { throw "missing $target" }

# The guard reads its baseline from git (`git show HEAD:sheets/<rel>`), and degrades
# silently when the sheet is not yet committed - correct in the engine (a missing
# baseline is not a defect) but wrong here, because the test would then "pass" by
# exercising nothing. This happened for real: the guard did not fire while the
# sheet was still untracked. Assert the precondition loudly instead.
$committedBlob = (& git rev-parse "HEAD:$rel").Trim()
if ($LASTEXITCODE -ne 0 -or -not $committedBlob) {
    throw "no committed baseline for $target; commit it first, or this test proves nothing"
}

# Mirror the book into the ignored scratch directory. The mirror keeps the
# sheets/<rel> layout, because the row-count baseline is looked up by that path.
if (Test-Path $scratch) { Remove-Item $scratch -Recurse -Force }
New-Item -ItemType Directory -Path $scratch -Force | Out-Null
Copy-Item -Path "sheets" -Destination $sheets -Recurse -Force

$missing = @()
try {
    $victim = "$sheets/re/client/triage.tsv"
    $all = [System.IO.File]::ReadAllText($victim) -split "`n"
    $head = $all | Where-Object { $_ -match '^#' }
    $data = $all | Where-Object { $_ -ne "" -and $_ -notmatch '^#' }
    $hdr = $data[0]
    $body = $data | Select-Object -Skip 1 -First 100

    $text = (($head + $hdr + $body) -join "`n") + "`n"
    [System.IO.File]::WriteAllText($victim, $text, (New-Object System.Text.UTF8Encoding($false)))

    # Assert the copy really is truncated, so a file that failed to shrink cannot
    # be mistaken for a passing guard (the codes below would not fire, but say why).
    $now = ([System.IO.File]::ReadAllText($victim) -split "`n" |
            Where-Object { $_ -ne "" -and $_ -notmatch '^#' }).Count - 1
    if ($now -ne $body.Count) {
        throw "the scratch copy has $now data rows, expected $($body.Count)"
    }
    Write-Host "truncated a copy at $victim to $($body.Count) data rows (was $($data.Count - 1))"

    $out = & $exe preflight --sheets $sheets 2>&1 | Out-String

    $expect = @("W-L7-ROWCOUNT", "E-L5-DIVERGE")
    foreach ($code in $expect) {
        if ($out -match [regex]::Escape($code)) {
            Write-Host "  ok   $code fired on truncation"
        } else {
            Write-Host "  FAIL $code did not fire on truncation" -ForegroundColor Red
            $missing += $code
        }
    }
    ($out -split "`n") | Where-Object { $_ -match "ROWCOUNT|DIVERGE|balances" } | ForEach-Object { Write-Host "       $_" }
}
finally {
    if (Test-Path $scratch) { Remove-Item $scratch -Recurse -Force -ErrorAction SilentlyContinue }
}

# A control: the committed book must be clean, or the guard is worthless noise.
$after = & $exe preflight 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) {
    Write-Host "FAIL the real book does not pass preflight" -ForegroundColor Red
    Write-Host $after
    exit 1
}
Write-Host "  ok   the real book passes preflight"

# The committed sheet must be exactly what HEAD says it is. A test that can leave
# the working tree dirty is a defect in the test, not a tolerable side effect.
$afterBlob = (& git hash-object $target).Trim()
if ($afterBlob -ne $committedBlob) {
    Write-Host "FAIL $target differs from HEAD after the run - the test wrote to the committed tree" -ForegroundColor Red
    exit 1
}
Write-Host "  ok   $target is byte-identical to HEAD"

if ($missing.Count -gt 0) {
    Write-Host ("FAILED: {0} guard(s) did not fire: {1}" -f $missing.Count, ($missing -join ", ")) -ForegroundColor Red
    exit 1
}
Write-Host "PASSED: the truncation guard fired and the committed book was never touched."
exit 0
