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

```
tools/setup.ps1            # env: GHIDRA_INSTALL_DIR, timeouts, DOTNET_ROLL_FORWARD
tools/extract-ghidra.ps1   # ghidra import + bulk export (--expect guarded)
tools/extract-ilspy.ps1    # ilspycmd entity lists + full project decompile
tools/synth.ps1            # exports -> sheets/re/*.tsv (canonical form)
```

## Legal (MDD §8.10)

Do **not** distribute decompiled code, reverse-engineered assets, or derivative
binaries. This work is local. `Terraria` is a trademark of Re-Logic; the
binary is used as an analysis subject only. See `sheets/decisions.tsv`.
