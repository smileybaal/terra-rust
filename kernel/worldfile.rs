//! worldfile: read a Terraria `.wld`.
//!
//! The first slice of the world work, and the one with no conflicts: it depends on
//! bytes and nothing else - no sockets, no player, no game state. `WorldFile` is 56
//! methods in the book; this covers the container and the world's identity, which is
//! what the world-select list and `WorldData` (7) need first.
//!
//! Container, from `WorldFile.cs:1907` `LoadFileFormatHeader`:
//!
//! ```text
//! int32   version
//! (version >= 135) uint64 magic, uint32 revision, uint64 flags
//! int16   sectionCount, then int32 position[sectionCount]
//! uint16  importanceCount, then that many BITS, LSB-first within each byte
//! ```
//!
//! The section pointers are the reason this parses at all: `positions[0]` is where the
//! world header starts, so a container parse can be checked EXACTLY rather than
//! approximately - see the test that asserts the stream lands on it.
//!
//! Header, from `WorldFile.cs:1957` `LoadHeader` and `:1998` `LoadWorldFlags`. Both are
//! read: `read_header` stops after `rockLayer`, which is where the container's pointer
//! check needs it to, and `read_flags` walks the rest of the same record. Every section
//! after the header (tiles, chests, signs, NPCs, tile entities, pressure plates, town
//! manager, bestiary, creative powers) is the next slice.

/// `FileMetadata.cs:66`: `(num & 0x00FFFFFFFFFFFFFF) == 27981915666277746`.
///
/// The low 7 bytes are the magic; the top byte is the file type.
pub const MAGIC: u64 = 27981915666277746;

/// `FileType.cs`: `None, Map, World, Player`, so World is 2.
pub const FILE_TYPE_WORLD: u8 = 2;

/// The version this reader is known to understand. `a.wld` and `d.wld` on the machine
/// this was written on both report 279.
pub const KNOWN_VERSION: i32 = 279;

/// Why a `.wld` could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldFileError {
    /// The data ended before the structure did.
    TooShort,
    /// The 7-byte magic did not match: this is not a Re-Logic file.
    BadMagic,
    /// The magic matched but the type byte was not `World`.
    NotAWorld,
}

/// The file container: the version, the section table, and the importance bits.
#[derive(Debug, Clone, PartialEq)]
pub struct Container {
    pub version: i32,
    pub revision: u32,
    pub favorite: bool,
    /// Byte offset of each section. `positions[0]` is the world header.
    pub positions: Vec<i32>,
    /// One bit per tile id: whether the tile is "important" (carries data even when
    /// its id is 0). Packed LSB-first.
    pub importance: Vec<bool>,
}

/// The world's identity and geometry, as far as `rockLayer`.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldHeader {
    pub name: String,
    pub seed: String,
    pub generator_version: u64,
    pub unique_id: [u8; 16],
    pub world_id: i32,
    pub left_world: i32,
    pub right_world: i32,
    pub top_world: i32,
    pub bottom_world: i32,
    pub max_tiles_x: i32,
    pub max_tiles_y: i32,
    pub game_mode: i32,
    /// The special-seed booleans, in the order the C# reads them (version >= 302).
    pub drunk_world: bool,
    pub get_good_world: bool,
    pub tenth_anniversary_world: bool,
    pub dont_starve_world: bool,
    pub not_the_bees_world: bool,
    pub remix_world: bool,
    pub no_traps_world: bool,
    pub zenith_world: bool,
    pub skyblock_world: bool,
    /// `DateTime.FromBinary` ticks, kept raw: the C# converts, and the conversion is
    /// `DateTime`-specific rather than a fact about the file.
    pub creation_time_binary: i64,
    pub last_played_binary: i64,
    pub moon_type: u8,
    pub tree_x: [i32; 3],
    pub tree_style: [i32; 4],
    pub cave_back_x: [i32; 3],
    pub cave_back_style: [i32; 4],
    pub ice_back_style: i32,
    pub jungle_back_style: i32,
    pub hell_back_style: i32,
    pub spawn_tile_x: i32,
    pub spawn_tile_y: i32,
    pub world_surface: f64,
    pub rock_layer: f64,
}

