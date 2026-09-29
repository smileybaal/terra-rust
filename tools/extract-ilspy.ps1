# tools/extract-ilspy.ps1 - managed decompilation with ILSpy.
#
# dec003: Terraria.exe is a CLI/.NET assembly, so ILSpy is the managed-evidence
# producer and Ghidra supplies PE/metadata evidence only.

. "$PSScriptRoot\env.ps1"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$exe = "re/binaries/Terraria.exe"
if (-not (Test-Path $exe)) { throw "missing $exe; copy the binary and hash it first" }

Write-Host "== entity inventory =="
foreach ($k in @("c", "s", "e", "i", "d")) {
    $f = "re/exports/ilspy_entities_$k.txt"
    ilspycmd --disable-updatecheck -l $k $exe > $f
    $n = (Get-Content $f | Measure-Object -Line).Lines
    Write-Host ("  {0}: {1} entities" -f $k, $n)
    if ($n -lt 1) { throw "empty entity list for $k" }
}

Write-Host "== full project decompile (one .cs per type) =="
# -p is required: without it ilspycmd -o writes only payload files and no source.
ilspycmd --disable-updatecheck -p --nested-directories -o re/exports/ilspy $exe

$tree = "re/exports/ilspy"
$cs = (Get-ChildItem -Recurse -Filter *.cs $tree | Measure-Object).Count
Write-Host "  $cs .cs files"
if ($cs -lt 100) { throw "decompiled tree looks too small ($cs files)" }

Write-Host "== done: run tools/synth.ps1 next =="
