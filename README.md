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
| rows | 142,357 |
| columns | 147 |
| preflight | 0 errors, 0 warnings; L3 = 16 covered / 0 unimplemented / 0 orphan |
| preflight rule coverage | **19 of 30** MDD checks exist (13 implemented, 5 partial, 1 unexercised, 11 absent) |
| managed evidence (ILSpy) | 1,549 types, 14,052 methods, 28,039 fields, 715 base/interface links |
| managed dependency edges | 2,212 (1,545 `uses`, 667 `inherits`), derived from C# declarations |
| strings / assets | 21,081 strings; 15,135 asset refs, 15,123 verified on disk |
| Ghidra | 18,300 CLI **symbol records**, 68,268 PE data types - and **0 decoded instructions** |

## What Ghidra actually produced here (read this before trusting the sheets)

`Terraria.exe` is managed .NET. Two separate limits follow, and both are
measured rather than assumed:

1. **Ghidra has no CIL decompiler.** `ghidra-cli` refuses the task itself:

   > This appears to be .NET managed code. Ghidra cannot decompile .NET IL bytecode.
   > Consider using a .NET decompiler (e.g., ilspy-cli) for better results.

2. **Ghidra decoded zero instructions.** `ghidra stats` reports
   `instructions: 0`; `ghidra disasm 0x00402051` fails with *"No instruction at
   address ... may be data or unanalyzed code"*; `ghidra graph calls` returns
   `edge_count: 0` over 18,299 nodes. The 18,300 "functions" are names and
   addresses read out of the .NET metadata, **not analyzed code**.

Consequences, all recorded in `sheets/decisions.tsv`:

| | |
|---|---|
| `re/functions.tsv` | relabelled `status: symbol_only`; `size` is a metadata record length, not code size (dec013) |
| `re/triage.tsv` | **not** scored from Ghidra size, because that would be scoring nothing. Scored from ILSpy-measured fields and methods per type, which tops out at `Terraria.Player` (1316 fields, 865 methods), `Main`, `WorldGen` - which are Terraria's largest classes (dec013) |
| `re/callgraph.tsv` | Ghidra contributed no edges. Edges are derived from the decompiled C# declarations instead (dec013) |
| Ghidra program state | `find string ""` mutated it (strings 21159 to 29). Prefer explicit patterns; re-import before trusting state (dec014) |

Ghidra is still the right tool for what it *can* do here: PE structure, imports,
the 21,081-string table, and 68,268 data types recovered from CLI metadata.

## Legal (MDD §8.10)

Do **not** distribute decompiled code, reverse-engineered assets, or derivative
binaries. This work is local. `Terraria` is a trademark of Re-Logic; the
binary is used as an analysis subject only. See `sheets/decisions.tsv`.
