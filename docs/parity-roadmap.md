# Roadmap: 1:1 parity with the native dedicated server

Every number and every claim below is cited to a sheet row or a decompiled line. Where
the evidence is a sheet, it is named; where it is the C#, it is `file:line`. Where
something is inferred rather than measured, it says so.

## 0. What "1:1 parity" has to mean, measured

The target is not a small program. From `sheets/re/server/{types,fields,methods}.tsv`:

| | count |
|---|---|
| types | 2,463 |
| fields | 30,040 |
| methods | 14,486 |

Half of those fields are constants already projected: `terraria.id` alone carries
**15,033 fields across 140 types** (`terraria.id.itemid` is 6,247 fields, `soundid`
585). Those are rows, and the port already emits them. The rest is behaviour.

The server's external surface, also enumerated:

| surface | count | source |
|---|---|---|
| message ids | 163 | `terraria.id.messageid.*` rows |
| CLI strings | 108 | `Terraria.Localization.Content.en-US.json` |
| launch flags | 21 | `Initializers/LaunchInitializer.cs:55-207` |
| config keys | 18 | `Main.cs:5088+` (`LoadDedConfig`) |
| console commands | 22 | the `CLI.*_Command` strings |
| world-file methods | 56 | `terraria.io.worldfile.*` rows |

**Definition of done for parity:** for the same world file and the same scripted
client, a byte-diff of the wire trace and of the console output shows no difference
between the native server and this port. That is checkable, which is why Stage 0
exists.

Top of the cost table, from `sheets/re/server/triage.tsv` (scores are ILSpy-measured
field/method counts, a size proxy per `dec013`; every one of these is priority 1):

| type | fields | methods | score |
|---|---|---|---|
| `terraria.player` | 1313 | 840 | 49.87 |
| `terraria.main` | 1244 | 768 | 49.32 |
| `terraria.worldgen` | 164 | 782 | 43.57 |
| `terraria.npc` | 343 | 402 | 42.82 |
| `terraria.projectile` | 126 | 355 | 39.40 |
| `terraria.item` | 160 | 112 | 35.12 |
| `terraria.io.worldfile` | 28 | 56 | 27.21 |
| `terraria.netplay` | 30 | 41 | 26.09 |
| `terraria.netmessage` | 7 | 39 | 21.97 |
| `terraria.messagebuffer` | 21 | 8 | 18.43 |

By subsystem, `terraria.gamecontent` is the mass: **1,251 types, 5,695 methods**.
Stages 1-4 are reachable and bounded. Stage 5 is the rest of the program.

## Stage 0 - the parity harness (first, because nothing else is verifiable without it)

`tools/mitm.py` already relays and names every packet from the book. What it needs to
become a harness:

- **Trace recording**: write each frame as `(direction, id, name, body, timestamp)` to a
  file, so a run can be replayed and diffed instead of eyeballed.
- **A scripted client**: the port's `%TEMP%` probe scripts, promoted into `tools/`, so the
  same client drives both servers.
- **A differential runner**: start the native server and the port on two ports, drive both
  with the same script, diff the traces. Report the first divergent frame with its id name
  from the book.

This is what turns "1:1" from an assertion into a test. Everything below is graded by it.

## Stage 1 - the launch surface

### 1.1 Launch flags (21)

