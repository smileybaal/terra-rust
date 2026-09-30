# Roadmap: a Rust plugin host for terra-rust

Every claim below is cited to a sheet row, a decompiled line, or a document. Where
something is a design decision rather than an observation, it says so and gives the
alternative it rejected. Where a number would be a guess, there is no number.

This document is the answer to one question: **what would it take to make this server
extensible, in Rust, without breaking the doctrine the rest of the repository is
built on?**

---

## 0. First, the honest part: what "plugin rewrite" can mean here

The repository's doctrine (D1, D4) is that the sheets are the truth and that
everything else is either a row or a listed kernel module. A plugin system is the
first feature that strains that directly, because a plugin is **code the repository
does not contain and cannot project**. Three readings of "plugin rewrite" are
possible, and they are very different projects:

| reading | what it means | is it compatible with the doctrine? |
|---|---|---|
| **(a) Port the native API surface** | give the port the console commands, the chat commands and the in-game hooks the C# server actually has | **Yes, cheaply.** Almost all of it is rows: the 25 console commands and their strings are already in `sheets/re/server/localization.tsv` |
| **(b) A host for external Rust plugins** | load third-party code (dylib or wasm) that can hook the server | **Yes, if the hook set is a sheet.** The ABI is code; the hook *points* are data |
| **(c) Reproduce TShock** | a full permissions/region/ban/account platform | Not a port of a binary, a port of a *different project*. Out of scope unless asked |

**Recommendation: do (a) first, and (b) only on top of (a).** The reason is not
sequencing politeness, it is that (b) without (a) has nothing to hook. A plugin host
whose only hook is "the world was loaded" is not extensible; it is a demo. Reading
(a) is also what produces (b)'s design, because the native server's own extension
points *are* the natural hook set.

---

## 1. What the native server actually exposes (measured, not assumed)

There is **no plugin framework in `TerrariaServer.exe`.** That is a finding, and it
matters, because it means anything TShock does, it does by patching memory or by
riding the few seams below. Searched across `sheets/re/server/`:

- No `IPlugin`, `IHook`, `LoadPlugin` or `PluginLoader` type or method.
- No directory scan for plugin files, and no plugin config key.
- The single `Assembly`-named field is `Main.SkipAssemblyLoad`
  (`terraria.main.skipassemblyload`), and it is **not** a plugin loader: it is the
  quick-launch flag that skips JIT-preloading the game's own assembly
  (`Program.StartForceLoad`, `Program.cs:51-61`, and stored as the config key
  `QuickLaunch` at `Main.cs:4360`). It is named here because a search for
  "AssemblyLoad" finds it, and a reader deserves to know what it is rather than to
  find it unexplained.

What *does* exist, and is therefore the real surface:

### 1.1 The console command loop

`Main.DedServ()` (`Main.cs:5363`) reads lines with `Console.ReadLine()`
(`Main.cs:6333`, via `ReadLineInput`) and dispatches them through a hard-coded
`else if` chain over localization keys:

```csharp
if (Language.GetText("CLI.MOTD_Command").EqualsCommand(text)) { ... }
else if (Language.GetText("CLI.Say_Command").ParseCommandPrefix(text, out remainder)) { ... }
else if (Language.GetText("CLI.Kick_Command").ParseCommandPrefix(text, out remainder)) { ... }
```

A command's **name and prefix are localization values**, not code. The dispatcher is
26 `CLI.*_Command` keys, all present as rows in
`sheets/re/server/localization.tsv` (the `CLI` section, 108 keys total).

This is the single highest-value seam to port, for three reasons: it is already rows,
it is what an operator actually touches, and it is where a plugin would register a
new admin command.

### 1.2 The chat command processor

`ChatCommandProcessor` (`ChatCommandProcessor.cs`) is a real, typed registry:

```csharp
public ChatCommandProcessor AddCommand<T>() where T : IChatCommand, new()
public ChatCommandProcessor AddDefaultCommand<T>() where T : IChatCommand, new()
private static ParseCommandPrefix<T>(...)
```

