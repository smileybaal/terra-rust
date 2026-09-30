//! worlddata: the messages that hand a client the world.
//!
//! Five message bodies, all built the way `NetMessage.SendData` builds them
//! (`NetMessage.cs:233-457`) and all checked against the way the CLIENT reads them
//! (`MessageBuffer.cs:473-951`). Those two are the two halves of one contract, and both
//! were read rather than one being trusted: the server's writer and the client's reader
//! agree field for field, which is what makes a single transcribed vector meaningful.
//!
//! | id | name            | built at          | read at           |
//! |----|-----------------|-------------------|-------------------|
//! | 7  | WorldData       | NetMessage.cs:233 | MessageBuffer.cs:473 |
//! | 9  | StatusTextSize  | NetMessage.cs:428 | MessageBuffer.cs:877 |
//! | 10 | TileSection     | NetMessage.cs:436 | MessageBuffer.cs:889 |
//! | 11 | TileFrameSection| NetMessage.cs:439 | MessageBuffer.cs:895 |
//! | 12 | PlayerSpawn     | NetMessage.cs:445 | MessageBuffer.cs:901 |
//!
//! # What the world file already knows, and what it does not
//!
//! `WorldData` is 60-odd fields, and the great majority of them are not new state: they
//! are the world header and the flag section, which `worldfile` already reads from the
//! `.wld`. This module therefore does not carry a copy of them. It takes the two structs
//! and reorders them onto the wire. The reordering is the risky part and it is real: the
//! message writes the 13 background styles as treeBG1, treeBG2, treeBG3, treeBG4, then the
//! nine biome styles, while the FILE stores them as treeBG1, then the nine biome styles,
//! then treeBG2..4. Writing them in file order would put four wrong backgrounds on the
//! client's sky, so the two orders are named rather than indexed blind.
//!
//! What is left over is genuinely runtime state the file does not record: which event is
//! running tonight, whether a skyblock world has revealed its tiles, a lobby id. Those
//! live in `Runtime`, defaulted to "nothing is happening", and each says so.

use crate::worldfile::{WorldFlags, WorldHeader};

/// Message ids, from `Terraria.ID.MessageID`.
pub const WORLD_DATA: u8 = 7;
pub const STATUS_TEXT_SIZE: u8 = 9;
pub const TILE_SECTION: u8 = 10;
pub const TILE_FRAME_SECTION: u8 = 11;
pub const PLAYER_SPAWN: u8 = 12;

/// `Lang.inter[44]` is `Language.GetText("LegacyInterface.44")` (`Lang.cs:488`), and
/// `LegacyInterface.44` is "Receiving tile data" (`en-US.Legacy.json`). The status text is
/// sent as a LOCALIZATION KEY, not as the English string, so the client renders it in
/// whatever language it is running.
pub const RECEIVING_TILE_DATA_KEY: &str = "LegacyInterface.44";

/// `NetworkText.Mode` (`NetworkText.cs`): the wire byte in front of every `NetworkText`.
pub const NETWORK_TEXT_LITERAL: u8 = 0;
pub const NETWORK_TEXT_FORMATTABLE: u8 = 1;
pub const NETWORK_TEXT_LOCALIZATION_KEY: u8 = 2;

/// The world state that the `.wld` file does not carry.
///
/// Every field here is genuinely absent from the file - not merely inconvenient to read -
/// and each default means "not happening", which is what a dedicated server that has just
/// loaded a world is doing. Anything the file DOES carry is taken from `WorldFlags`
/// instead, so this struct cannot drift away from the world it describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Runtime {
    /// `Main.pumpkinMoon`. An event that is running now, not a property of the world.
    pub pumpkin_moon: bool,
    /// `Main.snowMoon`.
    pub snow_moon: bool,
    /// `DD2Event.Ongoing`. A live invasion.
    pub dd2_ongoing: bool,
    /// `WorldGen.Skyblock.lowTiles`: whether a skyblock world has revealed its tiles yet.
    pub skyblock_low_tiles: bool,
    /// `NPC.freeCake`. Not saved by `SaveWorldFlags`.
    pub free_cake: bool,
    /// `SocialAPI.Network.GetLobbyId()`. A dedicated server has no social API, so the C#
    /// writes `0uL` (`NetMessage.cs:415`), which is also this default.
    pub lobby_id: u64,
}

