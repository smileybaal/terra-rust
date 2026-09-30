# tools/test-truncation.ps1 - prove the truncation guard fires.
#
# MDD risk R6 is "truncated extraction: a lost export silently halves the sheet",
# which is the most damaging operational failure in Mode B because nothing else
# notices. Two rules guard it from different angles:
#   W-L7-ROWCOUNT  the count moved badly against the last commit (check 28)
#   E-L5-DIVERGE   the sheet no longer balances against its producer (check 24)
#
# The client's triage sheet is used because it is large (1549 rows) and committed.
# Any sheet with a committed baseline works; if it moves, update $target.
#
# This truncates a real workbook sheet, asserts W-L7-ROWCOUNT fires, and restores
# the file. It writes LF without a BOM, because writing with a BOM or CRLF would
# corrupt the canonical form and the test would fail for the wrong reason - which
# is exactly what happened the first time this was written.
#
# Exit 0 = the guard fired and the sheet was restored cleanly.

$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

$target = "sheets/re/client/triage.tsv"
$bak = "re/exports/_trunc.bak"

if (-not (Test-Path $target)) { throw "missing $target" }

# The guard reads its baseline from git (`git show HEAD:sheets/<rel>`), and degrades
# silently when the sheet is not yet committed - which is correct in the engine (a
# missing baseline is not a defect) but wrong here, because the test would then
# "pass" by exercising nothing. This happened for real: the guard did not fire
# while the sheet was still untracked. Assert the precondition loudly instead.
& git show "HEAD:$($target -replace '\\', '/')" *> $null
if ($LASTEXITCODE -ne 0) {
    throw "no committed baseline for $target; commit it first, or this test proves nothing"
}
Copy-Item $target $bak -Force

try {
    $all = [System.IO.File]::ReadAllText($target) -split "`n"
    $head = $all | Where-Object { $_ -match '^#' }
    $data = $all | Where-Object { $_ -ne "" -and $_ -notmatch '^#' }
    $hdr = $data[0]
    $body = $data | Select-Object -Skip 1 -First 100

    $text = (($head + $hdr + $body) -join "`n") + "`n"
    [System.IO.File]::WriteAllText($target, $text, (New-Object System.Text.UTF8Encoding($false)))
    Write-Host "truncated $target to $($body.Count) data rows (was $($data.Count - 1))"

    $out = & target/debug/sheetty.exe preflight 2>&1 | Out-String

    $expect = @("W-L7-ROWCOUNT", "E-L5-DIVERGE")
    $missing = @()
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
    Copy-Item $bak $target -Force
    Remove-Item $bak -Force
    Write-Host "restored $target"
}

# A control: the restored book must be clean, or the guard is worthless noise.
$after = & target/debug/sheetty.exe preflight 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) {
    Write-Host "FAIL restored book does not pass preflight" -ForegroundColor Red
    Write-Host $after
    exit 1
}
Write-Host "  ok   restored book passes preflight"

if ($missing.Count -gt 0) {
    Write-Host ("FAILED: {0} guard(s) did not fire: {1}" -f $missing.Count, ($missing -join ", ")) -ForegroundColor Red
    exit 1
}
Write-Host "PASSED: the truncation guard fired and the book was restored clean."
exit 0