/// The rest of the world's flag section, i.e. the part of `WorldFile.cs:1986`
/// `LoadWorldFlags` that `read_header` does not already return.
///
/// The split is not arbitrary: `read_header` ends at `rock_layer` (`WorldFile.cs:2076`),
/// which is the last field the container's `positions[0]` pointer needs in order to be
/// checked. Everything after it is still part of the SAME flat record, so it cannot be
/// skipped - each field is read to advance the stream - and `WorldData`(7) sends almost
/// every one of them to the client. A world whose surface is right but whose dungeon
/// position is invented puts the client's dungeon in the wrong place.
///
/// Where the C# keeps a value in a `_tempX` static and applies it after the whole record
/// is read, this stores the temp under the name the file uses and lets the caller apply
/// it, rather than applying it here and losing the distinction.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorldFlags {
    // -- clock and sky -------------------------------------------------------
    /// `_tempTime`. A `double` in the file, cast to `int` for the wire.
    pub time: f64,
    pub day_time: bool,
    /// `_tempMoonPhase`. An `int` in the file, sent as a byte.
    pub moon_phase: i32,
    pub blood_moon: bool,
    pub eclipse: bool,
    // -- dungeon -------------------------------------------------------------
    pub dungeon_x: i32,
    pub dungeon_y: i32,
    /// `WorldGen.crimson`: whether the world generated crimson instead of corruption.
    pub crimson: bool,
    // -- progress ------------------------------------------------------------
    pub downed_boss1: bool,
    pub downed_boss2: bool,
    pub downed_boss3: bool,
    pub downed_queen_bee: bool,
    pub downed_mech_boss1: bool,
    pub downed_mech_boss2: bool,
    pub downed_mech_boss3: bool,
    pub downed_mech_boss_any: bool,
    pub downed_plant_boss: bool,
    pub downed_golem_boss: bool,
    pub downed_slime_king: bool,
    pub downed_goblins: bool,
    pub downed_clown: bool,
    pub downed_frost: bool,
    pub downed_pirates: bool,
    pub downed_fishron: bool,
    pub downed_martians: bool,
    pub downed_ancient_cultist: bool,
    pub downed_moonlord: bool,
    pub downed_halloween_king: bool,
    pub downed_halloween_tree: bool,
    pub downed_christmas_ice_queen: bool,
    pub downed_christmas_santank: bool,
    pub downed_christmas_tree: bool,
    pub downed_tower_solar: bool,
    pub downed_tower_vortex: bool,
    pub downed_tower_nebula: bool,
    pub downed_tower_stardust: bool,
    pub downed_empress_of_light: bool,
    pub downed_queen_slime: bool,
    pub downed_deerclops: bool,
    pub saved_goblin: bool,
    pub saved_wizard: bool,
    pub saved_mech: bool,
    pub saved_angler: bool,
    pub saved_stylist: bool,
    pub saved_tax_collector: bool,
    pub saved_golfer: bool,
    pub saved_bartender: bool,
    /// `WorldGen.shadowOrbSmashed`.
    pub shadow_orb_smashed: bool,
    pub shadow_orb_count: u8,
    pub altar_count: i32,
    pub hard_mode: bool,
    pub after_party_of_doom: bool,
    // -- invasion ------------------------------------------------------------
    pub invasion_delay: i32,
    pub invasion_size: i32,
    pub invasion_type: i32,
    pub invasion_x: f64,
    pub invasion_size_start: i32,
    pub slime_rain_time: f64,
    // -- weather -------------------------------------------------------------
    pub raining: bool,
    pub rain_time: i32,
    pub max_rain: f32,
    pub sandstorm_happening: bool,
    pub sandstorm_time_left: i32,
    pub sandstorm_severity: f32,
    pub sandstorm_intended_severity: f32,
    pub num_clouds: i16,
    pub wind_speed_target: f32,
    pub cloud_bg_active: i32,
    // -- dials ---------------------------------------------------------------
    pub sundial_cooldown: u8,
    pub moondial_cooldown: u8,
    // -- background styles ---------------------------------------------------
    /// The 13 styles, indexed the way `WorldGen.setBG` indexes them: 0 is `treeBG1`,
    /// 1..9 are the biome backgrounds, 10..12 are `treeBG2..4`. Stored in setBG order
    /// and reordered only at the wire, where the message writes a different order.
    pub bg: [u8; 13],
    /// `WorldGen.TreeTops`: 13 variations, in `AreaId` order.
    pub tree_tops: [i32; 13],
    // -- ore ----------------------------------------------------------------
    pub ore_copper: i32,
    pub ore_iron: i32,
    pub ore_silver: i32,
    pub ore_gold: i32,
    pub ore_cobalt: i32,
    pub ore_mythril: i32,
    pub ore_adamantite: i32,
    // -- misc progress -------------------------------------------------------
    pub angler_quest: i32,
    pub cultist_delay: i32,
    pub fast_forward_time_to_dawn: bool,
    pub fast_forward_time_to_dusk: bool,
    pub party_manual: bool,
    pub party_genuine: bool,
    pub party_cooldown: i32,
    pub party_celebrating_npcs: Vec<i32>,
    pub downed_invasion_t1: bool,
    pub downed_invasion_t2: bool,
    pub downed_invasion_t3: bool,
    pub combat_book_was_used: bool,
    pub combat_book_volume_two: bool,
    pub peddlers_satchel: bool,
    pub lantern_night_cooldown: i32,
    pub lantern_night_genuine: bool,
    pub lantern_night_manual: bool,
    pub force_halloween_today: bool,
    pub force_xmas_today: bool,
    pub force_halloween_forever: bool,
    pub force_xmas_forever: bool,
    pub bought_cat: bool,
    pub bought_dog: bool,
    pub bought_bunny: bool,
    pub unlocked_slime_blue_spawn: bool,
    pub unlocked_slime_green_spawn: bool,
    pub unlocked_slime_old_spawn: bool,
    pub unlocked_slime_purple_spawn: bool,
    pub unlocked_slime_rainbow_spawn: bool,
    pub unlocked_slime_red_spawn: bool,
    pub unlocked_slime_yellow_spawn: bool,
    pub unlocked_slime_copper_spawn: bool,
    pub unlocked_truffle_spawn: bool,
    pub unlocked_merchant_spawn: bool,
    pub unlocked_demolitionist_spawn: bool,
    pub unlocked_party_girl_spawn: bool,
    pub unlocked_dye_trader_spawn: bool,
    pub unlocked_arms_dealer_spawn: bool,
    pub unlocked_nurse_spawn: bool,
    pub unlocked_princess_spawn: bool,
    pub vampire_seed: bool,
    pub infected_seed: bool,
    pub team_based_spawns_seed: bool,
    pub dual_dungeons_seed: bool,
    pub more_lightning_seed: bool,
    pub no_lightning_seed: bool,
    /// `ExtraSpawnPointManager`: up to 255 points, each an x/y tile pair.
    pub extra_spawn_points: Vec<(i16, i16)>,
    /// `WorldGen.Manifest`, as the raw string. Empty before version 299, which stored
    /// no manifest at all - not an empty one.
    pub manifest: String,
    /// The version this was read for, so a caller can tell `read_flags` refused a gate.
    pub version: i32,
}

// -- primitive readers -------------------------------------------------------
//
// `&[u8]` implements `Read` and `read_exact` advances it, so the stream IS the slice
// and the position is `whole.len() - rest.len()`. No cursor object, no second copy of
// the 7-bit string rule: `net::read_string` is reused.

fn take<'a>(r: &mut &'a [u8], n: usize) -> Result<&'a [u8], WorldFileError> {
    if r.len() < n {
        return Err(WorldFileError::TooShort);
    }
    let (head, tail) = r.split_at(n);
    *r = tail;
    Ok(head)
}

fn u8_of(r: &mut &[u8]) -> Result<u8, WorldFileError> {
    Ok(take(r, 1)?[0])
}

fn bool_of(r: &mut &[u8]) -> Result<bool, WorldFileError> {
    Ok(u8_of(r)? != 0)
}

fn u16_of(r: &mut &[u8]) -> Result<u16, WorldFileError> {
    Ok(u16::from_le_bytes(take(r, 2)?.try_into().unwrap()))
}

fn i16_of(r: &mut &[u8]) -> Result<i16, WorldFileError> {
    Ok(i16::from_le_bytes(take(r, 2)?.try_into().unwrap()))
}

fn u32_of(r: &mut &[u8]) -> Result<u32, WorldFileError> {
    Ok(u32::from_le_bytes(take(r, 4)?.try_into().unwrap()))
}