/// The world, as the wire needs it: the two file structs plus the runtime bits.
#[derive(Debug, Clone, Copy)]
pub struct WorldFacts<'a> {
    pub header: &'a WorldHeader,
    pub flags: &'a WorldFlags,
    pub runtime: Runtime,
}

/// A `BitsByte`: eight flags in one byte, LSB first.
///
/// The C# uses an indexer, `bits[3] = x`, and this is the same thing with the same
/// numbering, so a transcription of the field list reads the same on both sides.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct BitsByte(u8);

impl BitsByte {
    fn set(&mut self, bit: u8, value: bool) {
        if value {
            self.0 |= 1 << bit;
        }
    }

    fn get(self) -> u8 {
        self.0
    }
}

fn push_i16(out: &mut Vec<u8>, v: i16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push_string(out: &mut Vec<u8>, s: &str) {
    crate::net::write_string(out, s).expect("writing to a Vec cannot fail");
}

/// Build the body of `WorldData` (7). `NetMessage.cs:233-421`.
///
/// Every field is written in the C#'s order, because the client reads them in that order
/// and a single transposition shifts everything after it.
pub fn write_world_data(out: &mut Vec<u8>, facts: &WorldFacts) {
    let h = facts.header;
    let f = facts.flags;
    let r = &facts.runtime;

    // The clock. `Main.time` is a double in the world and an int on the wire.
    push_i32(out, f.time as i32);
    let mut bits = BitsByte::default();
    bits.set(0, f.day_time);
    bits.set(1, f.blood_moon);
    bits.set(2, f.eclipse);
    out.push(bits.get());
    out.push(f.moon_phase as u8);
    push_i16(out, h.max_tiles_x as i16);
    push_i16(out, h.max_tiles_y as i16);
    push_i16(out, h.spawn_tile_x as i16);
    push_i16(out, h.spawn_tile_y as i16);
    // `worldSurface` and `rockLayer` are doubles in the file and shorts on the wire.
    push_i16(out, h.world_surface as i16);
    push_i16(out, h.rock_layer as i16);
    push_i32(out, h.world_id);
    push_string(out, &h.name);
    out.push(h.game_mode as u8);
    // `Guid.ToByteArray()`: the raw 16 bytes, in .NET's mixed-endian order. The client
    // does `new Guid(bytes)`, which undoes it, so the bytes are copied and not reordered.
    out.extend_from_slice(&h.unique_id);
    push_u64(out, h.generator_version);
    out.push(h.moon_type);

    // The 13 background styles, in the MESSAGE's order, which is not the file's.
    // `NetMessage.cs:254-265` writes treeBG1, treeBG2, treeBG3, treeBG4 and only then the
    // nine biome styles; the file writes treeBG1, the nine biome styles, treeBG2..4.
    // `setBG` (`WorldGen.cs:7165`) is what fixes the index meanings: 0 is treeBG1, 1..9 are
    // corruption..underworld, 10..12 are treeBG2..4.
    const TREE_BG1: usize = 0;
    const CORRUPTION: usize = 1;
    const JUNGLE: usize = 2;
    const SNOW: usize = 3;
    const HALLOW: usize = 4;
    const CRIMSON: usize = 5;
    const DESERT: usize = 6;
    const OCEAN: usize = 7;
    const MUSHROOM: usize = 8;
    const UNDERWORLD: usize = 9;
    const TREE_BG2: usize = 10;
    const TREE_BG3: usize = 11;
    const TREE_BG4: usize = 12;
    out.push(f.bg[TREE_BG1]);
    out.push(f.bg[TREE_BG2]);
    out.push(f.bg[TREE_BG3]);
    out.push(f.bg[TREE_BG4]);
    out.push(f.bg[CORRUPTION]);
    out.push(f.bg[JUNGLE]);
    out.push(f.bg[SNOW]);
    out.push(f.bg[HALLOW]);
    out.push(f.bg[CRIMSON]);
    out.push(f.bg[DESERT]);
    out.push(f.bg[OCEAN]);
    out.push(f.bg[MUSHROOM]);
    out.push(f.bg[UNDERWORLD]);

    out.push(h.ice_back_style as u8);
    out.push(h.jungle_back_style as u8);
    out.push(h.hell_back_style as u8);
    push_f32(out, f.wind_speed_target);
    // `Main.numClouds` is a short in the file and a byte on the wire.
    out.push(f.num_clouds as u8);
    for v in h.tree_x {
        push_i32(out, v);
    }
    for v in h.tree_style {
        out.push(v as u8);
    }
    for v in h.cave_back_x {
        push_i32(out, v);
    }
    for v in h.cave_back_style {
        out.push(v as u8);
    }
    // `TreeTops.SyncSend` (`TreeTopsInfo.cs:65`): the 13 variations, one byte each.
    for v in f.tree_tops {
        out.push(v as u8);
    }

    // `NetMessage.cs:289-292`: rain is zeroed on the wire when it is not raining, so a
    // client that has just joined does not see a storm the server is not having.
    push_f32(out, if f.raining { f.max_rain } else { 0.0 });

    // -- the eleven progress bytes -------------------------------------------
    // `NetMessage.cs:294-398` writes eleven `BitsByte`s between `maxRaining` and the two
    // dials. Eleven, not thirteen: they are `bitsByte4` through `bitsByte14`, and the two
    // numbers differ because the C#'s local numbering starts at 4.
    let mut b = BitsByte::default();
    b.set(0, f.shadow_orb_smashed);
    b.set(1, f.downed_boss1);
    b.set(2, f.downed_boss2);
    b.set(3, f.downed_boss3);
    b.set(4, f.hard_mode);
    b.set(5, f.downed_clown);
    // Bit 6 is `Main.ServerSideCharacter`, a SERVER setting, and the writer leaves it
    // clear; the client's reader does read it. Left clear, as the C# leaves it.
    b.set(7, f.downed_plant_boss);
    out.push(b.get());

    let mut b = BitsByte::default();
    b.set(0, f.downed_mech_boss1);
    b.set(1, f.downed_mech_boss2);
    b.set(2, f.downed_mech_boss3);
    b.set(3, f.downed_mech_boss_any);
    // `Main.cloudBGActive >= 1f`. The file's own `cloudBGActive` is read and then
    // OVERWRITTEN with `-WorldGen.genRand.Next(8640, 86400)` (`WorldFile.cs:2149`), so by
    // the time a client can join it is always negative. The bit is therefore always 0, and
    // that is computed from the loader's rule rather than from the file's stale value.
    b.set(4, false);
    b.set(5, f.crimson);
    b.set(6, r.pumpkin_moon);
    b.set(7, r.snow_moon);
    out.push(b.get());

    let mut b = BitsByte::default();
    // Bit 0 is unused by the writer.
    b.set(1, f.fast_forward_time_to_dawn);
    // `Main.slimeRain` is `Main.slimeRainTime > 0.0` after a load (`WorldFile.cs:1027`).
    b.set(2, f.slime_rain_time > 0.0);
    b.set(3, f.downed_slime_king);
    b.set(4, f.downed_queen_bee);
    b.set(5, f.downed_fishron);
    b.set(6, f.downed_martians);
    b.set(7, f.downed_ancient_cultist);
    out.push(b.get());

    let mut b = BitsByte::default();
    b.set(0, f.downed_moonlord);
    b.set(1, f.downed_halloween_king);
    b.set(2, f.downed_halloween_tree);
    b.set(3, f.downed_christmas_ice_queen);
    b.set(4, f.downed_christmas_santank);
    b.set(5, f.downed_christmas_tree);
    b.set(6, f.downed_golem_boss);
    // The writer sends `BirthdayParty.PartyIsUp`; the client reads it into
    // `ManualParty`. `PartyIsUp` is `GenuineParty || ManualParty` (`BirthdayParty.cs:23`).
    b.set(7, f.party_genuine || f.party_manual);
    out.push(b.get());

    let mut b = BitsByte::default();
    b.set(0, f.downed_pirates);
    b.set(1, f.downed_frost);
    b.set(2, f.downed_goblins);
    b.set(3, f.sandstorm_happening);
    b.set(4, r.dd2_ongoing);
    b.set(5, f.downed_invasion_t1);
    b.set(6, f.downed_invasion_t2);
    b.set(7, f.downed_invasion_t3);
    out.push(b.get());

    let mut b = BitsByte::default();
    b.set(0, f.combat_book_was_used);
    // The writer sends `LanternNight.LanternsUp`; the client reads it into
    // `ManualLanterns`. `LanternsUp` is `GenuineLanterns || ManualLanterns`.
    b.set(1, f.lantern_night_genuine || f.lantern_night_manual);
    b.set(2, f.downed_tower_solar);
    b.set(3, f.downed_tower_vortex);
    b.set(4, f.downed_tower_nebula);
    b.set(5, f.downed_tower_stardust);
    b.set(6, f.force_halloween_today);
    b.set(7, f.force_xmas_today);
    out.push(b.get());

    let mut b = BitsByte::default();
    b.set(0, f.bought_cat);
    b.set(1, f.bought_dog);
    b.set(2, f.bought_bunny);
    b.set(3, r.free_cake);
    b.set(4, h.drunk_world);
    b.set(5, f.downed_empress_of_light);
    b.set(6, f.downed_queen_slime);
    b.set(7, h.get_good_world);
    out.push(b.get());

    let mut b = BitsByte::default();
    b.set(0, h.tenth_anniversary_world);
    b.set(1, h.dont_starve_world);
    b.set(2, f.downed_deerclops);
    b.set(3, h.not_the_bees_world);
    b.set(4, h.remix_world);
    b.set(5, f.unlocked_slime_blue_spawn);
    b.set(6, f.combat_book_volume_two);
    b.set(7, f.peddlers_satchel);
    out.push(b.get());

    let mut b = BitsByte::default();
    b.set(0, f.unlocked_slime_green_spawn);
    b.set(1, f.unlocked_slime_old_spawn);
    b.set(2, f.unlocked_slime_purple_spawn);
    b.set(3, f.unlocked_slime_rainbow_spawn);
    b.set(4, f.unlocked_slime_red_spawn);
    b.set(5, f.unlocked_slime_yellow_spawn);
    b.set(6, f.unlocked_slime_copper_spawn);
    b.set(7, f.fast_forward_time_to_dusk);
    out.push(b.get());

    let mut b = BitsByte::default();
    b.set(0, h.no_traps_world);
    b.set(1, h.zenith_world);
    b.set(2, f.unlocked_truffle_spawn);
    b.set(3, f.vampire_seed);
    b.set(4, f.infected_seed);
    b.set(5, f.team_based_spawns_seed);
    b.set(6, h.skyblock_world);
    b.set(7, f.dual_dungeons_seed);
    out.push(b.get());

    let mut b = BitsByte::default();
    b.set(0, r.skyblock_low_tiles);
    b.set(1, f.force_halloween_forever);
    b.set(2, f.force_xmas_forever);
    b.set(3, f.more_lightning_seed);
    b.set(4, f.no_lightning_seed);
    out.push(b.get());

    out.push(f.sundial_cooldown);
    out.push(f.moondial_cooldown);
    push_i16(out, f.ore_copper as i16);
    push_i16(out, f.ore_iron as i16);
    push_i16(out, f.ore_silver as i16);
    push_i16(out, f.ore_gold as i16);
    push_i16(out, f.ore_cobalt as i16);
    push_i16(out, f.ore_mythril as i16);
    push_i16(out, f.ore_adamantite as i16);
    out.push(f.invasion_type as i8 as u8);
    push_u64(out, r.lobby_id);
    push_f32(out, f.sandstorm_intended_severity);
    // `ExtraSpawnPointManager.Write` (`ExtraSpawnPointManager.cs:449`): a byte count then
    // that many x/y int16 pairs. The `networking` argument changes nothing.
    out.push(f.extra_spawn_points.len() as u8);
    for (x, y) in &f.extra_spawn_points {
        push_i16(out, *x);
        push_i16(out, *y);
    }
    push_i16(out, f.dungeon_x as i16);
    push_i16(out, f.dungeon_y as i16);
}

/// Build the body of `StatusTextSize` (9). `NetMessage.cs:428-435`.
///
/// `NetMessage.cs:811` sends it as `TrySendData(9, whoAmi, -1, Lang.inter[44].ToNetworkText(),
/// num201)`, so the text is the localization KEY `LegacyInterface.44` and `status_max` is
/// the number of tile sections the client is about to be sent - which is what makes the
/// client's loading bar measure something real.
pub fn write_status_text_size(out: &mut Vec<u8>, status_max: i32, key: &str, flags: u8) {
    push_i32(out, status_max);
    // `NetworkText.Serialize` (`NetworkText.cs`): mode byte, the text, and - for anything
    // but a literal - a substitution count. `FromKey` with no substitutions means a
    // LocalizationKey with an empty list, and the empty list is still written.
    out.push(NETWORK_TEXT_LOCALIZATION_KEY);
    push_string(out, key);
    out.push(0);
    out.push(flags);
}

/// Build the body of `PlayerSpawn` (12). `NetMessage.cs:445-457`.
///
/// The client's reader forces the slot to `whoAmI` on a server, so the slot byte is only
/// meaningful to another client; it is written anyway because the C# writes it.
pub fn write_player_spawn(
    out: &mut Vec<u8>,
    slot: u8,
    spawn_x: i16,
    spawn_y: i16,
    respawn_timer: i32,
    deaths_pve: i16,
    deaths_pvp: i16,
    team: u8,
    context: u8,
) {
    out.push(slot);
    push_i16(out, spawn_x);
    push_i16(out, spawn_y);
    push_i32(out, respawn_timer);
    push_i16(out, deaths_pve);
    push_i16(out, deaths_pvp);
    out.push(team);
    out.push(context);
}

/// Build the body of `TileFrameSection` (11). `NetMessage.cs:439-444`.
///
/// Four int16s: the inclusive section rectangle the client should re-frame.
///
/// NOTE: nothing in the native server ever sends this. `NetMessage.SendData` has a case
/// for 11 and `MessageBuffer` has a reader for it, but no call site passes 11 - the only
/// senders of tile sections are `SendSection`, which sends 10 and only 10. It is
/// implemented because it is part of the protocol and cheap to get right, and it is
/// labelled here because an implementation that no code path can reach is not the same
/// thing as a working feature, and should not be counted as one.
pub fn write_frame_section(out: &mut Vec<u8>, start_x: i16, start_y: i16, end_x: i16, end_y: i16) {
    push_i16(out, start_x);
    push_i16(out, start_y);
    push_i16(out, end_x);
    push_i16(out, end_y);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> WorldHeader {
        WorldHeader {
            name: "Test".into(),
            seed: "1".into(),
            generator_version: 0,
            unique_id: [0; 16],
            world_id: 7,
            left_world: 0,
            right_world: 67200,
            top_world: 0,
            bottom_world: 19200,
            max_tiles_x: 4200,
            max_tiles_y: 1200,
            game_mode: 0,
            drunk_world: false,
            get_good_world: false,
            tenth_anniversary_world: false,
            dont_starve_world: false,
            not_the_bees_world: false,
            remix_world: false,
            no_traps_world: false,
            zenith_world: false,
            skyblock_world: false,
            creation_time_binary: 0,
            last_played_binary: 0,
            moon_type: 0,
            tree_x: [1, 2, 3],
            tree_style: [0, 0, 0, 0],
            cave_back_x: [4, 5, 6],
            cave_back_style: [0, 0, 0, 0],
            ice_back_style: 0,
            jungle_back_style: 0,
            hell_back_style: 0,
            spawn_tile_x: 100,
            spawn_tile_y: 200,
            world_surface: 300.0,
            rock_layer: 400.0,
        }
    }

    /// A `BitsByte` numbers its bits from the least significant end, which is the only
    /// thing that makes the eleven progress bytes mean what the C# means.
    #[test]
    fn a_bits_byte_is_lsb_first() {
        let mut b = BitsByte::default();
        b.set(0, true);
        assert_eq!(b.get(), 0b0000_0001);
        let mut b = BitsByte::default();
        b.set(7, true);
        assert_eq!(b.get(), 0b1000_0000);
        let mut b = BitsByte::default();
        b.set(2, true);
        b.set(4, true);
        assert_eq!(b.get(), 0b0001_0100);
    }

    /// The background styles are the one place where the message's field order differs
    /// from the file's, so it is the one place a transposition is invisible to a round
    /// trip. This pins the actual bytes: a distinct value per slot, read back in the
    /// MESSAGE's order.
    #[test]
    fn the_background_styles_are_reordered_not_copied() {
        let h = header();
        let mut f = WorldFlags::default();
        // Value 10 + i for setBG slot i, so every slot is distinguishable.
        for (i, slot) in f.bg.iter_mut().enumerate() {
            *slot = 10 + i as u8;
        }
        let facts = WorldFacts { header: &h, flags: &f, runtime: Runtime::default() };
        let mut out = Vec::new();
        write_world_data(&mut out, &facts);

        // Walk to the styles the way the CLIENT does, so the offsets are derived from the
        // reader rather than counted by hand.
        let mut i = 0usize;
        i += 4; // time
        i += 1; // bits
        i += 1; // moon phase
        i += 2 * 6; // maxX, maxY, spawnX, spawnY, surface, rock
        i += 4; // world id
        i += 1 + "Test".len(); // world name, 7-bit length of 4 is one byte
        i += 1; // game mode
        i += 16; // guid
        i += 8; // generator version
        i += 1; // moon type
        let styles = &out[i..i + 13];
        assert_eq!(
            styles,
            &[10, 20, 21, 22, 11, 12, 13, 14, 15, 16, 17, 18, 19],
            "the message writes treeBG1, treeBG2..4, then the nine biome styles"
        );
    }

    /// The whole body has to be exactly as long as the client's reader consumes, or the
    /// client reads the next packet's bytes as the tail of this one. This walks the
    /// reader's field list and asserts the two agree.
    #[test]
    fn the_world_body_is_exactly_as_long_as_the_client_reads() {
        let h = header();
        let mut f = WorldFlags::default();
        f.extra_spawn_points = vec![(1, 2), (3, 4)];
        f.tree_tops = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
        let facts = WorldFacts { header: &h, flags: &f, runtime: Runtime::default() };
        let mut out = Vec::new();
        write_world_data(&mut out, &facts);

        let expected = 4      // time
            + 1               // day/blood/eclipse bits
            + 1               // moon phase
            + 2 * 6           // maxX, maxY, spawnX, spawnY, surface, rock
            + 4               // world id
            + 5               // "Test" with its one-byte length
            + 1               // game mode
            + 16              // guid
            + 8               // generator version
            + 1               // moon type
            + 13              // background styles
            + 3               // ice, jungle, hell
            + 4               // wind speed target
            + 1               // num clouds
            + 3 * 4           // tree x
            + 4               // tree style
            + 3 * 4           // cave back x
            + 4               // cave back style
            + 13              // tree tops
            + 4               // max raining
            + 11              // the eleven progress bytes (bitsByte4..bitsByte14)
            + 2               // sundial, moondial
            + 7 * 2           // the seven ore tiers
            + 1               // invasion type
            + 8               // lobby id
            + 4               // sandstorm severity
            + 1 + 2 * 2 * 2   // extra spawn points: count + two points
            + 2 * 2; // dungeon x, dungeon y
        assert_eq!(out.len(), expected, "the body must be exactly the reader's length");
    }

    /// Message 9's text is a KEY, not a string. Sending the English text instead would
    /// work in English and be wrong in every other language, so the mode byte is pinned.
    #[test]
    fn the_status_text_is_a_localization_key() {
        let mut out = Vec::new();
        write_status_text_size(&mut out, 42, RECEIVING_TILE_DATA_KEY, 0);
        assert_eq!(&out[0..4], &42i32.to_le_bytes());
        assert_eq!(out[4], NETWORK_TEXT_LOCALIZATION_KEY);
        assert_eq!(out[5] as usize, RECEIVING_TILE_DATA_KEY.len());
        assert_eq!(&out[6..6 + RECEIVING_TILE_DATA_KEY.len()], RECEIVING_TILE_DATA_KEY.as_bytes());
        // The empty substitution list is still written: a reader that expects it would
        // otherwise take the next byte as the count.
        assert_eq!(out[6 + RECEIVING_TILE_DATA_KEY.len()], 0);
        assert_eq!(out.len(), 6 + RECEIVING_TILE_DATA_KEY.len() + 1 + 1);
    }

    /// Message 12's fields are four different widths, and the client reads them in this
    /// order: byte, short, short, int, short, short, byte, byte.
    #[test]
    fn a_player_spawn_is_eight_fields_of_four_widths() {
        let mut out = Vec::new();
        write_player_spawn(&mut out, 3, 100, 200, 0, 1, 2, 5, 0);
        assert_eq!(
            out,
            vec![3, 100, 0, 200, 0, 0, 0, 0, 0, 1, 0, 2, 0, 5, 0]
        );
        assert_eq!(out.len(), 15);
    }

    /// Message 11 is four int16s, and nothing calls it. The test is here so the body is
    /// not merely believed to be right; the doc comment says what it cannot prove.
    #[test]
    fn a_frame_section_is_four_shorts() {
        let mut out = Vec::new();
        write_frame_section(&mut out, 1, 2, 3, 4);
        assert_eq!(out, vec![1, 0, 2, 0, 3, 0, 4, 0]);
    }
}
