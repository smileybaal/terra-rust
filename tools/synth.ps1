# tools/synth.ps1 - normalize raw producer output into canonical evidence sheets.
# This is the [normalize] stage of the pipeline (MDD 5.1).

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

Write-Host "== normalize: ghidra exports -> sheets =="
python re/ghidra_scripts/synth_ghidra.py
if ($LASTEXITCODE -ne 0) { throw "synth_ghidra failed" }

Write-Host "== normalize: ilspy exports -> sheets =="
python re/ghidra_scripts/synth_ilspy.py
if ($LASTEXITCODE -ne 0) { throw "synth_ilspy failed" }

Write-Host "== validate: preflight (D5, gates every build) =="
cargo run -q -p sheetty-cli -- preflight
if ($LASTEXITCODE -ne 0) {
    Write-Warning "preflight reported errors; see the report above"
    exit $LASTEXITCODE
}

Write-Host "== done =="
