# terraria-port - reproducing the pipeline

Everything below is runnable from a clean checkout. Raw producer output is not
committed (`re/exports/**` is gitignored); the *evidence sheets* are, because
"The Ghidra project is a cache, the evidence sheet is the artifact" (MDD §4).

## 0. Prerequisites (pinned in sheets/00-doctrine.tsv)

| Tool | Version used | Notes |
|---|---|---|
| Ghidra | 12.1.4 PUBLIC | `C:\Users\PORTMANTEAU\Desktop\Misc\ghidra_12.1.4_PUBLIC` |
| ghidra-cli | 0.2.2 | `git clone https://github.com/akiselev/ghidra-cli && cargo install --path .` |
| JDK | 27 | `ghidra doctor` passes and the bridge compiles under it (dec007) |
| Rust | 1.98.1 | stable-x86_64-pc-windows-msvc |
| .NET SDK | 9.0.205 | needed to run `ilspycmd` |
| ilspycmd | 9.1.0.7988 | `dotnet tool install -g ilspycmd` |
| git | 2.49.0 | with `core.autocrlf=input` so LF is preserved (preflight check 2) |

## 1. One-time setup

```
ghidra config set ghidra_install_dir C:/Users/PORTMANTEAU/Desktop/Misc/ghidra_12.1.4_PUBLIC
ghidra config set ghidra_project_dir  <repo>/re/ghidra_projects
ghidra config set default_limit       100000
ghidra doctor            # must pass before anything else
```

`default_limit` matters: the built-in default is 1000 and it silently truncates
large listings. With 18300 functions, the default would cut the export to 5%.

## 2. Ingest

```
ghidra import re/binaries/Terraria.exe --project terraria --program Terraria
```

Analysis of this binary takes roughly 15-20 minutes. Watch it without blocking:

```
ghidra jobs --project terraria        # {"active_job":{...,"state":"running"}}
```

`ghidra-cli` serialises program operations behind the running analysis, so
`function list` will queue rather than fail. `ping`, `status`, `jobs` and
`cancel` stay live throughout.

## 3. Extract

```
ghidra function list --project terraria --json --fields name,address,size > re/exports/ghidra/functions.json
ghidra symbol list   --project terraria --json > re/exports/ghidra/symbols.json
ghidra type list     --project terraria --json > re/exports/ghidra/types.json
ghidra find interesting --project terraria --json > re/exports/ghidra/interesting.json
ghidra find string ""   --project terraria --json > re/exports/ghidra/strings_all.json
ghidra find crypto      --project terraria --json > re/exports/ghidra/crypto.json
ghidra comment list     --project terraria --json > re/exports/ghidra/comments.json
```

`re/assets.tsv` additionally reads the installed content directory. Set
`CONTENT_DIR` if your install is not at the Steam default:

```
set CONTENT_DIR=D:\Games\Terraria\Content
```

ILSpy side:

```
set DOTNET_ROLL_FORWARD=LatestMajor
ilspycmd --disable-updatecheck -p --nested-directories -o re/exports/ilspy re/binaries/Terraria.exe
for %K in (c s e i d) do ilspycmd --disable-updatecheck -l %K re/binaries/Terraria.exe > re/exports/ilspy_entities_%K.txt
```

Two calibrations learned the hard way:

- `ilspycmd -o` writes only the JSON/TSV/dll payloads unless `-p` is given.
  Use `-p` to get the actual `.cs` tree.
- `ilspycmd -il` ignores `-t` and emits the **whole assembly** as IL. For this
  binary that is **125 MB** in one file. Use `-p` per-type files instead, and
  reach for `-il` only when you genuinely need IL for one type.

## 4. Normalize (raw exports -> canonical sheets)

```
python re/ghidra_scripts/synth_ghidra.py     # -> re/functions, re/triage, re/types_pe, re/strings, re/assets
python re/ghidra_scripts/synth_ilspy.py      # -> re/types, re/methods
```

Both scripts write canonical form (UTF-8 no BOM, LF, TAB), dedup by id, and
put `ref_addr` + `ref_conf` + `evidence` on every row.

## 5. Validate (preflight gates everything, D5)

```
cargo run -p sheetty-cli -- preflight
cargo run -p sheetty-cli -- preflight --json
cargo run -p sheetty-cli -- preflight --layer L0
cargo run -p sheetty-cli -- overlap 02-plan 03-impl
cargo run -p sheetty-cli -- report unimplemented --rank blast-radius
cargo run -p sheetty-cli -- view re/triage --cols id,score,priority,ref_addr
cargo run -p sheetty-cli -- schema            # list declarations
cargo run -p sheetty-cli -- order             # dependency order (Kahn)
cargo run -p sheetty-cli -- rules             # which MDD checks exist, and which do not
cargo run -p sheetty-cli -- pack v_types      # context pack + budget check (L6)
cargo run -p sheetty-cli -- pack v_methods --rows 1..900 --out re/ctx/pack.txt
cargo run -p sheetty-cli -- schema --emit sheets/01-schema.tsv   # bootstrap only
```

Exit codes: `0` clean, `1` errors present, `2` usage error.

`sheetty rules` is not decoration. The engine implements L0-L3 only; units,
defaults and topological order are covered, but enum domain is form-only, and
L4-L7 plus the whole `[emit]` stage do not exist. Printing that in every
preflight report is the difference between "the checks pass" and "the checklist
passes".

### Proving the rules fire

```
powershell -File tools/test-preflight.ps1
```

This runs the engine against `tests/preflight-rules/`, a fixture that
deliberately breaks seven rules, and asserts each diagnostic appears; then it
asserts the real book is still clean. A rule that has never been seen to fire is
not a verified rule (MDD: "a rule that never fires may be checking nothing").

**`schema --emit` is a bootstrap tool, not a routine command.** It generates
`01-schema.tsv` from the current headers. Once committed, the schema is the
truth and any header divergence is `E-SCHEMA-DRIFT`. The `view` and `notes`
columns are preserved across regeneration, because they are schema-only curation
that does not exist in the sheet headers - without that, re-bootstrapping would
silently destroy every view assignment.