fn i32_of(r: &mut &[u8]) -> Result<i32, WorldFileError> {
    Ok(i32::from_le_bytes(take(r, 4)?.try_into().unwrap()))
}

fn u64_of(r: &mut &[u8]) -> Result<u64, WorldFileError> {
    Ok(u64::from_le_bytes(take(r, 8)?.try_into().unwrap()))
}

fn i64_of(r: &mut &[u8]) -> Result<i64, WorldFileError> {
    Ok(i64::from_le_bytes(take(r, 8)?.try_into().unwrap()))
}

fn f64_of(r: &mut &[u8]) -> Result<f64, WorldFileError> {
    Ok(f64::from_le_bytes(take(r, 8)?.try_into().unwrap()))
}

fn f32_of(r: &mut &[u8]) -> Result<f32, WorldFileError> {
    Ok(f32::from_le_bytes(take(r, 4)?.try_into().unwrap()))
}

fn string_of(r: &mut &[u8]) -> Result<String, WorldFileError> {
    crate::net::read_string(r).map_err(|_| WorldFileError::TooShort)
}

/// Read the container and report how many bytes it consumed.
///
/// The returned length is the check that matters: `WorldFile.cs:1771` refuses to
/// continue unless the stream is exactly at `positions[0]` after this structure, so a
/// caller can assert the same thing and catch any drift in the metadata size or the
/// importance packing.
pub fn read_container(data: &[u8]) -> Result<(Container, usize), WorldFileError> {
    let mut r = data;
    let version = i32_of(&mut r)?;

    let (revision, favorite) = if version >= 135 {
        let magic = u64_of(&mut r)?;
        if magic & 0x00FF_FFFF_FFFF_FFFF != MAGIC {
            return Err(WorldFileError::BadMagic);
        }
        let file_type = ((magic >> 56) & 0xFF) as u8;
        if file_type != FILE_TYPE_WORLD {
            return Err(WorldFileError::NotAWorld);
        }
        let revision = u32_of(&mut r)?;
        let flags = u64_of(&mut r)?;
        (revision, flags & 1 == 1)
    } else {
        // `FileMetadata.FromCurrentSettings` writes nothing for an old file.
        (0, false)
    };

    let count = i16_of(&mut r)?;
    if count < 0 {
        return Err(WorldFileError::TooShort);
    }
    let mut positions = Vec::with_capacity(count as usize);
    for _ in 0..count {
        positions.push(i32_of(&mut r)?);
    }

    let important = u16_of(&mut r)?;
    let mut importance = Vec::with_capacity(important as usize);
    // LSB-first within each byte: bit i of the block is `(bytes[i / 8] >> (i % 8)) & 1`.
    let mut byte = 0u8;
    for i in 0..important as usize {
        if i % 8 == 0 {
            byte = u8_of(&mut r)?;
        }
        importance.push((byte >> (i % 8)) & 1 == 1);
    }

    Ok((
        Container { version, revision, favorite, positions, importance },
        data.len() - r.len(),
    ))
}

/// Read the world header from the start of `positions[0]`.
///
/// `version` comes from the container: every conditional field in the header is keyed
/// on it, so passing the wrong one mis-reads silently rather than failing.
pub fn read_header(r: &mut &[u8], version: i32) -> Result<WorldHeader, WorldFileError> {
    let name = string_of(r)?;

    let (seed, generator_version) = if version >= 179 {
        // Version 179 exactly stored the seed as an int; every later one as a string.
        let seed = if version != 179 {
            string_of(r)?
        } else {
            i32_of(r)?.to_string()
        };
        (seed, u64_of(r)?)
    } else {
        (String::new(), 0)
    };

    let unique_id: [u8; 16] = if version >= 181 {
        take(r, 16)?.try_into().unwrap()
    } else {
        // The C# invents a GUID for files older than 181 (`WorldFile.cs:1966`). Zero is
        // this reader's way of saying the file carried none, which is not the same thing
        // as a world whose id happens to be all zeros.
        [0u8; 16]
    };

    let world_id = i32_of(r)?;
    let left_world = i32_of(r)?;
    let right_world = i32_of(r)?;
    let top_world = i32_of(r)?;
    let bottom_world = i32_of(r)?;
    // Y before X: the file stores them in this order (`WorldFile.cs:1972`).
    let max_tiles_y = i32_of(r)?;
    let max_tiles_x = i32_of(r)?;

    // Every one of these is version-guarded in the C# (`WorldFile.cs:1998`
    // `LoadWorldFlags`), and reading them unconditionally would shift the whole header
    // silently. The order is the C#'s.
    let game_mode = i32_of(r)?;
    let drunk_world = if version >= 222 { bool_of(r)? } else { false };
    let get_good_world = if version >= 227 { bool_of(r)? } else { false };
    let tenth_anniversary_world = if version >= 238 { bool_of(r)? } else { false };
    let dont_starve_world = if version >= 239 { bool_of(r)? } else { false };
    let not_the_bees_world = if version >= 241 { bool_of(r)? } else { false };
    let remix_world = if version >= 249 { bool_of(r)? } else { false };
    let no_traps_world = if version >= 266 { bool_of(r)? } else { false };
    // Zenith is READ from 267 on, and DERIVED before that.
    let zenith_world = if version >= 267 {
        bool_of(r)?
    } else {
        remix_world && drunk_world
    };
    let skyblock_world = if version >= 302 { bool_of(r)? } else { false };

    let creation_time_binary = if version >= 141 { i64_of(r)? } else { 0 };
    // `>= 284`, not `>= 141`: a 279 world has no last-played field at all.
    let last_played_binary = if version >= 284 { i64_of(r)? } else { 0 };
    let moon_type = u8_of(r)?;

    let mut tree_x = [0i32; 3];
    for v in tree_x.iter_mut() {
        *v = i32_of(r)?;
    }
    let mut tree_style = [0i32; 4];
    for v in tree_style.iter_mut() {
        *v = i32_of(r)?;
    }
    let mut cave_back_x = [0i32; 3];
    for v in cave_back_x.iter_mut() {
        *v = i32_of(r)?;
    }
    let mut cave_back_style = [0i32; 4];
    for v in cave_back_style.iter_mut() {
        *v = i32_of(r)?;
    }
    let ice_back_style = i32_of(r)?;
    let jungle_back_style = i32_of(r)?;
    let hell_back_style = i32_of(r)?;
    let spawn_tile_x = i32_of(r)?;
    let spawn_tile_y = i32_of(r)?;
    let world_surface = f64_of(r)?;
    let rock_layer = f64_of(r)?;

    Ok(WorldHeader {
        name,
        seed,
        generator_version,
        unique_id,
        world_id,
        left_world,
        right_world,
        top_world,
        bottom_world,
        max_tiles_x,
        max_tiles_y,
        game_mode,
        drunk_world,
        get_good_world,
        tenth_anniversary_world,
        dont_starve_world,
        not_the_bees_world,
        remix_world,
        no_traps_world,
        zenith_world,
        skyblock_world,
        creation_time_binary,
        last_played_binary,
        moon_type,
        tree_x,
        tree_style,
        cave_back_x,
        cave_back_style,
        ice_back_style,
        jungle_back_style,
        hell_back_style,
        spawn_tile_x,
        spawn_tile_y,
        world_surface,
        rock_layer,
    })
}