`LaunchInitializer.cs` has three loaders. What the port has today is `-p`/`-port`,
`-pass`/`-password` (both in `kernel/boot.rs`, with `TryParameter`'s first-key-wins order)
and `-savedirectory`.

| flag | effect | cite |
|---|---|---|
| `-p`, `-port` | `Netplay.ListenPort` | :30 |
| `-maxplayers`, `-players` | `SetNetPlayers`, range 1..255 | :104-111 |
| `-pass`, `-password` | `Netplay.ServerPassword` | :113-116 |
| `-lang`, `-language` | language | :118-125 |
| `-worldname` | `SetWorldName` | :127-130 |
| `-motd` | `NewMOTD` | :132-135 |
| `-banlist` | `Netplay.BanFilePath` | :137-140 |
| `-autoshutdown` | hide the console window | :141-144 |
| `-hosttoken` | `Netplay.HostToken` | :146-149 |
| `-secure` | `Netplay.SpamCheck` | :150-153 |
| `-worldrollbackstokeep` | backup count | :155-158 |
| `-autocreate` | `autoCreate` | :160-163 |
| `-noupnp` | `Netplay.UseUPNP = false` | :164-167 |
| `-experimental` | experimental features | :168-171 |
| `-world` | `SetWorld`, or auto-gen target | :173-184 |
| `-cloudworld` | Steam-cloud world | :185-196 |
| `-config` | `LoadDedConfig` | :198-201 |
| `-seed` | `Main.AutogenSeedName` | :202-206 |
| `-forcepriority` | process priority, 0-5 | :60-98 |
| `-loadlib` | shared | :25-28 |

### 1.2 The config file (18 keys)

`LoadDedConfig` (`Main.cs:5088+`) is a line reader: a line is matched by a
case-insensitive `prefix=`, so it is not a real parser and unknown keys are ignored
silently. Keys, in the order the code tests them: `world`, `port`,
`worldrollbackstokeep`, `maxplayers`, `priority`, `password`, `motd`, `lang`,
`language`, `worldpath`, `worldname`, `seed`, `banlist`, `difficulty`, `autocreate`,
`secure`, `upnp`, `npcstream`.

### 1.3 The interactive world select (what the user asked about)

`Main.DedServ()` at `Main.cs:5363`. The loop at :5386 runs until `worldPathName` is set:

1. `LoadWorlds()` populates `WorldList` from disk.
2. Print `CLI.Server` ("Terraria Server {0}") then each world as `i+1\t\t<name>`,
   prefixing `(StatusName) ` for worlds that fail to load (:5394-5403).
3. Print `n` / `d <number>` with their descriptions, then `CLI.ChooseWorld`.
4. `d <n>`: `CLI.DeleteConfirmation`, `(y/n)`, `EraseWorld(n-1)` (:5425-5454).
5. `n`: choose size, then difficulty, then evil, then name, then seed flags, then
   `WorldGen.CreateNewWorld` (:5455-5683, `DedServ_SeedFlagsMenu` at :5957).
   Sizes are hard-coded: **4200x1200**, **6400x1800**, **8400x2400** (:5473-5483).
6. Picking an existing world prompts `CLI.SetInitialMaxPlayers` (empty = **16**,
   :5733-5740), `CLI.SetInitialPort` (empty = 7777, :5765-5772), and
   `CLI.EnterServerPassword` (:5820).

Note the two different defaults for the player limit, both real: the field
`Main.maxNetPlayers = 255` (`Main.cs:1108`), and the interactive prompt's empty-answer
default of 16. Whichever path runs decides which applies.

### 1.4 The console command loop (22)

`help`, `clear`, `exit`, `exit-nosave`, `save`, `kick <player>`, `ban <player>`,
`password [pass]`, `motd [words]`, `say <words>`, `time`, `port`, `maxplayers`,
`playing`, `version`, `seed`, `settle`, `fps`, `dawn`, `noon`, `dusk`, `midnight`.
Every one is a `CLI.*_Command` string, and the help text is `CLI.AvailableCommands`.

### 1.5 Proof for Stage 1

Run both binaries with a scripted stdin (a world number, then `help`, `port`,
`maxplayers`, `version`, `exit-nosave`) and diff stdout. This is achievable early
because it needs no world support for the command half.

## Stage 2 - world files

`terraria.io.worldfile` is 56 methods, and the format surface is enumerated:

- `LoadWorld`, `LoadWorld_Version2`, `LoadHeader`, `LoadWorldFlags`,
  `LoadWorldTiles`, `LoadWorld_LastMinuteFixes`
- `LoadWorld_Version1_Old_BeforeRelease88` - a legacy reader that must stay
- `SaveWorld`, `SaveWorld_Version2`, `SaveWorldHeader`, `SaveWorldFlags`,
  `SaveWorldTiles`
- `WorldFileData` metadata (`GetAllMetadata`) feeds the world-select list

The header is the interesting half: it is what `WorldData(7)` sends to the client, so
Stage 2 and Stage 3 share it. Read a real world (`Documents/My Games/Terraria/Worlds/*.wld`)
and compare every header field against the native server's, which is a cheap,
high-signal check.

Backups: `worldrollbackstokeep` / `WorldRollingBackupsCountToKeep`, plus autosave.

## Stage 3 - the load path (a vanilla client gets in)

The server state machine, confirmed line by line:

| receives | guard | new state | sends | cite |
|---|---|---|---|---|
| `Hello(1)` | `State==0`, greeting `"Terraria"+326` | 1 (or -1) | `PlayerInfo(3)` / `RequestPassword(37)` | MessageBuffer.cs:203-214 |
| `RequestWorldData(6)` | - | 2 | `WorldData(7)` | MessageBuffer.cs:465-469 |
| `SpawnTileData(8)` | - | 3 | `StatusTextSize(9)`, then `SendSection` xN | MessageBuffer.cs:807-818 |
| `PlayerSpawn(12)` | `State>=3` | 10 | `PlayerSpawn(12)` broadcast, `FinishedConnectingToServer(129)`, `greetPlayer` | MessageBuffer.cs:921-945 |

`WorldData(7)` is ~40 fields (`NetMessage.cs:233-302+`): time, day/blood-moon/eclipse
bits, moon phase, world size, spawn tile, world surface, rock layer, `WorldId`, world
name, `GameMode`, a GUID, generator version, moon type, 14 background style bytes, ice/
jungle/hell back styles, wind, clouds, `treeX[3]`, `treeStyle[4]`, `caveBackX[3]`,
`caveBackStyle[4]`, `TreeTops.SyncSend`, rain, boss-down bits.

Sections stream via `SendSection(whoAmi, sectionX, sectionY)` as `TileSection(10)` and
`TileFrameSection(11)`. The port already knows `Netplay.NetBufferSize` (1024) and the
frame bound, so section sizing has its rows.

**This is where the sheets run out.** Every field above is runtime world state, and the
book marks them `todo` with no value (`Main.maxTilesX` ord 396, `worldName` 469,
`maxNetPlayers` 406, `ActiveWorldFileData` 831). So the values come from Stage 2's world
reader. Stage 3 without Stage 2 means hand-authored world values, which is a documented
departure from D1 rather than a derivation, and should be labelled as such if chosen.

## Stage 4 - the server loop and the full message surface

- `Netplay.ServerLoop` (:9375 in the methods sheet), the sleep pattern at
  `Netplay.cs:333-339`, and the per-client `RemoteClient` state machine.
- Slot allocation: `FindNextOpenClientSlot` loops `i < Main.maxNetPlayers`
  (`Netplay.cs:642-651`), and `OnConnectionAccepted` kicks with `CLI.ServerIsFull` when
  it returns -1 (:221-235).
- Then the remaining ~160 message ids, each with a `MessageBuffer.GetData` case and a
  `NetMessage.SendData` case.

Proof: two vanilla clients see each other, and a scripted movement trace diffs clean.

## Stage 5 - the simulation (the bulk, and the honest part)

`terraria.gamecontent` 1,251 types / 5,695 methods; `terraria.npc` 343/402;
`terraria.projectile` 126/355; `terraria.item` 160/112; `terraria.player` 1313/840;
`terraria.worldgen` 164/782. This is tile logic, liquids, wiring, NPC AI and spawning,
boss fights, day/night, weather, events and invasions, chests, achievements, bestiary.

Nothing here is optional for true parity, and it is the majority of the 14,486 methods.
A credible sequence inside Stage 5 is: tiles and collision -> items and inventory ->
NPC spawn/AI -> projectiles and damage -> world events -> wiring and mechanisms ->
bosses. Each is independently observable through the harness.

## Stage 6 - the periphery

Save and autosave, world rollback backups, ban list (`Netplay.IsBanned`,
`BanFilePath`), MOTD/password/`-secure`, UPnP port mapping (`Netplay.cs:172-210`),
Steam/social (`SocialAPI`, lobby, `Netplay.cs:261+`), logging (`Program.SetupLogging`,
`-logpath`), crash reporting, and localization.

## Cross-cutting: the strings gap is now the biggest doctrine hole

The sheets carry **no strings** (`dec015`; the server has no `strings` sheet), so every
protocol string in `kernel/net.rs` is hand-cited from the C# and the localization table -
the module says so itself. But the export ships the localization JSON, and it is
machine-readable: 3,699 string leaves in en-US, of which `CLI.*` is 108, `Net.*` is 27,
and `LegacyMultiplayer.*` (the `Lang.mp[n]` table the kick path uses) is 27.

**Recommended early task: emit a strings sheet from the localization JSON.** It is a
mechanical generator, it closes the largest remaining "typed by hand" surface, and it
turns `mp::INCORRECT_PASSWORD = "LegacyMultiplayer.1"` from a citation into a projection.
The JSON is JSON.NET output with trailing commas, which a reader must tolerate.

## Defects and divergences found while researching this

1. **Connection cap is wrong.** `kernel/net.rs:233` uses `Netplay::MaxConnections` (256,
   the `Clients[]` array size) as the limit. The C# limits by `Main.maxNetPlayers`
   (default 255, `Main.cs:1108`; interactive default 16), so the port accepts one extra
   player and ignores `-maxplayers`/`maxplayers=`. The kick key is already correct
   (`CLI.ServerIsFull`, `Netplay.cs:233`).
2. **`terraria.id.messageid.count` carries no value** (`-`, `todo`), like
   `Netplay.ServerPassword`. The table is otherwise complete at 163 ids with no
   duplicates. Harmless today (nothing consumes `Count`), but it is a book gap.
3. **`tools/mitm.py` mislabelled message 4** as a connection `State`; it is `SyncPlayer`
   (fixed, `b3ad118` and `319f565`). The book was right; the hand label was wrong.
4. **The boot report is ambiguous**: it prints `listen port 7777` from the sheet's
   `DefaultPort` while the process may be listening elsewhere. Relabel to
   `default listen port`.

## Suggested order

1. Stage 0 harness (small, and everything after it is graded by it).
2. The strings sheet (mechanical, closes the doctrine hole).
3. Stage 1.1-1.2 (flags and config): pure surface, no world needed, directly diffable.
4. Stage 2 (world read), then 1.3 (interactive select, which needs the world list).
5. Stage 3 (client loads in) - the first milestone a player can feel.
6. Stage 4, then Stage 5 subsystem by subsystem, then Stage 6.
