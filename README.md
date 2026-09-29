# terraria-port

A **sheet book** for reverse-engineering and re-implementing Terraria, built to
`THE SPREADSHEET METHOD` (Master Design Document, v1.0).

The sheets are the source of truth. Code is a projection of the sheets. Nothing
is in this repository that is not a row (D4) or a listed kernel.

## Mode

**Mode B: Autonomous Decompile**, with one recorded deviation (see
`sheets/decisions.tsv`):

- `Terraria.exe` is a **managed .NET (CLI) assembly**, not native x86.
  Ghidra has no CIL decompiler, so the *managed* evidence (the actual game
  logic) is produced by **ILSpy** (`ilspycmd`), while Ghidra remains the
  producer of PE / import / resource / string / CLI-metadata-structure evidence.
- Both producers write into `sheets/re/**`; every row carries
  `ref_addr` + `ref_conf` + `evidence` (MDD §8.9, preflight L2).

## Layout

```
sheets/                 AUTHORITATIVE, committed, text only (TSV)
  00-doctrine.tsv       project identity, pinned toolchain, legal boundary
  01-schema.tsv         authoritative column declarations + views
  02-plan.tsv           specification relation (what must exist)
  03-impl.tsv           implementation relation (what exists, status)
  kernel.tsv            hand-written module allowlist (D4)
  work.tsv              swarm claim board
  tokens.tsv            token ledger
  decisions.tsv         recorded deviations from the MDD
  re/                   Mode B evidence sheets
re/
  binaries/             checksummed working copy (never the only copy)
  exports/              regenerable, NOT committed
  ghidra_projects/      Ghidra project cache, NOT committed
  ghidra_scripts/       extraction scripts (committed)
  traces/               parity traces; commit hashes only
crates/
  sheetty/              parser, schema checker, overlap engine, emitter
  sheetty-cli/          `sheetty preflight | overlap | report | view`
kernel/                 hand-written algorithmic code ONLY (listed in kernel.tsv)
docs/generated/         projected docs, never hand-edited
```

## Pipeline

```
ingest -> normalize -> validate -> graph -> emit -> compile -> verify
  |          |            |          |        |        |         |
 ILSpy    canonical   preflight   ref graph OUT_DIR  rustc    parity
 Ghidra    TSV          L0-L7      acyclic
```

`validate` (preflight) gates every build (D5). It cannot be skipped.

## Quick start

```
cargo run -p sheetty-cli -- preflight
cargo run -p sheetty-cli -- report unimplemented --rank blast-radius
cargo run -p sheetty-cli -- overlap 02-plan 03-impl --key id
```

## Reproduce the evidence

```powershell
. .\tools\env.ps1            # pin GHIDRA_INSTALL_DIR, finite timeouts, DOTNET_ROLL_FORWARD
tools\extract-ghidra.ps1     # wait for analysis, then bulk export with shape guards
tools\extract-ilspy.ps1      # entity lists + full project decompile (one .cs per type)
tools\synth.ps1              # normalize exports -> sheets/re/*.tsv, then preflight
```

See `docs/PIPELINE.md` for exact commands, pinned versions, and the two ILSpy
calibrations that cost time to find (`-p` is required to get source; `-il`
emits the whole 125 MB assembly regardless of `-t`).

## Verified state

| Item | Value |
|---|---|
| target | `Terraria.exe`, PE32 i386, .NET CLI assembly, sha256 `960a03bf...` |
| sheets | 17 |
| rows | 120,677 |
| columns | 146 |
| preflight | 0 errors, 0 warnings; L3 = 16 covered / 0 unimplemented / 0 orphan |
| native evidence | 18,300 functions, 68,268 PE types, 21,159 strings (Ghidra 12.1.4) |
| managed evidence | 1,549 types, 14,052 methods, 28,039 fields (ILSpy 9.1.0) |

## What Ghidra can and cannot do here

`Terraria.exe` is managed .NET. Ghidra has no CIL decompiler, and `ghidra-cli`
says so itself:

> This appears to be .NET managed code. Ghidra cannot decompile .NET IL bytecode.
> Consider using a .NET decompiler (e.g., ilspy-cli) for better results.

The split is deliberate and recorded (`dec003`, `dec012`): Ghidra produces PE,
import, resource, string and CLI-metadata-structure evidence; **ILSpy produces
the readable game logic**. Ghidra's auto-analysis still recovers real managed
method names on 18,300 functions, which is useful provenance even where the
decompiler output is not readable.

## Legal (MDD §8.10)

Do **not** distribute decompiled code, reverse-engineered assets, or derivative
binaries. This work is local. `Terraria` is a trademark of Re-Logic; the
binary is used as an analysis subject only. See `sheets/decisions.tsv`.