`AddCommand<T>` reads a `ChatCommandAttribute` off `T`, builds the key
`"ChatCommand." + attr.Name`, and registers the instance in `_commands`. So the game
has a first-class `IChatCommand` interface with attribute-driven registration - the
closest thing to a plugin API that ships in the binary.

**This is the model to copy**, because it is the game's own answer to "how does a
command get registered", and copying it means a plugin author's mental model matches
the game's.

### 1.3 Events and delegates

Only two events exist on the networking type:

| field | row |
|---|---|
| `Netplay.OnDisconnect` | `terraria.netplay.ondisconnect` (`Action`) |
| `Netplay.HasFullyConnectedClients` | a `bool` property, not an event |

`PlacementHook` (`terraria.datastructures.placementhook`) is a structured multi-return
hook for tile placement, not an extension point.

So: **the native server almost never calls out to anything.** There is no
`ServerChat` hook in vanilla - that is TShock's own invention, built by a runtime
patch. A port that wants hooks has to define them itself, and the honest way to do
that is to define them where the C# already has a decision point, so each hook can
cite the line it corresponds to.

### 1.4 What TShock adds, as a design reference only

TShock's developer docs describe two hook families:

| family | example | how it registers |
|---|---|---|
| **TSAPI** (`TerrariaApi.Server`) | `ServerApi.Hooks.ServerChat.Register(this, OnServerChat)` | `Register(this, callback)` |
| **TShock packet hooks** | `TShock.GetDataHandlers.TogglePvp += OnTogglePvp` | `+=` on a `GetDataHandlers` event |

TShock's `GetDataHandlers` exposes one .NET event per packet id, with a typed
`EventArgs` carrying the packet's already-parsed fields (`TileEditEventArgs` has
`X`, `Y`, `Type`, `EditType`; `PlayerInfoEventArgs` has `playerid`, `name`,
`difficulty`). That design is worth copying conceptually: **one hook per message id,
with typed arguments, not one generic "packet arrived" callback.** The typed version
is what makes a plugin readable, and this repository already has the per-message
field knowledge to generate those argument types.

Also relevant, from TShock's own docs: plugins are **trusted code**. `Plugin Safety`
is a page telling users not to install unknown plugins. There is no sandbox in the
TShock ecosystem. That is a data point for §3.

---

## 2. What the port has today (measured)

| thing | state |
|---|---|
| console command surface | **none.** No `help`, no `exit`, no line reader. `kernel/boot.rs` starts the listener and never reads stdin |
| chat command surface | none |
| hooks | none |
| message dispatch | a literal `if id == ...` chain in `kernel/net.rs::client_loop`: **7** such branches, against a table that has 163 ids |
| ids the kernel sheet claims | 17 (`tools/barrier.py --check`) |
| `net` in `sheets/02-plan.tsv` | `priority 9`, `status todo`, "rank unmeasured, but the entry path is written" |

The dispatch chain is the thing a hook system must not become: it is already growing
one `if` per message, and at 163 message ids it will be unreadable. **The plugin
roadmap's first real work is not plugins - it is replacing that chain with a
dispatch table**, because a table is both the honest port of `MessageBuffer`'s own
`switch (msgType)` and the thing a hook can attach to.

---

## 3. The design decision: how external code loads

Three options, and the tradeoff is real. This is the section where a wrong choice is
expensive, so the rejected alternatives are recorded.

| option | isolation | hot reload | Rust ABI risk | author ergonomics |
|---|---|---|---|---|
| **native dylib** (`libloading`) | none - a segfault takes the server down | hard (must unload, and Rust cannot safely) | every plugin must match the host's exact compiler and layout | best: plain Rust |
| **stable ABI** (`abi_stable`) | none | hard, same reason | checked at load; refuses a mismatched plugin instead of corrupting | good: Rust with `#[repr(C)]` and versioned traits |
| **wasm** (`wasmtime` / `extism`) | full sandbox, fuel-limited, memory-capped | easy: drop the module, re-instantiate | none: the ABI *is* the wasm contract, checked at link | worst: a build toolchain and a WIT/capability layer |

