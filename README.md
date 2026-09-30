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
- **The client and the server are separate sheet sets**, not one set plus a
  hand-maintained delta sheet: two binaries are two relations. They share 1,544
  of 1,547 type ids, so the platform split is a *query*
  (`sheetty overlap re/client/types re/server/types`), not a table (dec015).
- Both producers write into `sheets/re/{client,server}/**`; every row carries
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
  views.tsv             named column projections + row windows per view
  re/                   Mode B evidence sheets, one relation per binary
    client/             from Terraria.exe         7 sheets: types, methods,
                        callgraph, triage, functions, strings, types_pe
    server/             from TerrariaServer.exe   4 sheets: types, methods,
                        callgraph, triage
    assets.tsv          shared: both binaries load the same Content/
    sources.tsv         shared: producer + version pins
re/
  binaries/             checksummed working copy (never the only copy)
  exports/              regenerable, NOT committed
  ghidra_projects/      Ghidra project cache, NOT committed
  ghidra_scripts/       extraction scripts (committed)
  traces/               parity traces; commit hashes only
crates/
  sheetty/              parser, schema checker, overlap engine, emitter (incl. the port projection)
  sheetty-cli/          `sheetty preflight | overlap | report | view`
  terraria-demo/        proves the strut equation: sheets -> build.rs -> include!'d Rust
  terraria-server/      the server binary: a thin shim, deliberately almost empty
kernel/                 the hand-written Rust server: lib.rs, args.rs, boot.rs (listed in kernel.tsv)
                        owns the GENERATED port it includes at build time from $OUT_DIR
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
cargo run -p sheetty-cli -- overlap re/client/types re/server/types # the platform split
cargo run -p sheetty-cli -- order          # dependency order over the sheets
cargo run -p sheetty-cli -- rules          # which MDD checks this engine enforces
cargo run -p sheetty-cli -- emit --out target/generated --verify-determinism
cargo run -p sheetty-cli -- pack v_client_types          # assemble a view, check its budget
cargo run -p sheetty-cli -- pack v_server_types --rows 1..900  # ad-hoc row window
cargo test -p terraria-demo                # proves the emitted code compiles and is correct
cargo build -p terraria-server             # the Rust server: generated shape + hand-written kernel
cargo run -p terraria-server -- -savedirectory C:\saves
cargo test -p terraria-kernel              # 16 tests over the port and the kernel
powershell -File tools\test-preflight.ps1  # prove the rules actually fire
powershell -File tools\test-truncation.ps1 # prove the truncation guard fires
```

`preflight` states its own coverage, because "preflight passed" must never be
mistaken for "the whole MDD checklist passed". An absent rule is not a passing
one:

```
PREFLIGHT  sheets/  22 sheets, 161764 rows, 192 columns
  emitter 0.1.0
  rule coverage  29/30 MDD checks exist (implemented 22, partial 6, unexercised 1, absent 1)
  (run `sheetty rules` for the per-check status; an absent rule is not a passing one)