/// Walk the rest of the flag section (`WorldFile.cs:1986` `LoadWorldFlags`).
///
/// Call this with the stream positioned immediately after `rock_layer`, i.e. right after
/// `read_header` has returned. The two together are one flat record; splitting them is a
/// convenience for the container check, not a boundary in the file.
///
/// The C# RETURNS EARLY from this method on several old versions (95, 99, 101, 104, 109,
/// 128, 131, 140). Those returns are reproduced, because a v90 world really does stop
/// there and reading on would consume the next section's bytes as flags. That is why the
/// result carries `version`.
pub fn read_flags(r: &mut &[u8], version: i32) -> Result<WorldFlags, WorldFileError> {
    let mut f = WorldFlags { version, ..Default::default() };

    // -- clock, sky and dungeon ----------------------------------------------
    f.time = f64_of(r)?;
    f.day_time = bool_of(r)?;
    f.moon_phase = i32_of(r)?;
    f.blood_moon = bool_of(r)?;
    f.eclipse = bool_of(r)?;
    f.dungeon_x = i32_of(r)?;
    f.dungeon_y = i32_of(r)?;
    f.crimson = bool_of(r)?;

    // -- boss progress -------------------------------------------------------
    f.downed_boss1 = bool_of(r)?;
    f.downed_boss2 = bool_of(r)?;
    f.downed_boss3 = bool_of(r)?;
    f.downed_queen_bee = bool_of(r)?;
    f.downed_mech_boss1 = bool_of(r)?;
    f.downed_mech_boss2 = bool_of(r)?;
    f.downed_mech_boss3 = bool_of(r)?;
    f.downed_mech_boss_any = bool_of(r)?;
    f.downed_plant_boss = bool_of(r)?;
    f.downed_golem_boss = bool_of(r)?;
    f.downed_slime_king = if version >= 118 { bool_of(r)? } else { false };
    f.saved_goblin = bool_of(r)?;
    f.saved_wizard = bool_of(r)?;
    f.saved_mech = bool_of(r)?;
    f.downed_goblins = bool_of(r)?;
    f.downed_clown = bool_of(r)?;
    f.downed_frost = bool_of(r)?;
    f.downed_pirates = bool_of(r)?;
    f.shadow_orb_smashed = bool_of(r)?;
    // `WorldGen.spawnMeteor` is read and not used by the world message, so it is read
    // for the stream position and dropped rather than stored.
    let _spawn_meteor = bool_of(r)?;
    f.shadow_orb_count = u8_of(r)?;
    f.altar_count = i32_of(r)?;
    f.hard_mode = bool_of(r)?;
    f.after_party_of_doom = if version >= 257 { bool_of(r)? } else { false };

    // -- invasion ------------------------------------------------------------
    f.invasion_delay = i32_of(r)?;
    f.invasion_size = i32_of(r)?;
    f.invasion_type = i32_of(r)?;
    f.invasion_x = f64_of(r)?;
    f.slime_rain_time = if version >= 118 { f64_of(r)? } else { 0.0 };
    f.sundial_cooldown = if version >= 113 { u8_of(r)? } else { 0 };

    // -- rain, before `FixEndlessRainWorlds` gets a chance to clear it ---------
    f.raining = bool_of(r)?;
    f.rain_time = i32_of(r)?;
    f.max_rain = f32_of(r)?;
    // `FixEndlessRainWorlds` (WorldFile.cs:3297) reads NOTHING: it clears the three
    // fields above for a >317 world with an absurd rain timer and no rain secret seed.
    // Reproducing it here would need the secret-seed list, so it is not applied; the
    // three raw values are what the file holds.

    // -- hardmode ore, then the first eight background styles ------------------
    f.ore_cobalt = i32_of(r)?;
    f.ore_mythril = i32_of(r)?;
    f.ore_adamantite = i32_of(r)?;
    for i in 0..8 {
        f.bg[i] = u8_of(r)?;
    }
    f.cloud_bg_active = i32_of(r)?;
    f.num_clouds = i16_of(r)?;
    f.wind_speed_target = f32_of(r)?;

    // -- the version-95 gate: everything below is absent in older files ---------
    if version < 95 {
        return Ok(f);
    }
    let angler_count = i32_of(r)?;
    for _ in 0..angler_count.max(0) {
        let _ = string_of(r)?;
    }

    if version < 99 {
        return Ok(f);
    }
    f.saved_angler = bool_of(r)?;

    if version < 101 {
        return Ok(f);
    }
    f.angler_quest = i32_of(r)?;

    if version < 104 {
        return Ok(f);
    }
    f.saved_stylist = bool_of(r)?;
    f.saved_tax_collector = if version >= 129 { bool_of(r)? } else { false };
    f.saved_golfer = if version >= 201 { bool_of(r)? } else { false };
    // Before 107 the C# invents an invasion start rather than reading one.
    f.invasion_size_start = if version >= 107 { i32_of(r)? } else { 0 };
    f.cultist_delay = if version < 108 { 86400 } else { i32_of(r)? };

    if version < 109 {
        return Ok(f);
    }
    // `BannerSystem.Load` (BannerSystem.cs:188) reads TWO counts: a count of int32 kill
    // counts, and - from version 289 - a count of uint16 claimable banners. The SAVE side
    // (`BannerSystem.Save`) always writes both halves, so the gate is in the READER: a
    // pre-289 file simply has no second half. Reading only the first half is what left
    // d.wld (version 326) 639 bytes short of its own tile section, and the first half
    // alone parses cleanly, so nothing but the section pointer could have caught it.
    let banner_count = i16_of(r)?;
    for _ in 0..banner_count.max(0) {
        let _ = i32_of(r)?;
    }
    if version >= 289 {
        let claimable_count = i16_of(r)?;
        for _ in 0..claimable_count.max(0) {
            let _ = u16_of(r)?;
        }
    }

    if version < 128 {
        return Ok(f);
    }
    f.fast_forward_time_to_dawn = bool_of(r)?;

    if version < 131 {
        return Ok(f);
    }
    f.downed_fishron = bool_of(r)?;
    f.downed_martians = bool_of(r)?;
    f.downed_ancient_cultist = bool_of(r)?;
    f.downed_moonlord = bool_of(r)?;
    f.downed_halloween_king = bool_of(r)?;
    f.downed_halloween_tree = bool_of(r)?;
    f.downed_christmas_ice_queen = bool_of(r)?;
    f.downed_christmas_santank = bool_of(r)?;
    f.downed_christmas_tree = bool_of(r)?;

    if version < 140 {
        return Ok(f);
    }
    f.downed_tower_solar = bool_of(r)?;
    f.downed_tower_vortex = bool_of(r)?;
    f.downed_tower_nebula = bool_of(r)?;
    f.downed_tower_stardust = bool_of(r)?;
    // The four `TowerActive` flags and `LunarApocalypseIsUp` follow. They set shield
    // strengths on load and are not sent, so they are read and dropped.
    let _tower_active_solar = bool_of(r)?;
    let _tower_active_vortex = bool_of(r)?;
    let _tower_active_nebula = bool_of(r)?;
    let _tower_active_stardust = bool_of(r)?;
    let _lunar_apocalypse_is_up = bool_of(r)?;

    if version < 170 {
        f.party_manual = false;
        f.party_genuine = false;
        f.party_cooldown = 0;
    } else {
        f.party_manual = bool_of(r)?;
        f.party_genuine = bool_of(r)?;
        f.party_cooldown = i32_of(r)?;
        let n = i32_of(r)?;
        for _ in 0..n.max(0) {
            f.party_celebrating_npcs.push(i32_of(r)?);
        }
    }

    if version < 174 {
        f.sandstorm_happening = false;
        f.sandstorm_time_left = 0;
        f.sandstorm_severity = 0.0;
        f.sandstorm_intended_severity = 0.0;
    } else {
        f.sandstorm_happening = bool_of(r)?;
        f.sandstorm_time_left = i32_of(r)?;
        f.sandstorm_severity = f32_of(r)?;
        f.sandstorm_intended_severity = f32_of(r)?;
    }

    // `DD2Event.Load` (DD2Event.cs:158): before 178 it resets progress and reads nothing.
    if version < 178 {
        f.saved_bartender = false;
        f.downed_invasion_t1 = false;
        f.downed_invasion_t2 = false;
        f.downed_invasion_t3 = false;
    } else {
        f.saved_bartender = bool_of(r)?;
        f.downed_invasion_t1 = bool_of(r)?;
        f.downed_invasion_t2 = bool_of(r)?;
        f.downed_invasion_t3 = bool_of(r)?;
    }

    // -- the last five background styles, each on its own gate ------------------
    f.bg[8] = if version > 194 { u8_of(r)? } else { 0 };
    f.bg[9] = if version >= 215 { u8_of(r)? } else { 0 };
    if version > 195 {
        f.bg[10] = u8_of(r)?;
        f.bg[11] = u8_of(r)?;
        f.bg[12] = u8_of(r)?;
    } else {
        // Before 196 the three tree backgrounds are the FIRST one, copied.
        f.bg[10] = f.bg[0];
        f.bg[11] = f.bg[0];
        f.bg[12] = f.bg[0];
    }

    f.combat_book_was_used = if version >= 204 { bool_of(r)? } else { false };

    if version < 207 {
        f.lantern_night_cooldown = 0;
        f.lantern_night_genuine = false;
        f.lantern_night_manual = false;
    } else {
        f.lantern_night_cooldown = i32_of(r)?;
        f.lantern_night_genuine = bool_of(r)?;
        f.lantern_night_manual = bool_of(r)?;
        let _next_night_is_genuine = bool_of(r)?;
    }

    // `TreeTopsInfo.Load` (TreeTopsInfo.cs:51): from 211 it reads a count then that many
    // int32s, and it reads only the first 13 - a file claiming more would desynchronise
    // the native server too, so the same limit is kept rather than being "fixed".
    if version < 211 {
        // Older files carry no tree tops: `CopyExistingWorldInfo` builds them from the
        // styles. AreaId 0..3 are the four forest areas, which before 196 are all the
        // FIRST tree background; 4..12 are the biome backgrounds.
        f.tree_tops[0] = f.bg[10] as i32;
        f.tree_tops[1] = f.bg[10] as i32;
        f.tree_tops[2] = f.bg[10] as i32;
        f.tree_tops[3] = f.bg[10] as i32;
        f.tree_tops[4] = f.bg[1] as i32;
        f.tree_tops[5] = f.bg[2] as i32;
        f.tree_tops[6] = f.bg[3] as i32;
        f.tree_tops[7] = f.bg[4] as i32;
        f.tree_tops[8] = f.bg[5] as i32;
        f.tree_tops[9] = f.bg[6] as i32;
        f.tree_tops[10] = f.bg[7] as i32;
        f.tree_tops[11] = f.bg[8] as i32;
        f.tree_tops[12] = f.bg[9] as i32;
    } else {
        let n = i32_of(r)?;
        for i in 0..n.max(0).min(13) as usize {
            f.tree_tops[i] = i32_of(r)?;
        }
    }

    if version >= 212 {
        f.force_halloween_today = bool_of(r)?;
        f.force_xmas_today = bool_of(r)?;
    }
    if version >= 216 {
        f.ore_copper = i32_of(r)?;
        f.ore_iron = i32_of(r)?;
        f.ore_silver = i32_of(r)?;
        f.ore_gold = i32_of(r)?;
    } else {
        f.ore_copper = -1;
        f.ore_iron = -1;
        f.ore_silver = -1;
        f.ore_gold = -1;
    }
    if version >= 217 {
        f.bought_cat = bool_of(r)?;
        f.bought_dog = bool_of(r)?;
        f.bought_bunny = bool_of(r)?;
    }
    if version >= 223 {
        f.downed_empress_of_light = bool_of(r)?;
        f.downed_queen_slime = bool_of(r)?;
    }
    f.downed_deerclops = if version >= 240 { bool_of(r)? } else { false };
    f.unlocked_slime_blue_spawn = if version >= 250 { bool_of(r)? } else { false };
    if version >= 251 {
        f.unlocked_merchant_spawn = bool_of(r)?;
        f.unlocked_demolitionist_spawn = bool_of(r)?;
        f.unlocked_party_girl_spawn = bool_of(r)?;
        f.unlocked_dye_trader_spawn = bool_of(r)?;
        f.unlocked_truffle_spawn = bool_of(r)?;
        f.unlocked_arms_dealer_spawn = bool_of(r)?;
        f.unlocked_nurse_spawn = bool_of(r)?;
        f.unlocked_princess_spawn = bool_of(r)?;
    }
    f.combat_book_volume_two = if version >= 259 { bool_of(r)? } else { false };
    f.peddlers_satchel = if version >= 260 { bool_of(r)? } else { false };
    if version >= 261 {
        f.unlocked_slime_green_spawn = bool_of(r)?;
        f.unlocked_slime_old_spawn = bool_of(r)?;
        f.unlocked_slime_purple_spawn = bool_of(r)?;
        f.unlocked_slime_rainbow_spawn = bool_of(r)?;
        f.unlocked_slime_red_spawn = bool_of(r)?;
        f.unlocked_slime_yellow_spawn = bool_of(r)?;
        f.unlocked_slime_copper_spawn = bool_of(r)?;
    }
    if version >= 264 {
        f.fast_forward_time_to_dusk = bool_of(r)?;
        f.moondial_cooldown = u8_of(r)?;
    }
    if version >= 287 {
        f.force_halloween_forever = bool_of(r)?;
        f.force_xmas_forever = bool_of(r)?;
    }
    f.vampire_seed = if version >= 288 { bool_of(r)? } else { false };
    f.infected_seed = if version >= 296 { bool_of(r)? } else { false };
    if version >= 291 {
        let _meteor_shower_count = i32_of(r)?;
        let _coin_rain = i32_of(r)?;
    }
    if version >= 297 {
        f.team_based_spawns_seed = bool_of(r)?;
        // `ExtraSpawnPointManager.Read` (ExtraSpawnPointManager.cs:437): a byte count,
        // then that many x/y int16 pairs. The `networking` flag changes nothing here.
        let n = u8_of(r)?;
        for _ in 0..n {
            let x = i16_of(r)?;
            let y = i16_of(r)?;
            f.extra_spawn_points.push((x, y));
        }
    }
    // `version >= 304 && reader.ReadBoolean()`: the `&&` SHORT-CIRCUITS, so an older file
    // has no byte here at all. Reading unconditionally would eat the next field.
    f.dual_dungeons_seed = version >= 304 && bool_of(r)?;
    f.more_lightning_seed = version >= 323 && bool_of(r)?;
    f.no_lightning_seed = version >= 323 && bool_of(r)?;

    // A 299..312 file carried four bytes nothing reads. It is not padding: the C# reads
    // them to move the stream, and skipping the read desynchronises the manifest.
    if version >= 299 && version < 313 {
        let _ = u32_of(r)?;
    }
    f.manifest = if version < 299 { String::new() } else { string_of(r)? };

    Ok(f)
}

