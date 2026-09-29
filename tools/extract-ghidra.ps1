# tools/extract-ghidra.ps1 - ingest + bulk export with --expect guards.
#
# MDD 8.4: "Bulk extraction belongs in a script, not in N CLI invocations."
# MDD O21: every producer of an artifact should assert the artifact's shape at
# the moment of production. --expect is that assertion.

. "$PSScriptRoot\env.ps1"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$project = "terraria"
$program = "Terraria"
$out = "re/exports/ghidra"
New-Item -ItemType Directory -Force -Path $out | Out-Null

function Need-File {
    param([string]$Path, [int]$MinBytes)
    if (-not (Test-Path $Path)) {
        throw "expected artifact missing: $Path"
    }
    $len = (Get-Item $Path).Length
    if ($len -lt $MinBytes) {
        throw "artifact short: $Path is $len bytes, expected >= $MinBytes"
    }
    Write-Host ("  ok {0} ({1:N0} bytes)" -f $Path, $len)
}

Write-Host "== status =="
ghidra status --project $project

Write-Host "== waiting for analysis to finish =="
while ($true) {
    $j = ghidra jobs --project $project | ConvertFrom-Json
    if ($null -eq $j.active_job) { break }
    Write-Host ("  {0} ({1:P0}) {2}" -f $j.active_job.command,
        ($(if ($j.active_job.maximum -gt 0) { $j.active_job.progress / $j.active_job.maximum } else { 0 })),
        $j.active_job.progress_message)
    Start-Sleep -Seconds 15
}
Write-Host "  analysis idle"

Write-Host "== bulk export =="
ghidra function list --project $project --json --fields name,address,size > "$out/functions.json"
Need-File "$out/functions.json" 500000

ghidra symbol list --project $project --json > "$out/symbols.json"
Need-File "$out/symbols.json" 100000

ghidra type list --project $project --json > "$out/types.json"
Need-File "$out/types.json" 100000

ghidra find interesting --project $project --json > "$out/interesting.json"
Need-File "$out/interesting.json" 10

ghidra find crypto --project $project --json > "$out/crypto.json"

ghidra comment list --project $project --json > "$out/comments.json"

Write-Host "== summary =="
ghidra summary --project $project
ghidra stats --project $project

Write-Host "== done: run tools/synth.ps1 next =="
