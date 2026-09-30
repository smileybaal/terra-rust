# terra-rust

Reverse-engineering **Terraria** into a machine-checkable **sheet book**, and
projecting a **Rust server** out of it.

The sheets are the source of truth. Code is a projection of the sheets. Nothing is
in this repository that is not a row (D4) or a listed kernel module.

Built to `THE SPREADSHEET METHOD` (Master Design Document v1.0). The method exists
to decouple code volume from token cost: the agent authors *rows*, not code, and a
row projects mechanically to a strut.

**Where it stands:** the evidence is complete and preflight-clean, and the server
port **compiles** - 64,040 lines of Rust generated from 47,989 sheet rows. The
generated bodies are stubs: this is the server's *shape*, not its behaviour. See
[Status](#status-what-is-and-is-not-done) before reading further.

## Mode

**Mode B: Autonomous Decompile**, with recorded deviations (see
`sheets/decisions.tsv`).

- **Both binaries are managed .NET (CLI) assemblies**, not native x86. Ghidra has
  no CIL decompiler, so the *managed* evidence (the actual game logic) is produced
  by **ILSpy** (`ilspycmd`), while Ghidra remains the producer of PE / import /
  resource / string / CLI-metadata-structure evidence.
- **The client and the server are separate sheet sets**, not one set plus a
  hand-maintained delta sheet: two binaries are two relations. The split is a
  *query* (`sheetty overlap re/client/types re/server/types`), not a table (dec015).
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
  decisions.tsv         recorded deviations from the MDD (dec001-dec020)
  views.tsv             named column projections + row windows per view
  re/                   Mode B evidence sheets, one relation per binary
    client/             from Terraria.exe         8 sheets: types, fields,
                        methods, callgraph, triage, functions, strings, types_pe
    server/             from TerrariaServer.exe   5 sheets: types, fields,
                        methods, callgraph, triage
    assets.tsv          shared: both binaries load the same Content/
    sources.tsv         shared: producer + version pins
re/
  binaries/             checksum only, never the binary (see Legal)
  exports/              regenerable, NOT committed
  ghidra_projects/      Ghidra project cache, NOT committed
  ghidra_scripts/       extraction scripts (committed)
  traces/               parity traces; commit hashes only
crates/
  sheetty/              parser, schema checker, overlap engine, emitter (incl. the port projection)
  sheetty-cli/          `sheetty preflight | overlap | report | view | emit | pack`
  terraria-demo/        proves the strut equation: sheets -> build.rs -> include!'d Rust
  terraria-server/      the server binary: a thin shim, deliberately almost empty
kernel/                 the hand-written Rust server: lib.rs, args.rs, boot.rs (listed in kernel.tsv)
                        owns the GENERATED port it includes at build time from $OUT_DIR
tools/                  test harnesses and the managed-sheet invariant verifier
docs/                   pipeline notes; docs/generated/ is projected, never hand-edited
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
cargo run -p sheetty-cli -- overlap re/client/types re/server/types  # the platform split
cargo run -p sheetty-cli -- order          # dependency order over the sheets
cargo run -p sheetty-cli -- rules          # which MDD checks this engine enforces
cargo run -p sheetty-cli -- emit --out target/generated --verify-determinism
cargo run -p sheetty-cli -- pack v_client_types                 # assemble a view, check its budget
cargo run -p sheetty-cli -- pack v_server_types --rows 1..900   # ad-hoc row window

cargo build -p terraria-server             # the Rust server: generated shape + hand-written kernel
cargo run -p terraria-server -- -savedirectory C:\saves
cargo test -p terraria-kernel              # 16 tests over the port and the kernel
cargo test -p terraria-demo                # proves the emitted code compiles and is correct

python tools\ps.py tools\test-preflight.ps1  # prove the rules actually fire
python tools\ps.py tools\test-truncation.ps1 # prove the truncation guard fires
python tools\ps.py tools\test-rules-sweep.ps1# prove all 31 diagnostic codes fire
python tools\verify-managed-sheets.py      # cross-sheet invariants over the managed evidence
```

`preflight` states its own coverage, because "preflight passed" must never be
mistaken for "the whole MDD checklist passed". An absent rule is not a passing one:

```
PREFLIGHT  sheets/  24 sheets, 229260 rows, 224 columns
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
calibrations that cost time to find (`-p` is required to get source; `-il` emits the
whole 125 MB assembly regardless of `-t`).

## Verified state

| Item | Value |
|---|---|
| targets | `Terraria.exe` (client) `960a03bf...`; `TerrariaServer.exe` (server) `328872c6...`; both PE32 i386 .NET CLI assemblies |
| sheets | 24 |
| rows | 229,260 |
| columns | 224 |
| preflight | 0 errors, 0 warnings; L3 = 16 covered / 0 unimplemented / 0 orphan |
| preflight rule coverage | **29 of 30** MDD checks exist (22 implemented, 6 partial, 1 unexercised, 1 absent) - run `sheetty rules` |
| diagnostic coverage | 31 codes proven to fire by `tools/test-rules-sweep.ps1` against generated fixtures |
| client evidence (ILSpy) | 2,464 types, 30,057 members, 14,517 methods, 3,602 edges |
| server evidence (ILSpy) | 2,463 types, 30,040 members, 14,486 methods, 3,601 edges |
| client vs server | 2,456 shared, 6 client-only, 5 server-only, 2 divergent |
| Rust port (generated) | **compiles**: 2,463 types, 30,040 members, 14,486 methods, 504 placeholders, 64,040 lines |
| Rust kernel (hand-written) | `kernel/{lib,args,boot}.rs`, all listed in `kernel.tsv`; 16 tests |
| strings / assets | 21,081 strings; 15,135 asset refs, 15,123 verified on disk **with a real sha256 each** |
| Ghidra | 18,300 CLI **symbol records**, 68,268 PE data types - and **0 decoded instructions** |

## The Rust port

`cargo build -p terraria-server` builds a Rust server whose shape is **generated**
from the sheet book. It is not typed by hand and it is not checked in: `sheetty
emit` writes it into `$OUT_DIR` on every build, so the sheets stay the only place
the server's shape is written down (D1). If preflight fails, the port does not build
at all (D5).

A Rust item is a **join of three relations**, which is why no single sheet could
project it:

| sheet | becomes |
|---|---|
| `re/server/types` | the item: `struct`, `enum`, `trait` or delegate marker, in a module tree mirroring the namespace |
| `re/server/fields` | the struct body, in **declaration order**, plus the `const`s and the enum discriminants |
| `re/server/methods` | the `impl` body, one `fn` per row, with `ret` and `params` mapped to Rust types |

The values are real, not placeholders. `ItemID::DirtBlock == 2` and
`Netplay::DefaultPort == 7777` travel from the C# source through ILSpy into a sheet
row and out as a Rust constant, and the tests assert them. So does the negative
discriminant on `AchievementCategory::None == -1`. Nested types project as sibling
modules, which is why module names are lowercased: `mod player` and `struct Player`
coexist.

What the port is NOT: behaviour. Every generated body is `unimplemented!()`. It is
the server's shape at a fidelity the sheets can prove, and `terraria-server` says so
when it runs rather than printing a banner that implies it is serving:

```
$ cargo run -p terraria-server -- -savedirectory C:\saves
terraria-server (Rust port of TerrariaServer.exe)

launch parameters: 1
  -savedirectory = C:\saves

save directory: C:\saves
thread pool floor: 8

the port knows:
  listen port                  7777
  max connections              256
  net buffer size              1024
  first item id (DirtBlock)    2
  types projected              2463
  members projected            30040
  methods projected            14486
  inventory rows               2463

NOT SERVING. This build is the projected SHAPE of the server.
```

Three things the projection does that are worth knowing, because each is a recorded
decision rather than an accident:

- **Unresolved references become visible placeholders.** 504 names are referenced by
  the server but not declared in it - the BCL, XNA, `IntPtr` - and each becomes a
  generated marker in `port::externs`. The port therefore always compiles, and the
  size of the not-yet-ported surface is a number rather than a silence.
- **Size cycles are broken by boxing exactly one edge.** A field whose type is its
  own struct, or the edge that closes a cycle, is boxed; the edge is chosen by a DFS
  over the field graph, so the other ~2,400 types stay plain. C# class fields *are*
  references, so for those the `Box` is what the original means.
- **A constant we cannot state exactly is omitted, not approximated.** `1f / 60.0`
  is an expression; it is left out rather than guessed at, because a plausible wrong
  number in an ID table is worse than a missing one.

`kernel/` is the hand-written half. `args.rs` mirrors `Utils.ParseArguements` with
one recorded divergence (dec020: C# throws on a repeated flag, this keeps the first
value and reports the repeat). `boot.rs` mirrors `Program.LaunchGame` as far as a
stub port allows. Both are listed in `sheets/kernel.tsv`, because D4 allows exactly
two kinds of file: rows, and listed kernel modules.

## Status: what is and is not done

Done, and verified:

- Both binaries decompiled; the managed evidence normalised into 14 evidence sheets.
- `sheetty`: canonical TSV parser, L0-L7 preflight (29 of 30 MDD checks), overlap
  engine, blast-radius ranking, topological order, budgeted context packs, and the
  emitter.
- The client/server split derived by query rather than maintained by hand.
- The Rust server port generated from the sheets and **compiling**, with the
  hand-written kernel mirroring the real entry point.

Not done, and not pretended otherwise:

- **The port has no behaviour.** All 14,486 generated method bodies are stubs. The
  kernel is a boot report, not a server loop.
- **Ghidra analysed the client only.** The server has no `functions`, `strings` or
  `types_pe` sheets (dec015, t0007), so nothing in this repository says what
  `TerrariaServer.exe`'s native surface looks like.
- **No parity traces yet.** `re/traces/` is empty, so nothing here proves the port
  behaves like the original - only that it has the original's shape.
- **No client port.** Only the server is projected; the client is evidence only.
- **One MDD preflight check is absent** (dead-column detection, check 26) because a
  generic emitter consumes every column, so the rule could only ever pass. It is
  reported as absent rather than faked.

## The client/server split

`Terraria.exe` and `TerrariaServer.exe` are two binaries, so they get two sheet sets
rather than one shared set plus a hand-maintained delta sheet. Duplicating the
inventories looks wasteful - the two agree on the overwhelming majority of type ids -
but a delta is a *copy of a derivable relation*, and a copy drifts. Instead the split
is a query over two real relations:

```
$ sheetty overlap re/client/types re/server/types
  covered        2456
  unimplemented  6    terraria.audio.mp3audiotrack, terraria.audio.oggaudiotrack,
                      terraria.initializers.chromainitializer.eventlocalization,
                      terraria.testing.fxreader,
                      terraria.testing.fxreader.dummypipelinecontext,
                      terraria.testing.fxreader.pipelinelogger
  orphan         5    natupnplib.istaticportmapping,
                      natupnplib.istaticportmappingcollection, natupnplib.iupnpnat,
                      natupnplib.upnpnat, terraria.properties.settings
  divergent      2    ~ terraria.initializers.chromainitializer.fields: 15 vs 11
                      ~ terraria.netplay.fields: 28 vs 30
```

The two `divergent` rows are the point of the whole exercise. An entity-name
comparison - which is all a delta sheet of type *names* can carry - only sees that a
type exists on both sides, and so reported "no gameplay type differs". Comparing the
two separated relations compares what the rows actually *say*, and that found two
conditionally compiled types: same name, 4 and 2 members apart. Neither can be ported
once; both are regression tests in `crates/terraria-demo` (dec017).

Beyond those two, the shape is what you would predict: the server gains UPnP port
mapping (`natupnplib.*`) and `Properties.Settings`; the client gains its audio
decoders (`MP3AudioTrack`, `OGGAudioTrack`) and the FX pipeline (`FxReader`).

Making the comparison tell the truth required excluding provenance columns from it.
`artifact` legitimately differs per platform (`ilspy/...` vs `ilspy_server/...`), and
comparing it reported `covered 0 / divergent 4640` for 1,546 identical types.
Comparing provenance as data can never show agreement between two producers (dec016);
the reserved-column set is now explicit in the overlap engine.

## What Ghidra actually produced here (read this before trusting the sheets)

Both targets are managed .NET. Two separate limits follow, and both are measured
rather than assumed:

1. **Ghidra has no CIL decompiler.** `ghidra-cli` refuses the task itself:

   > This appears to be .NET managed code. Ghidra cannot decompile .NET IL bytecode.
   > Consider using a .NET decompiler (e.g., ilspy-cli) for better results.

2. **Ghidra decoded zero instructions.** `ghidra stats` reports `instructions: 0`;
   `ghidra disasm 0x00402051` fails with *"No instruction at address ... may be data
   or unanalyzed code"*; `ghidra graph calls` returns `edge_count: 0` over 18,299
   nodes. The 18,300 "functions" are names and addresses read out of the .NET
   metadata, **not analyzed code**.

Consequences, all recorded in `sheets/decisions.tsv`:

| | |
|---|---|
| `re/client/functions.tsv` | relabelled `status: symbol_only`; `size` is a metadata record length, not code size (dec013) |
| `re/client/triage.tsv` | **not** scored from Ghidra size, because that would be scoring nothing. Scored from ILSpy-measured fields and methods per type, which tops out at `Terraria.Player`, `Main` and `WorldGen` - Terraria's largest classes (dec013) |
| `re/client/callgraph.tsv` | Ghidra contributed no edges. Edges are derived from the decompiled C# declarations instead (dec013) |
| Ghidra program state | `find string ""` mutated it (strings 21159 to 29). Prefer explicit patterns; re-import before trusting state (dec014) |

Ghidra is still the right tool for what it *can* do here: PE structure, imports, the
21,081-string table, and 68,268 data types recovered from CLI metadata.

## The emit stage (the actual thesis)

"Code volume is decoupled from token cost. The agent authors *rows*, not code." This
is where that is cashed. One row of `sheets/02-plan.tsv`:

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
warning: terraria-demo@0.1.0: sheetty: 5 module(s), 5 written, 0 unchanged
warning: terraria-demo@0.1.0:   plan <- 02-plan (16 rows)
warning: terraria-demo@0.1.0:   re_client_types <- re/client/types (2464 rows)
warning: terraria-demo@0.1.0:   re_server_types <- re/server/types (2463 rows)
warning: terraria-demo@0.1.0:   registry <- (registry) (24 rows)
warning: terraria-demo@0.1.0:   port <- (port) (1 rows)

test tests::a_row_projects_to_a_def ... ok
test tests::the_two_conditional_compilation_divergences_are_visible ... ok
test tests::nested_types_are_rows_that_own_their_members ... ok
```

The doctrine this exercises, and how it was verified rather than asserted:

| Doctrine | Verified by |
|---|---|
| D2 one row, one strut | `re/client/types.tsv` 2,464 rows -> 2,464 `Def` values; `by_id("terraria.player").fields == 1313` |
| D5 emit gated on preflight | `sheetty emit --sheets tests/preflight-rules` refuses with 7 errors and writes nothing |
| D6 banner, never committed | `E-L0-GENERATED` fires when a generated file is planted in the tree |
| 5.9 one module per sheet | separate `plan.rs`, `re_client_types.rs`, `re_server_types.rs`, `registry.rs`, plus the multi-sheet `port.rs` |
| 5.9 hash-gated writes | a second `emit` run reports `0 written, N unchanged` |
| L7 determinism | `emit --verify-determinism` byte-compares a clean re-emit: 0 differing files |

## Context packs and views (why the evidence sheets are windowed)

`sheetty pack` assembles a declared view into a context pack: doctrine, then the
view's schema, then the projected rows, with a stable prefix hash recorded. The first
measurement answered the MDD's own open question Q6 ("how large can a sheet get
before it stops being a good context unit?") with a number:

| view | rows | est. tokens, unwindowed |
|---|---|---|
| `re/client/methods` | 14,517 | **~411,287** |
| `re/client/strings` | 21,081 | ~269,180 |
| `re/client/functions` | 18,300 | ~267,598 |
| `re/client/types` | 2,464 | ~69,488 |

A whole evidence sheet is more than twice a 200k context window on its own, so it is
not a usable context unit. The MDD's prescribed fix is a **row window**, not a
smaller sheet, "because splitting the sheet would break the key space".
`sheets/views.tsv` therefore declares both a column projection and a row window per
view:

```
v_client_types   re/client/types   id,kind,namespace,name,fields,base   1..760   25000
v_server_types   re/server/types   id,kind,namespace,name,fields,base   1..760   25000
```

Windows are sized to fit the budget, and the budget is enforced on every preflight as
`E-L6-BUDGET` - including on ad-hoc `--rows` overrides, so the check constrains real
usage rather than nodding at a pre-blessed number.

## The truncation guard (MDD risk R6)

"Truncated extraction: a lost export silently halves the sheet" is the most damaging
Mode B failure, because nothing else notices - a synthesizer that loses rows reports
the smaller count with a straight face. Two rules guard it from different angles, and
both are proven by `tools/test-truncation.ps1`, which actually truncates a committed
sheet and restores it:

| Rule | Catches |
|---|---|
| `E-L5-DIVERGE` | the sheet no longer balances against its producer |
| `W-L7-ROWCOUNT` | the count moved badly against the last commit |

Every evidence sheet records how many records its producer emitted and how many were
deliberately dropped, and the two must **add up**:

```
# source_rows: 21159
# dropped: 78
# dropped_reason: text contains TAB or newline, which the canonical form forbids (dec008)
```

All fourteen evidence sheets balance today:

| sheet | source_rows | rows | dropped |
|---|---|---|---|
| re/client/functions | 18,300 | 18,300 | 0 |
| re/client/strings | 21,159 | 21,081 | 78 |
| re/client/types_pe | 68,272 | 68,268 | 4 |
| re/client/types | 2,960 | 2,464 | 496 |
| re/client/fields | 63,098 | 30,057 | 33,041 |
| re/client/methods | 14,517 | 14,517 | 0 |
| re/client/callgraph | 3,602 | 3,602 | 0 |
| re/client/triage | 2,464 | 2,464 | 0 |
| re/server/types | 2,958 | 2,463 | 495 |
| re/server/fields | 63,037 | 30,040 | 32,997 |
| re/server/methods | 14,486 | 14,486 | 0 |
| re/server/callgraph | 3,601 | 3,601 | 0 |
| re/server/triage | 2,463 | 2,463 | 0 |
| re/assets | 15,135 | 15,135 | 0 |

The row count alone cannot catch a silent loss, because the loss changes the count
too. It is the *sum* that has to reconcile against an independently recorded total.

`re/*/fields` is the one sheet with a large `dropped`: the producer counts every line
inside a type body, and roughly half are braces, `get`/`set` accessors and initializer
statements that were never member declarations. The count is stated rather than
hidden, so the balance is checkable.

## Legal (MDD §8.10)

Do **not** distribute decompiled code, reverse-engineered assets, or derivative
binaries. This work is local, and **no source assets are committed**: the repository
holds text only - a checksum for the target binary, and sheet rows that *name* asset
paths with a sha256 each, but no asset bytes. The binaries and the decompiled trees
live in `re/binaries/` and `re/exports/`, both gitignored.

`Terraria` is a trademark of Re-Logic. The binary is used as an analysis subject
only, and no game code or asset is redistributed here.

### This is checked, not asserted

Two tools, both runnable on demand, both exiting non-zero when the claim fails.

`tools/check-history-for-game-files.py` answers the question that actually matters
for a push, which is *history*, not the working tree: a file committed once and
deleted later still ships. It compares by content rather than by filename - a git
blob's name is `sha1("blob <len>\0" + bytes)` - so all 16,019 files of
`C:\Steam\steamapps\common\Terraria` are hashed that way and intersected with every
object the repository contains. Current result:

```
221 blobs in the object database, 16,019 install files hashed
  install files that exist as git objects:  0
  paths ever added matching the install:    0
  paths ever added with a game extension:   0
HISTORY CONTAINS GAME FILES: NO
```

The largest blob in the whole history is 8.2 MB and it is
`sheets/re/server/fields.tsv` - a sheet this book generated. Nothing in the pack is
game-shaped.

`tools/find-game-file-copies.py` does the same for the working tree and then checks
that every match is *neutralised*: tracked (a failure, it must leave the index) or
untracked and not ignored (also a failure, it is one `git add -A` away). Exactly two
files match - `re/binaries/Terraria.exe` and `TerrariaServer.exe` - and both are
ignored, so none can be committed.

### And it cannot become untrue

`.githooks/pre-commit` runs the guard before every commit:

```
git config core.hooksPath .githooks
```

It hashes only the files being committed (the install's blob names are cached in
`.git/`, which is never committed), and it refuses with the reason:

```
REFUSED: this commit would add a file from the Terraria install.
  zz_leak_probe.bin: the CONTENT is identical to a file in the install
```

That message is from a real test: a game file was copied in under a name the
install does not use, so the name and extension checks could not see it, and the
content check refused the commit anyway.

### What IS in here, plainly

The repository contains no original game file and no asset bytes. It does contain
material *derived* from the binaries, and the distinction is worth stating rather
than blurring: `re/client/strings.tsv` holds 21,081 strings recovered from the
executable, `re/*/functions.tsv` holds symbol names, `re/client/types_pe.tsv` holds
68,268 metadata type names, and the `fields` sheets hold constant *values* (which is
why `ItemID::DirtBlock == 2` compiles). Those are rows describing the binary, not
copies of it, and they are the evidence the whole method is built on. If you would
not publish extracted string tables and decompiled type structure, then this
repository is not publishable either - but no file in it is a game file.