```

## Reproduce the evidence

```powershell
. .\tools\env.ps1            # pin GHIDRA_INSTALL_DIR, finite timeouts, DOTNET_ROLL_FORWARD
tools\extract-ghidra.ps1     # wait for analysis, then bulk export with shape guards
tools\extract-ilspy.ps1      # entity lists + full project decompile (one .cs per type)
tools\synth.ps1              # normalize exports -> sheets/re/{client,server}/*.tsv, then preflight
```

See `docs/PIPELINE.md` for exact commands, pinned versions, and the two ILSpy
calibrations that cost time to find (`-p` is required to get source; `-il`
emits the whole 125 MB assembly regardless of `-t`).

## Verified state

| Item | Value |
|---|---|
| target | `Terraria.exe` (client) `960a03bf...`; `TerrariaServer.exe` (server) `328872c6...`; both PE32 i386 .NET CLI assemblies |
| sheets | 24 (10 declaration/kernel + 8 client evidence + 5 server evidence + 2 shared, minus overlap) |
| rows | 229,260 |
| columns | 224 |
| preflight | 0 errors, 0 warnings; L3 = 16 covered / 0 unimplemented / 0 orphan |
| preflight rule coverage | **29 of 30** MDD checks exist (22 implemented, 6 partial, 1 unexercised, 1 absent) - run `sheetty rules` |
| rule tests | `tools/test-preflight.ps1` (8 diagnostics on a broken fixture), `tools/test-truncation.ps1` (truncation guard), `tools/test-rules-sweep.ps1` (31 codes) |
| client evidence (ILSpy) | 2,464 types, 14,517 methods, 3,602 edges, 30,057 members |
| server evidence (ILSpy) | 2,463 types, 14,486 methods, 3,601 edges, 30,040 members |
| client vs server | 2,456 shared, 6 client-only, 5 server-only, 2 divergent |
| Rust port (generated) | **compiles**: 2,463 types, 30,040 members, 14,486 methods, 504 placeholders, 64,040 lines |
| Rust kernel (hand-written) | `kernel/{lib,args,boot}.rs`, all listed in `kernel.tsv`; 16 tests |
| strings / assets | 21,081 strings; 15,135 asset refs, 15,123 verified on disk **with a real sha256 each** |
| Ghidra | 18,300 CLI **symbol records**, 68,268 PE data types - and **0 decoded instructions** |

## The client/server split

`Terraria.exe` and `TerrariaServer.exe` are two binaries, so they get two sheet
sets rather than one shared set plus a hand-maintained delta sheet. Duplicating
the inventories looks wasteful - the two share 1,544 type ids - but a delta is a
*copy of a derivable relation*, and a copy drifts. Instead the split is a query
over two real relations:

```
$ sheetty overlap re/client/types re/server/types
  covered        1544
  unimplemented  3    terraria.audio.mp3audiotrack, terraria.audio.oggaudiotrack,
                      terraria.testing.fxreader
  orphan         5    natupnplib.upnpnat, natupnplib.iupnpnat,
                      natupnplib.istaticportmapping,
                      natupnplib.istaticportmappingcollection,
                      terraria.properties.settings
  divergent      2    ~ terraria.initializers.chromainitializer.fields: 17 vs 11
                      ~ terraria.netplay.fields: 29 vs 31
```

The two `divergent` rows are the point of the whole exercise. An entity-name
comparison - which is all a delta sheet of type *names* can carry - only sees
that a type exists on both sides, and so reported "no gameplay type differs".
Comparing the two separated relations compares what the rows actually *say*, and
that found two conditionally compiled types: same name, 6 and 2 fields apart.
Neither can be ported once; both are now regression tests in
`crates/terraria-demo` (dec017).

Beyond those two, the shape is what you would predict: the server gains UPnP
port mapping (`natupnplib.*`) and `Properties.Settings`; the client gains its
audio decoders (`MP3AudioTrack`, `OGGAudioTrack`) and the FX pipeline
(`FxReader`).

Making the comparison tell the truth required excluding provenance columns from
it. `artifact` legitimately differs per platform (`ilspy/...` vs
`ilspy_server/...`), and comparing it reported `covered 0 / divergent 4640` for
1,546 identical types. Comparing provenance as data can never show agreement
between two producers (dec016); the reserved-column set is now explicit in the
overlap engine.

Ghidra has only analysed the client, so the server has no `functions`,
`strings` or `types_pe` sheets yet (dec015, t0007).

## The Rust port

`cargo build -p terraria-server` builds a Rust server whose shape is **generated**
from the sheet book. It is not typed by hand and it is not checked in: `sheetty
emit` writes it into `$OUT_DIR` on every build, so the sheets stay the only place
the server's shape is written down (D1). If preflight fails, the port does not
build at all (D5).

A Rust item is a **join of three relations**, which is why no single sheet could
project it:

| sheet | becomes |
|---|---|
| `re/server/types` | the item: `struct`, `enum`, `trait` or delegate marker, in a module tree mirroring the namespace |
| `re/server/fields` | the struct body, in **declaration order**, plus the `const`s and the enum discriminants |
| `re/server/methods` | the `impl` body, one `fn` per row, with `ret` and `params` mapped to Rust types |

The values are real, not placeholders: `ItemID::DirtBlock == 2` and
`Netplay::DefaultPort == 7777` come from the C# source through ILSpy into a sheet
row and out as a Rust constant, and the tests assert them. So does the negative
discriminant on `AchievementCategory::None`.

What the port is NOT: behaviour. Every generated body is `unimplemented!()`. It is
the server's shape at a fidelity the sheets can prove, and `terraria-server` says
so when it runs rather than printing a banner that implies it is serving:

```
$ cargo run -p terraria-server -- -savedirectory C:\saves
the port knows:
  listen port                  7777
  max connections              256
  first item id (DirtBlock)    2
  types projected              2463