Published comparison puts wasm at roughly 80-95% of native for compute-bound work,
with cross-boundary calls being the real cost; a native dylib has no such cost but
also no isolation. **No benchmark was run for this document** - the numbers above are
from a published survey, and the roadmap's Stage 0 exists precisely so the choice can
be made on a measurement of *this* server rather than on someone else's page.

### Recommendation

**Start with the in-process hook API and no loader at all (Stage 1-3). Then choose a
loader (Stage 4) once the hooks exist and the cost is measurable.**

Rationale, in the order that matters:

1. **The hook surface is the hard part, not the loader.** Defining 20 hooks with
   typed arguments that cite the C# is weeks of work; `Plugin::new(&wasm)...` is a
   day. Doing the loader first produces a beautiful loader with nothing to load.
2. **A plugin that needs to touch tiles and players cannot be sandboxed usefully.**
   The interesting hooks (`TileEdit`, `NpcStrike`, `PlayerSpawn`) all *mutate world
   state*. Over wasm that becomes a copy-in/copy-out of 20 million tiles or a
   capability surface as large as the world - which is the isolation argument
   collapsing on contact. Sandbox first makes sense for a plugin that *transforms
   data*; this server's plugins *are* the data's owner.
3. **TShock's entire ecosystem is trusted, in-process C#.** The most successful
   Terraria server platform chose trusted dylib-equivalent loading and has thousands
   of plugins. That is strong evidence about what this ecosystem's authors want.
4. **If untrusted plugins are ever wanted, the escape hatch is a separate process**,
   not wasm: an RPC plugin with its own address space gets real isolation without
   making every tile access a marshalling problem. That door stays open.

So the phased answer is: **in-process hooks now, `abi_stable` for the loader
(mismatch refused at load beats silent corruption), wasm or out-of-process only if
untrusted plugins become a requirement.**

---

## 4. The doctrine question: how does a plugin system stay sheet-driven?

This is the part that decides whether the roadmap is honest. A hook API is code, and
D4 says the repository holds rows and listed kernel modules. Resolutions, in order:

### 4.1 The hook SET is a sheet

Hooks are data: a name, the C# line that defines them, the arguments, and whether
they can veto. That is a relation, and it goes in a sheet - proposed
`sheets/hooks.tsv` - so that:

- the Rust trait, the plugin-facing docs and the dispatch table are all projections
  of one relation, and cannot drift;
- an unloading change to a hook name is a preflight problem, not a silent break;
- `03-impl`-style status tracking applies: a hook marked `todo` is one with no call
  site yet, and a `--strict` run can refuse to ship a documented hook that is a stub.

A hook row is roughly:

```
id:string*       name         args                    veto  cs_ref                      status
hook.tile_edit   TileEdit     player,x,y,type,edit    yes   MessageBuffer.cs:1550        todo
hook.server_chat ServerChat   player,text             yes   MessageBuffer.cs:1240        todo
```

Every `cs_ref` is a line in the decompiled server, so "does this hook correspond to a
real decision point" is checkable rather than asserted.

### 4.2 The message dispatch table is generated from the id table

`MessageBuffer`'s 163 ids are already rows. A generated `match` over them - with the
handler name taken from the same sheet - replaces `client_loop`'s `if` chain and
gives every message a single named entry point that a hook can wrap. This is D2
(one row, one strut) applied to the protocol.

### 4.3 A plugin's own config surface is a sheet too

A plugin that adds `/home` and `/sethome` is adding **commands**, and commands are
rows in the native design (§1.1: names are localization values). So a plugin declares
its commands in its own manifest, and the host registers them into the same table the
native commands use. The plugin's code is a dylib; its *interface to the server* is
data. That is the split that keeps the doctrine intact.

### 4.4 What is honestly NOT sheet-driven

The plugin's Rust bodies. There is no way to project arbitrary plugin logic from
rows, and pretending otherwise would be the exact "hand-typed copy of a derivable
value" the doctrine forbids, inverted. State it plainly in the docs: **the sheets own
the hook surface and the command surface; a plugin owns its own behaviour.** The
host's job is to make the boundary checkable.