/// The world sizes the interactive world-select offers (`Main.cs:5473-5483`).
pub const VALID_WORLD_SIZES: [(i32, i32); 3] = [(4200, 1200), (6400, 1800), (8400, 2400)];

/// One section of the file, named, with whether this port decodes it yet.
///
/// The order is `WorldFile.cs:1765` `LoadWorld_Version2`: each entry is a section the
/// C# loads in turn, and several only exist from a given version. Naming them is what
/// makes "unimplemented" visible instead of implied: the reader reports every section
/// the file declares and says which two it actually reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub index: usize,
    pub name: &'static str,
    pub offset: i32,
    /// Whether `load_world` decodes this section's contents.
    pub decoded: bool,
}

/// The section names, in the C#'s load order.
pub const SECTION_NAMES: [&str; 11] = [
    "world header",
    "tiles",
    "chests",
    "signs",
    "NPCs",
    "tile entities",
    "weighted pressure plates",
    "town manager",
    "bestiary",
    "creative powers",
    "footer",
];

/// Why a whole world could not be loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    /// The container could not be read.
    Container(WorldFileError),
    /// The header could not be read.
    Header(WorldFileError),
    /// The flag section could not be read.
    Flags(WorldFileError),
    /// The tile section could not be decoded.
    Tiles(crate::tiles::TileError),
    /// A section pointer is negative, out of order, or past the end of the file.
    BadSectionPointer(usize),
}

