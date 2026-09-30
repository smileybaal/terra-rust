# The `sheetty` engine

`sheetty` is the tool that makes the sheets the source of truth rather than a
document that describes one. It parses the TSV book, checks it against the MDD
preflight checklist, answers relational queries over it, and projects the parts
marked `# emit: rust` into Rust. Everything else in this repository is either a
sheet, a row, or a hand-written kernel module listed in `sheets/kernel.tsv`.

This document describes the engine as it is, not as it should be. Where a rule is
partial, absent, or cannot fire, it says so and says why: an engine that reports
"preflight passed" while silently checking less than it claims is the failure mode
the whole project is built to avoid.

```
cargo build -p sheetty-cli          # the engine is a crate in this workspace
target/debug/sheetty.exe <command>
```

---

## 1. Commands

| command | what it does | gates a build? |
|---|---|---|
| `preflight` | parse the book, run the L0-L7 checks, print findings | **yes** (D5) |
| `overlap A B` | compare two sheets over a shared key; report covered / unimplemented / orphan / divergent | no |
| `report unimplemented` | rank the spec units that have no implementation, by blast radius | no |
| `view SHEET` | print a sheet as TSV, with a column projection and a row window | no |
| `schema` | print the authoritative column declarations, per sheet | no |
| `rules` | print which MDD checks this engine implements, and how | no |
| `order` | print a topological order of the sheets (dependencies first) | no |
| `emit` | project the `# emit: rust` sheets into `$OUT_DIR` (or `--out`) | yes, via `build.rs` |
| `pack` | assemble a view into a context pack and check its token budget | no |

### Exit codes

| code | meaning |
|---|---|
| 0 | clean: no errors (and, with `--strict`, no warnings) |
| 1 | errors present - a gate. `preflight` returning 1 fails the build (D5) |
| 2 | usage error, or the engine could not check at all |

A gate that cannot distinguish "checked and clean" from "could not check" is not a
gate, which is why the third code exists rather than folding every failure into 1.
Several tools in `tools/` follow the same convention; the one that matters most is
`check-readme-numbers.py`, which exits 2 rather than skipping a figure it cannot
re-derive.

### Common flags

| flag | applies to | meaning |
|---|---|---|
| `--sheets DIR` | most | the book root; default `sheets/` |
| `--json` | `preflight`, `overlap`, `report`, `schema` | machine-readable output |
| `--layer L0\|L1\|L2\|L3` | `preflight` | run one layer only |
| `--strict` | `preflight` | warnings also fail |
| `--pair A:B` | `preflight` | restrict L3 to one spec/impl pair |
| `--key COL` | `overlap` | the join column; default `id` |
| `--cols a,b,c` | `view` | column projection |
| `--rows FROM..TO` | `pack` | ad-hoc row window |
| `--out DIR` | `emit`, `view` | where to write |
| `--verify-determinism` | `emit` | re-emit into a clean directory and byte-compare |

---

## 2. The layers

Preflight is layered so a finding points at the right kind of problem. A layer runs
only if the ones before it pass, because a type error discovered while checking
references is noise.

```
L0  structural   bytes, encoding, framing, keys, the manifest
L1  types        every cell against its schema declaration
L2  refs         every foreign key resolves; no cycles; a topological order exists
L3  overlap      spec vs impl, and client vs server, as relations
L5  evidence     every sheet balances against its producer (truncation guard)
L6  context      every declared view fits its token budget
L7  determinism  the emitter is byte-stable, and generated files are not in the tree
```

There are no L4 and L8 codes. L4 is reserved by the MDD for a check this engine
does not implement; the layer numbering is the MDD's, not this engine's, and
renumbering would make a diagnostic code stop matching the document that defines it.

---

## 3. Rule coverage, stated honestly

`sheetty rules` prints this table, and `preflight` prints its own coverage line, so
"preflight passed" is never mistaken for "the whole MDD checklist passed".

```
PREFLIGHT RULE COVERAGE  29 of 30 MDD checks exist in this engine
  implemented 22   partial 6   unexercised 1   absent 1
```

| status | meaning |
|---|---|
| **implemented** | the rule runs and can fail; `tools/test-rules-sweep.ps1` has seen it fire |
| **partial** | it runs, but checks less than the MDD asks for; the `note` says what is missing |
| **unexercised** | implemented both ways, but nothing in the tree has ever triggered it |
| **absent** | not implemented. Reported as absent rather than faked |

The distinction matters because each status implies different work. A `partial` rule
needs more code; an `unexercised` rule needs either a fixture or deletion; an
`absent` rule needs a decision about whether it can exist at all.

### The six partial rules, and exactly what is missing