---

## 5. Stages

Each stage has a stated proof. A stage without a proof is not done.

### Stage 0 - measure before choosing a loader

**Goal:** decide the isolation question with a number from this server.

- Add `benches/` with two microbenchmarks: (a) a no-op hook called once per tile
  edit, (b) a no-op hook called once per message, at 20 million tiles and at 163 ids.
- Implement the same trivial hook three ways - direct call, `abi_stable` call, wasm
  call - and record the per-call cost.
- Publish the table in this document, with the machine named.

**Proof:** a table of measured ns/call for the three, replacing the "80-95% of
native" figure borrowed from a survey. **Until this exists, §3's recommendation is
reasoned, not measured, and this document says so.**

### Stage 1 - the console command surface (native parity, no plugins yet)

**Goal:** the 26 `CLI.*_Command` commands, dispatched from rows.

- A line reader thread (`stdin`), which the port does not have at all today.
- `ParseCommandPrefix` / `EqualsCommand` ported from `LocalizedText`
  (`kernel/` module), because the C# matches commands with them, and the semantics
  (prefix + remainder, case handling) are the actual behaviour.
- A generated dispatch table from `sheets/re/server/localization.tsv`'s `CLI` rows.
- Commands with real behaviour first: `help`, `exit`, `version`, `say`, `kick`,
  `players`/`playing`, `time`/`dawn`/`dusk`/`noon`/`midnight`, `save`.
  `newworld`/`deleteworld` need world-gen, so they are named as unimplemented.

**Proof:** `tools/capture.py` cannot see this (it is not on the wire), so the proof is
a new `tools/console.py` that drives the server's stdin with a script and diffs the
output against the native server's, exactly as `compat.py` does for the wire. **The
harness exists; the console half of it does not, and that is the first thing Stage 1
builds.**

### Stage 2 - the dispatch table (a refactor with a proof)

**Goal:** `client_loop`'s `if` chain becomes a generated `match` over the id table,
with one named handler per id.

- Generated from `sheets/re/server/fields.tsv`'s `terraria.id.messageid.*` rows.
- Every currently-handled id keeps its behaviour; `barrier.py` must still report no
  barrier in the client's opening.
- A message with no handler gets the existing honest kick, still naming the id.

**Proof:** `tools/capture.py` traces before and after are byte-ALIGNED
(`compat.py`), which proves the refactor changed no behaviour. This is the first
place the harness pays for itself outside parity work.

### Stage 3 - the hook API, in-process, no loader

**Goal:** `sheets/hooks.tsv` exists, and 6-8 hooks are real, each with a call site
citing a `MessageBuffer`/`NetMessage` line.

Proposed first set, each a real decision point:

| hook | call site | veto? |
|---|---|---|
| `OnConnection` | `Netplay.OnConnectionAccepted` | yes (refuse) |
| `OnHello` | `MessageBuffer.cs:181-222` | yes (kick) |
| `OnPlayerInfo` / `OnSyncPlayer` | `MessageBuffer.cs:248` | yes |
| `OnPlayerSpawn` | `MessageBuffer.cs:901-950` | no |
| `OnTileEdit` | the tile-edit message case | yes |
| `OnServerChat` | the chat message case | yes |
| `OnNpcStrike` | the damage path | yes |
| `OnCommand` | the console + chat command tables | yes |

- A trait per hook or one trait with methods; a `HookSet` registered at startup.
- **Veto semantics are explicit**: each hook row says whether returning `false`
  cancels the action, because "can a plugin stop this" is the single most
  consequential property of a hook and must not be discovered by reading code.
- A plugin is, at this stage, a Rust module in-tree: it proves the hook set without
  proving a loader.

**Proof:** a test plugin that vetoes `OnTileEdit` at one coordinate, and a trace
showing the client's tile edit was refused - over a real socket, the way
`kernel/tests/hostile.rs` already does it.

### Stage 4 - the loader

