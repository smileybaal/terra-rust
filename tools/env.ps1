# tools/env.ps1 - pin the toolchain for this project (MDD 8.9 bounded autonomy).
# Dot-source this before any extraction step:  . .\tools\env.ps1

$ErrorActionPreference = "Stop"

$env:GHIDRA_INSTALL_DIR = "C:/Users/PORTMANTEAU/Desktop/Misc/ghidra_12.1.4_PUBLIC"

# Finite timeouts so a runaway analysis returns to the agent instead of
# hanging the swarm. This deliberately inverts ghidra-cli's unbounded default
# and belongs in the project env file (MDD 8.3).
$env:GHIDRA_CLI_OP_TIMEOUT = "1800"
$env:GHIDRA_CLI_DECOMPILE_TIMEOUT = "120"
$env:GHIDRA_CLI_LAUNCH_TIMEOUT = "180"

# ilspycmd 9.1 targets .NET 8; the machine has 6/7/9 installed, so roll forward.
$env:DOTNET_ROLL_FORWARD = "LatestMajor"
$env:DOTNET_CLI_TELEMETRY_OPTOUT = "1"

$root = Split-Path -Parent $PSScriptRoot
Write-Host "terraria-port root: $root"
Write-Host "GHIDRA_INSTALL_DIR: $env:GHIDRA_INSTALL_DIR"