NOT SERVING. This build is the projected SHAPE of the server.
```

Three things the projection does that are worth knowing, because each is a
recorded decision rather than an accident:

- **Unresolved references become visible placeholders.** 504 names are
  referenced by the server but not declared in it - the BCL, XNA, `IntPtr` - and
  each becomes a generated marker in `port::externs`. The port therefore always
  compiles, and the size of the not-yet-ported surface is a number rather than a
  silence.
- **Size cycles are broken by boxing exactly one edge.** A field whose type is its
  own struct, or the edge that closes a cycle, is boxed; the edge is chosen by a
  DFS over the field graph, so the other 2,400 types stay plain. C# class fields
  *are* references, so for those the `Box` is what the original means.
- **A constant we cannot state exactly is omitted, not approximated.** `1f / 60.0`
  is an expression; it is left out rather than guessed at, because a plausible
  wrong number in an ID table is worse than a missing one.

`kernel/` is the hand-written half: `args.rs` mirrors `Utils.ParseArguements`
(with one recorded divergence, dec020), and `boot.rs` mirrors
`Program.LaunchGame` as far as a stub port allows. Both are listed in
`sheets/kernel.tsv`, because D4 allows exactly two kinds of file: rows, and listed
kernel modules.

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
| `re/client/functions.tsv` | relabelled `status: symbol_only`; `size` is a metadata record length, not code size (dec013) |
| `re/client/triage.tsv` | **not** scored from Ghidra size, because that would be scoring nothing. Scored from ILSpy-measured fields and methods per type, which tops out at `Terraria.Player` (1316 fields, 865 methods), `Main`, `WorldGen` - which are Terraria's largest classes (dec013) |
| `re/client/callgraph.tsv` | Ghidra contributed no edges. Edges are derived from the decompiled C# declarations instead (dec013) |
| Ghidra program state | `find string ""` mutated it (strings 21159 to 29). Prefer explicit patterns; re-import before trusting state (dec014) |

Ghidra is still the right tool for what it *can* do here: PE structure, imports,
the 21,081-string table, and 68,268 data types recovered from CLI metadata.

## The emit stage (the actual thesis)

"Code volume is decoupled from token cost. The agent authors *rows*, not code."
This is where that is cashed. One row of `sheets/02-plan.tsv`:

```
core_math	subsystem	domain/core	crate::core::math	1	todo	Vector2/Rectangle/Utils ports
```

becomes a strut:

```rust
// GENERATED BY sheetty v0.1.0 FROM 02-plan - DO NOT EDIT
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Def {
    pub id: &'static str,
    pub kind: &'static str,
    pub sheet: &'static str,
    pub target: &'static str,
    pub priority: u8,
    pub status: &'static str,
    pub notes: &'static str,
}
```

and each row becomes a `Def { .. }` in a sorted `ALL` slice. `crates/terraria-demo`
`include!`s the output and its tests assert the projection is faithful:

```
$ cargo test -p terraria-demo
warning: terraria-demo@0.1.0: sheetty: 4 module(s), 4 written, 0 unchanged
warning: terraria-demo@0.1.0:   plan <- 02-plan (16 rows)
warning: terraria-demo@0.1.0:   re_client_types <- re/client/types (1549 rows)
warning: terraria-demo@0.1.0:   re_server_types <- re/server/types (1551 rows)
warning: terraria-demo@0.1.0:   registry <- (registry) (22 rows)