**Goal:** external plugins load, and a mismatched one is refused with a reason.

- `abi_stable` root module, one per plugin, loaded from a `plugins/` directory.
- A plugin manifest (name, version, host-ABI version, commands, hooks it uses).
  The manifest is the plugin's row contribution: commands and hook subscriptions are
  data, so the same preflight can read them.
- **A load-time ABI check that fails loudly**, plus a `--no-plugins` flag so a broken
  plugin cannot make the server unstartable.
- Panic containment: a plugin panic must be caught at the hook boundary and reported
  with the plugin's name, not take the server down. (This is the one isolation
  property worth having even without a sandbox.)

**Proof:** two plugins, one good and one built against a different ABI version; the
server loads one, refuses the other by name and reason, and serves a client
(`capture.py` ALIGNED) in both cases.

### Stage 5 - what TShock has that is worth having

Not a TShock port. Three things from it, in value order:

1. **Permissions/groups** - a user, group and permission model, because every real
   plugin needs one and re-inventing it per plugin is how plugin ecosystems rot.
2. **Regions** - protected areas, which is what most Terraria plugins actually want
   to do.
3. **REST API** - TShock's `/v2/...` surface. Note it is a *separate* concern from
   plugins and should be its own roadmap if wanted.

Each of these is a sheet (users, groups, permissions, regions) plus behaviour, so
each fits the doctrine. None of them is needed for Stage 4.

---

## 6. Risks, stated rather than discovered later

| risk | why it is real | mitigation |
|---|---|---|
| **The hook set is guessed wrong** | 20 hooks designed in advance will not match what plugins need | Stage 3 ships 6-8, and `sheets/hooks.tsv` makes adding one cheap and visible |
| **Rust has no stable ABI** | a plugin built with a different rustc is undefined behaviour, not an error | `abi_stable`'s load-time check, and a documented pinned toolchain (`00-doctrine.tsv` already pins rust 1.98.1) |
| **A plugin deadlocks the server** | hooks are called from `client_loop`, which holds state | hooks get `&mut World`-style access with a documented rule that they must not block; Stage 0 measures the call cost so a slow hook is visible |
| **Hot reload becomes a requirement** | plugin authors will ask; Rust cannot unload safely in general | do not promise it. The honest answer is "restart the server"; a plugin changing its config does not need a reload |
| **The doctrine erodes** | "just put it in the plugin" is how a sheet-driven project becomes a normal one | §4's split: the hook and command surfaces are sheets, plugin bodies are not, and the boundary is written down |
| **The dispatch refactor (Stage 2) breaks the join** | it touches the exact code path a client depends on | the refactor's proof is a byte-ALIGNED trace diff, which is why Stage 2 comes before any plugin work |

---

## 7. What this roadmap does NOT claim

- **No benchmark was run.** §3's speed figures are from a published survey.
  Stage 0 exists to replace them.
- **Nothing here is implemented.** Stages 1-5 are unstarted. The port has no console
  surface, no dispatch table and no hooks today (§2), and this document says so
  rather than describing the design as if it were the state.
- **No TShock compatibility.** A TShock plugin is a .NET assembly; it cannot load
  into a Rust server. Only the *shape* of TShock's API is a reference.
- **The hook list is a proposal, not a measurement.** §5's table is what I would
  expect to be needed, derived from where the C# has decision points; it has not been
  validated against a plugin author's needs.

---

## 8. Suggested order

1. **Stage 0** (measure) - small, and it turns the isolation decision from an
   opinion into a number.
2. **Stage 1** (console) - the native surface, already rows, and the first thing an
   operator touches. It also builds the console half of the harness.
3. **Stage 2** (dispatch table) - a refactor whose proof already exists.
4. **Stage 3** (in-process hooks) - the real design work.
5. **Stage 4** (loader) - small once Stage 3 has something to load.
6. **Stage 5** (permissions, regions) - only if wanted.

The order is chosen so that every stage is provable with tooling that exists, except
Stage 1's console driver and Stage 0's benchmarks, which are named as the two tools
these stages must build.