| check | what actually happens |
|---|---|
| 10 enum domain | checks that a value is a valid identifier; the declared variant set is **not** consulted, so a typo that is still an identifier passes |
| 16 no unimplemented rows | the overlap reports the count but does not gate; `--strict` gates warnings only |
| 17 no orphan rows | reported, never fails - an orphan is a fact about coverage, not a defect |
| 19 every asset exists | done by the synthesizer at extraction time, not by the engine on every run |
| 20 asset hash matches baseline | every present asset carries a real sha256; drift against a **previous** run is not automated |
| 23 no hand edits to generated files | covered indirectly: a generated file cannot exist in the tree at all, so there is nothing to hash inside `OUT_DIR` |

### The two that are not implemented

**Check 21, kernel allowlist, is `unexercised`.** The rule is implemented in both
directions - it fails on a listed module whose path does not exist, and on a module
under `kernel/` that is not listed - but the description of it is stale. `kernel/` is
no longer empty: it holds nine modules and an integration test, and the rule **has**
fired for real. It caught a sheet edit in this session that named a module, `serve`,
that does not exist:

```
E-L0-KERNEL
  kernel:0   rust_module  kernel module 'serve' is listed but not found under kernel/
```

So the printed status is behind the evidence. It is left as `unexercised` in the
table rather than quietly flipped, because the honest fix is either to retitle the
note or to add a fixture that keeps it firing; flipping a status is not a fix.

**Check 26, dead columns, is `absent`.** The emitter is generic: it consumes every
column of an emitted sheet, so a column that nothing reads cannot occur, and the rule
could only ever pass. A rule that can only pass is exactly what this repository calls
"a check that cannot fail", so it is reported as absent rather than implemented as
decoration.

---

## 4. Diagnostic codes

31 codes are proven to fire by `tools/test-rules-sweep.ps1` against a generated
fixture, one per rule, each asserting the code appears. The list is in that script,
which is the authority; this is the shape of the naming.

```
E-L0-BOM            a byte-order mark            W-L0-SHORT       a row omits trailing cells (dec004)
E-L0-EOL            CRLF where LF is required    W-L0-CROSSKEY    two sheets share a key space
E-L0-ENC            not valid UTF-8                              (exempted by `key_alias:`)
E-L0-DELIM          a space or comma delimiter  E-L1-TYPE        a cell that is not the declared type
E-L0-HEADER         the header row is malformed E-L1-VALUE       a cell that is the type but not legal
E-L0-NOHEADER       no header row               E-L1-EMPTY       empty in a required column
E-L0-MANIFEST       the manifest does not parse E-L1-UNIT        one name, two units (cross-sheet)
E-L0-EMITTER        manifest vs doctrine version E-L1-DEFAULT    a declared default that does not parse
E-L0-KEY            a missing primary key      W-L1-QUOTED      an RFC4180-quoted cell (dec008)
E-L0-CHARSET        an id with a forbidden char
E-L0-DUPKEY         a duplicate primary key    E-L2-REF         a foreign key that does not resolve
E-L0-WIDE           more cells than columns    E-L2-NOEVIDENCE  a Mode B row with no evidence
E-L0-KERNEL         kernel allowlist mismatch  E-L2-CYCLE       a reference cycle
E-L0-GENERATED      a generated file in the tree E-L2-TOPO      no topological order exists
                                               E-SCHEMA-DRIFT   header vs 01-schema.tsv
E-L5-DIVERGE        source_rows != rows + dropped E-SCHEMA-EXTRA / E-SCHEMA-MISSING
E-L6-BUDGET         a context pack over budget    the two directions of drift
W-L6-VIEW           a view that selects nothing
E-L7-ROWCOUNT       a sheet's row count moved badly vs the last commit
```

### Two rules that exist because the obvious version does not work

**`E-L5-DIVERGE` is a sum, not a count.** The truncation guard cannot be a row
count, because a silent loss changes the count too. Each evidence sheet records how
many records its producer emitted and how many were deliberately dropped, and the two
must add up to the row count. A synthesizer that loses rows reports a smaller count
with a straight face; it cannot also make the arithmetic balance against an
independently recorded total.

**`E-L1-UNIT` is cross-sheet.** `speed` in m/s in one sheet and km/h in another is
not an error in either sheet. The rule reads the same column name across the whole
book, which is the only place the conflict is visible.

### A rule that is deliberately NOT a rule

`re/client/functions.tsv` carries `status: symbol_only` because Ghidra decoded zero
instructions for this target (dec013). The engine does not have a rule asserting the
absence of code metrics, because the sheet states it instead: the `status` column and
the `notes` on each row are the record, and a rule would be re-asserting something the
producer already guarantees.

---

## 5. The overlap engine

`overlap` compares two sheets over a shared key space and reports four buckets.

```
$ sheetty overlap re/client/types re/server/types
  covered        2456
  unimplemented  6     terraria.audio.mp3audiotrack, terraria.ogg..., ...
  orphan         5     natupnplib.istaticportmapping, ...
  divergent      2     ~ terraria.netplay.fields: spec='28' impl='30'
```