test tests::a_row_projects_to_a_def ... ok           # Terraria.Player, 1316 fields
test tests::the_two_conditional_compilation_divergences_are_visible ... ok
```

The doctrine this exercises, and how it was verified rather than asserted:

| Doctrine | Verified by |
|---|---|
| D2 one row, one strut | `re/client/types.tsv` 1549 rows -> 1549 `Def` values; `by_id("terraria.player").fields == 1316` |
| D5 emit gated on preflight | `sheetty emit --sheets tests/preflight-rules` refuses with 7 errors and writes nothing |
| D6 banner, never committed | `E-L0-GENERATED` fires when a generated file is planted in the tree |
| 5.9 one module per sheet | separate `plan.rs`, `re_client_types.rs`, `re_server_types.rs`, plus a counts-only `registry.rs` |
| 5.9 hash-gated writes | second `emit` run reports `0 written, 4 unchanged` |
| L7 determinism | `emit --verify-determinism` byte-compares a clean re-emit: 0 differing files |

## Context packs and views (why the evidence sheets are windowed)

`sheetty pack` assembles a declared view into a context pack: doctrine, then the
view's schema, then the projected rows, with a stable prefix hash recorded. The
first measurement answered the MDD's own open question Q6 ("how large can a sheet
get before it stops being a good context unit?") with a number:

| view | rows | est. tokens |
|---|---|---|
| `re/client/methods` unwindowed | 14,052 | **~379,526** |
| `re/client/strings` unwindowed | 21,081 | ~269,180 |
| `re/client/functions` unwindowed | 18,300 | ~267,598 |
| `re/assets` unwindowed | 15,135 | ~180,828 |

A whole evidence sheet is 1.9x a 200k context window on its own, so it is not a
usable context unit. The MDD's prescribed fix is a **row window**, not a smaller
sheet, "because splitting the sheet would break the key space". `sheets/views.tsv`
therefore declares both a column projection and a row window per view:

```
v_client_types   re/client/types   id,kind,namespace,name,fields,base   1..830   25000
v_server_types   re/server/types   id,kind,namespace,name,fields,base   1..830   25000
```

Windows are sized to fit the budget, and the budget is enforced on every
preflight as `E-L6-BUDGET` - including on ad-hoc `--rows` overrides, so the check
constrains real usage rather than nodding at a pre-blessed number.

## The truncation guard (MDD risk R6)

"Truncated extraction: a lost export silently halves the sheet" is the most
damaging Mode B failure, because nothing else notices - a synthesizer that loses
rows reports the smaller count with a straight face. Two rules guard it from
different angles, and both are proven by `tools/test-truncation.ps1`, which
actually truncates a committed sheet and restores it:

| Rule | Catches |
|---|---|
| `E-L5-DIVERGE` | the sheet no longer balances against its producer |
| `W-L7-ROWCOUNT` | the count moved badly against the last commit |

Every evidence sheet records how many records its producer emitted and how many
were deliberately dropped, and the two must **add up**:

```
# source_rows: 21159
# dropped: 78
# dropped_reason: text contains TAB or newline, which the canonical form forbids (dec008)
```

All twelve evidence sheets balance today:

| sheet | source_rows | rows | dropped |
|---|---|---|---|
| re/client/functions | 18,300 | 18,300 | 0 |
| re/client/strings | 21,159 | 21,081 | 78 |
| re/client/types_pe | 68,272 | 68,268 | 4 |
| re/client/types | 2,960 | 1,549 | 1,411 |
| re/client/methods | 14,052 | 14,052 | 0 |
| re/client/callgraph | 2,212 | 2,212 | 0 |
| re/client/triage | 1,549 | 1,549 | 0 |
| re/server/types | 2,958 | 1,551 | 1,407 |
| re/server/methods | 14,025 | 14,025 | 0 |
| re/server/callgraph | 2,211 | 2,211 | 0 |
| re/server/triage | 1,551 | 1,551 | 0 |
| re/assets | 15,135 | 15,135 | 0 |

The row count alone cannot catch a silent loss, because the loss changes the
count too. It is the *sum* that has to reconcile against an independently
recorded total.

## Legal (MDD §8.10)

Do **not** distribute decompiled code, reverse-engineered assets, or derivative
binaries. This work is local. `Terraria` is a trademark of Re-Logic; the
binary is used as an analysis subject only. See `sheets/decisions.tsv`.