/// A world, as far as this port can read one.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedWorld {
    pub container: Container,
    pub header: WorldHeader,
    /// The rest of the header's own record, i.e. the flag section.
    pub flags: WorldFlags,
    pub tile_stats: crate::tiles::TileStats,
    pub sections: Vec<Section>,
}

impl LoadedWorld {
    /// The sections this port does not decode yet, by name.
    pub fn undecoded(&self) -> Vec<&'static str> {
        self.sections.iter().filter(|s| !s.decoded).map(|s| s.name).collect()
    }

    /// The raw bytes of the tile section: `positions[1]` to `positions[2]`.
    ///
    /// `load_world` counts the tiles and throws them away, because reporting a world
    /// should not cost 600 MB. A server that is going to SEND the tiles has to keep them,
    /// and this is how it gets at the bytes to decode them from - the same span
    /// `load_world` already checked ends exactly on `positions[2]`.
    pub fn tile_section<'a>(&self, data: &'a [u8]) -> Option<&'a [u8]> {
        let start = *self.container.positions.get(1)?;
        let end = *self.container.positions.get(2)?;
        if start < 0 || end < start || end as usize > data.len() {
            return None;
        }
        Some(&data[start as usize..end as usize])
    }
}

/// Read a whole world: container, header, tiles.
///
/// The two runtime tables `tiles::decode_tiles` takes are supplied permissively here,
/// and that is safe for this function's purpose because NEITHER one can move the cursor:
/// `wall_count` only decides whether a wall id is zeroed, and `save_slopes` only decides
/// whether a shape in the header byte is recorded. The byte position is unaffected by
/// both, so the section-pointer checks still hold. What it does mean is that the
/// reported slope count is an upper bound, which the caller says out loud rather than
/// hiding.
pub fn load_world(data: &[u8]) -> Result<LoadedWorld, LoadError> {
    let (container, used) = read_container(data).map_err(LoadError::Container)?;
    if container.positions.is_empty() {
        return Err(LoadError::BadSectionPointer(0));
    }
    // The C#'s own check, reproduced: the container must end where the header begins.
    if used as i32 != container.positions[0] {
        return Err(LoadError::BadSectionPointer(0));
    }
    let header_start = container.positions[0] as usize;
    if header_start >= data.len() {
        return Err(LoadError::BadSectionPointer(0));
    }
    let mut r = &data[header_start..];
    let header = read_header(&mut r, container.version).map_err(LoadError::Header)?;
    // Immediately after the header, on the SAME stream: the two are one record.
    let flags = read_flags(&mut r, container.version).map_err(LoadError::Flags)?;

    // The tile section is the span from positions[1] to positions[2]; the C# refuses to
    // continue unless the decode ends exactly on the latter.
    let tile_stats = if container.positions.len() >= 3 {
        let start = container.positions[1];
        let end = container.positions[2];
        if start < 0 || end < start || end as usize > data.len() {
            return Err(LoadError::BadSectionPointer(1));
        }
        let section = &data[start as usize..end as usize];
        let save_slopes = vec![true; 0x10000];
        let (stats, consumed) = crate::tiles::decode_tiles(
            section,
            header.max_tiles_x,
            header.max_tiles_y,
            &container.importance,
            &save_slopes,
            u16::MAX,
        )
        .map_err(LoadError::Tiles)?;
        if consumed != (end - start) as usize {
            return Err(LoadError::BadSectionPointer(2));
        }
        stats
    } else {
        crate::tiles::TileStats::default()
    };

    let sections = container
        .positions
        .iter()
        .enumerate()
        .map(|(i, offset)| Section {
            index: i,
            name: SECTION_NAMES.get(i).copied().unwrap_or("unknown section"),
            offset: *offset,
            // Only the first two are read. The rest are NAMED so the gap is visible.
            decoded: i <= 1,
        })
        .collect();

    Ok(LoadedWorld { container, header, flags, tile_stats, sections })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// Build a container the way the C# writes one, so the reader has something to
    /// fail against without a world file.
    fn synthetic(version: i32, positions: &[i32], importance: &[bool]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&version.to_le_bytes());
        if version >= 135 {
            let magic = MAGIC | ((FILE_TYPE_WORLD as u64) << 56);
            out.extend_from_slice(&magic.to_le_bytes());
            out.extend_from_slice(&7u32.to_le_bytes());
            out.extend_from_slice(&1u64.to_le_bytes()); // favorite
        }
        out.extend_from_slice(&(positions.len() as i16).to_le_bytes());
        for p in positions {
            out.extend_from_slice(&p.to_le_bytes());
        }
        out.extend_from_slice(&(importance.len() as u16).to_le_bytes());
        let mut i = 0;
        while i < importance.len() {
            let mut byte = 0u8;
            for bit in 0..8 {
                if i + bit < importance.len() && importance[i + bit] {
                    byte |= 1 << bit;
                }
            }
            out.push(byte);
            i += 8;
        }
        out
    }

    #[test]
    fn a_container_round_trips_and_reports_its_length() {
        let data = synthetic(KNOWN_VERSION, &[100, 200, 300], &[true, false, true, false, true]);
        let (c, used) = read_container(&data).unwrap();
        assert_eq!(c.version, KNOWN_VERSION);
        assert_eq!(c.revision, 7);
        assert!(c.favorite);
        assert_eq!(c.positions, vec![100, 200, 300]);
        assert_eq!(c.importance, vec![true, false, true, false, true]);
        assert_eq!(used, data.len(), "the container is the whole fixture");
    }

    /// The importance bits are LSB-first within each byte, which is the packing the
    /// C# walks with a shifting mask.
    #[test]
    fn importance_is_lsb_first() {
        let mut bits = vec![false; 10];
        bits[0] = true;
        bits[7] = true;
        bits[8] = true;
        let data = synthetic(KNOWN_VERSION, &[1], &bits);
        let (c, _) = read_container(&data).unwrap();
        assert_eq!(c.importance, bits);
        // Byte 0 holds indices 0..7, byte 1 holds 8..15.
        assert!(c.importance[0] && c.importance[7] && c.importance[8]);
        assert!(!c.importance[1] && !c.importance[9]);
    }

    #[test]
    fn a_foreign_file_is_refused_by_name() {
        let mut data = synthetic(KNOWN_VERSION, &[1], &[]);
        // Corrupt the magic: the low 7 bytes must match exactly.
        data[4] ^= 0xFF;
        assert_eq!(read_container(&data), Err(WorldFileError::BadMagic));

        // Right magic, wrong type byte: a .plr, not a .wld.
        let mut data = synthetic(KNOWN_VERSION, &[1], &[]);
        data[11] = 3; // top byte of the little-endian u64
        assert_eq!(read_container(&data), Err(WorldFileError::NotAWorld));
    }

    #[test]
    fn a_truncated_file_is_too_short_not_a_panic() {
        let data = synthetic(KNOWN_VERSION, &[100, 200], &[true]);
        for cut in 0..data.len() {
            let r = read_container(&data[..cut]);
            assert_eq!(r, Err(WorldFileError::TooShort), "cut at {cut}");
        }
    }

    /// Where the world files live on this machine. The real-file check below is only
    /// as good as the presence of one, and says so rather than pretending.
    fn worlds_dir() -> Option<PathBuf> {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
        let dir = PathBuf::from(home).join("Documents").join("My Games").join("Terraria").join("Worlds");
        if dir.is_dir() {
            Some(dir)
        } else {
            None
        }
    }

    /// The per-file checks, so every world present is checked rather than the first.
    fn check_one(path: &std::path::Path) -> WorldHeader {
        let data = fs::read(path).expect("the world file is readable");
        let (c, used) = read_container(&data).expect("a real world's container parses");
        assert!(!c.positions.is_empty());
        assert_eq!(
            used as i32, c.positions[0],
            "{}: the container must end exactly where the C# says the header begins",
            path.display()
        );
        // The header starts at that pointer, so the C# check is reproduced rather than
        // approximated.
        let mut r = &data[c.positions[0] as usize..];
        let h = read_header(&mut r, c.version).expect("the header parses");
        let f = read_flags(&mut r, c.version).expect("the flag section parses");

        // The exact check, and the one that makes the flag walk verifiable at all: the
        // header and the flags are ONE record, and `positions[1]` is where it ends. Every
        // field of the flag section is read only to advance the stream, so a single
        // wrong width or a missed version gate lands the cursor somewhere else and this
        // fails. Nothing weaker would catch a one-byte drift.
        assert_eq!(
            data.len() - r.len(),
            c.positions[1] as usize,
            "{}: header+flags must end exactly on positions[1]",
            path.display()
        );

        assert!(!h.name.is_empty(), "a world has a name");
        assert!(h.name.is_ascii(), "world names are ascii in practice: {:?}", h.name);
        // The flags are not just walked: the values they yield have to be sane, or the
        // walk landed on the right byte with the wrong interpretation.
        assert_eq!(f.version, c.version);
        assert!(
            f.dungeon_x > 0 && f.dungeon_x < h.max_tiles_x && f.dungeon_y > 0 && f.dungeon_y < h.max_tiles_y,
            "{}: the dungeon ({}, {}) sits inside the world",
            path.display(),
            f.dungeon_x,
            f.dungeon_y
        );
        assert!(
            f.ore_copper == -1 || f.ore_copper > 0,
            "an ore tier is either unset (-1) or a real tile id, not {}",
            f.ore_copper
        );
        assert!(h.max_tiles_x > 0 && h.max_tiles_y > 0);
        assert!(
            VALID_WORLD_SIZES.contains(&(h.max_tiles_x, h.max_tiles_y)),
            "{}x{} is not one of the three sizes the game offers",
            h.max_tiles_x,
            h.max_tiles_y
        );
        assert!(h.right_world >= h.left_world && h.bottom_world >= h.top_world);
        assert!(
            h.spawn_tile_x >= 0 && h.spawn_tile_x < h.max_tiles_x,
            "spawn x {} is outside 0..{}",
            h.spawn_tile_x,
            h.max_tiles_x
        );
        assert!(
            h.spawn_tile_y >= 0 && h.spawn_tile_y < h.max_tiles_y,
            "spawn y {} is outside 0..{}",
            h.spawn_tile_y,
            h.max_tiles_y
        );
        assert!(
            h.world_surface > 0.0 && h.world_surface <= h.rock_layer,
            "surface {} must be above rock layer {}",
            h.world_surface,
            h.rock_layer
        );
        assert!(h.rock_layer < h.max_tiles_y as f64);
        assert!(h.unique_id.iter().any(|b| *b != 0), "the GUID is not all zeros");
        assert!(!h.seed.is_empty() || h.generator_version > 0);
        println!(
            "{} (file version {}): {:?} {}x{} mode={} seed={:?} spawn=({},{}) surface={} rock={}",
            path.file_name().unwrap().to_string_lossy(),
            c.version,
            h.name,
            h.max_tiles_x,
            h.max_tiles_y,
            h.game_mode,
            h.seed,
            h.spawn_tile_x,
            h.spawn_tile_y,
            h.world_surface,
            h.rock_layer
        );
        h
    }

    /// `load_world` ties the three pieces together, and names what it did NOT read.
    ///
    /// The undecoded list is the part that matters: a loader that reported only its own
    /// successes would be indistinguishable from one that reads everything.
    #[test]
    fn load_world_reports_what_it_did_not_read() {
        let Some(dir) = worlds_dir() else {
            println!("no Terraria Worlds directory; the load_world check did not run");
            return;
        };
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)
            .expect("the Worlds directory is readable")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "wld"))
            .collect();
        files.sort();
        let Some(path) = files.first() else {
            println!("no .wld in {}; the load_world check did not run", dir.display());
            return;
        };

        let data = fs::read(path).expect("the world file is readable");
        let w = load_world(&data).expect("the world loads");
        assert!(!w.header.name.is_empty());
        assert!(w.tile_stats.active > 0);
        // Eleven sections in a 1.4.4 file, and the first two are the ones read.
        assert!(w.sections.len() >= 3, "only {} sections", w.sections.len());
        assert_eq!(w.sections[0].name, "world header");
        assert_eq!(w.sections[1].name, "tiles");
        assert!(w.sections[0].decoded && w.sections[1].decoded);
        assert!(
            w.sections[2..].iter().all(|s| !s.decoded),
            "only the header and the tiles are read"
        );
        assert!(w.undecoded().contains(&"chests"));
        // Every section pointer is inside the file and in order.
        for pair in w.sections.windows(2) {
            assert!(pair[0].offset <= pair[1].offset, "{:?} then {:?}", pair[0], pair[1]);
        }
        let last = w.sections.last().unwrap();
        assert!(last.offset >= 0 && (last.offset as usize) < data.len());
    }

    /// A file whose section pointers disagree with its own contents is refused rather
    /// than decoded optimistically.
    #[test]
    fn load_world_refuses_a_broken_section_pointer() {
        // A container whose positions[0] does not match where the container ended.
        let mut data = synthetic(KNOWN_VERSION, &[999, 1000, 1001], &[false]);
        data.extend_from_slice(&[0u8; 64]);
        assert_eq!(load_world(&data), Err(LoadError::BadSectionPointer(0)));

        // A container that ends correctly but declares an empty section table.
        let data = synthetic(KNOWN_VERSION, &[], &[]);
        assert_eq!(load_world(&data), Err(LoadError::BadSectionPointer(0)));
    }

    /// A REAL world file, end to end: the container must land exactly on `positions[0]`,
    /// and the header must describe a plausible world.
    ///
    /// EVERY `.wld` present is checked, not the first, because a second file is what
    /// would expose a version guard that is wrong - the two on this machine were written
    /// fifteen months apart. On a machine with no world files this asserts nothing and
    /// says so out loud; the synthetic tests above are what run everywhere.
    #[test]
    fn a_real_world_parses_and_lands_on_the_header_pointer() {
        let Some(dir) = worlds_dir() else {
            println!("no Terraria Worlds directory; the real-file check did not run");
            return;
        };
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)
            .expect("the Worlds directory is readable")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "wld"))
            .collect();
        files.sort();
        if files.is_empty() {
            println!("no .wld in {}; the real-file check did not run", dir.display());
            return;
        }
        let headers: Vec<WorldHeader> = files.iter().map(|p| check_one(p)).collect();
        assert_eq!(headers.len(), files.len());
    }
}