| bucket | meaning |
|---|---|
| covered | the key is in both, and every shared column agrees |
| unimplemented | in the left sheet, absent from the right |
| orphan | in the right sheet, absent from the left |
| divergent | in both, but a shared column disagrees |

**Provenance columns are excluded from the comparison** (dec016). `artifact` differs
by platform (`ilspy/...` vs `ilspy_server/...`), and comparing provenance as data can
never show agreement between two producers. Before the reserved-column set was made
explicit, overlapping the two type sheets reported `covered 0 / divergent 4640` for
1,546 identical types.

The `divergent` bucket is where the value is. The split between client and server was
found by this query, and so were the two conditionally compiled types that no
name-based comparison could see: `terraria.netplay` has 28 members in the client and
30 in the server.

---

## 6. The emitter

`emit` projects every sheet whose manifest declares `# emit: rust` into Rust. It is
not a template engine: a sheet row's *columns* are the struct fields, so the
projection is a transcription of the schema, and the schema is checked by preflight
before the emitter runs.

A Rust item is a **join of three relations**, which is why no single sheet could
project one:

| sheet | becomes |
|---|---|
| `re/server/types` | the item: `struct`, `enum`, `trait` or delegate marker, in a module tree mirroring the namespace |
| `re/server/fields` | the struct body, in declaration order, plus `const`s and enum discriminants |
| `re/server/methods` | the `impl` body, one `fn` per row |

### The three properties the emitter guarantees

**Determinism.** `emit --verify-determinism` re-emits into a clean directory and
byte-compares. Output is sorted by key, so a sheet that is re-ordered produces
identical files. This is check 27, and it is why `include!`-ing the output is safe:
the same sheets produce the same bytes on every machine.

**Hash-gated writes.** Each emitted file is written only when its content hash
changes, so a second run reports `0 written, N unchanged` and does not touch
timestamps. That keeps incremental builds incremental.

**Preflight gating.** `emit` runs the pipeline first and refuses to write anything if
preflight fails. `sheetty emit --sheets tests/preflight-rules` fails with 8 errors and
writes nothing, which is the check that proves the gate is real rather than a
comment.

### The banner is not decoration

Every emitted file begins:

```rust
// GENERATED BY sheetty v0.1.0 FROM 02-plan - DO NOT EDIT
```

`E-L0-GENERATED` scans for that banner outside `target/` and `.git` and fails if it
finds one. Generated code is never committed (D6): it is regenerated on every build
from `$OUT_DIR`, so the sheets stay the only place the port's shape is written down.

### Unresolved references become visible

504 names are referenced by the server but not declared in it - the BCL, XNA,
`IntPtr`. Each becomes a generated marker in `port::externs`, so the port always
compiles and the size of the not-yet-ported surface is a number rather than a
silence.

---

## 7. Context packs and views

An evidence sheet is too large to be a context unit. Measured:

| sheet | rows | est. tokens, unwindowed |
|---|---|---|
| `re/client/methods` | 14,517 | ~411,287 |
| `re/client/strings` | 21,081 | ~269,180 |
| `re/client/functions` | 18,300 | ~267,598 |
| `re/client/types` | 2,464 | ~69,488 |

A whole evidence sheet is more than twice a 200k context window on its own. The
MDD's prescribed fix is a **row window**, not a smaller sheet, "because splitting the
sheet would break the key space". `sheets/views.tsv` declares a column projection and
a row window per view:

```
v_client_types   re/client/types   id,kind,namespace,name,fields,base   1..760   25000
```

`pack` assembles the view, measures it, prints a **stable prefix hash**, and checks it
against the declared budget. The prefix hash is recorded so a cache miss is visible: if
it changes, the cache is cold (MDD 11.2). The budget is enforced on every preflight as
`E-L6-BUDGET`, including on ad-hoc `--rows` overrides, so the check constrains real
usage rather than nodding at a pre-blessed number.

---

## 8. What the engine is not

- **It is not a build system.** It emits; `build.rs` decides when. The engine has no
  concept of incremental compilation beyond the content hash on each file.
- **It does not know what the sheets mean.** It knows their types, their keys, their
  references and their schema. A sheet that is *wrong* in the sense of describing the
  wrong behaviour passes preflight; that is what `tools/compat.py` is for.
- **It does not verify parity.** It verifies that the book is internally consistent
  and that the projection is faithful. Whether the projected server behaves like the
  original is a different question with a different tool.
- **It cannot check a rule it cannot express.** Check 19 and 20 live in the
  synthesizer because asset existence is a fact about the filesystem at extraction
  time, not about the sheet's text.

The last two are the honest boundary. Every engine has one, and the useful thing is to
name it rather than to describe the engine as if it had none.
